//! The communicator over a scripted transport, a verifier and peer rules that stand in for the
//! real ones, on tokio's paused clock: each route's request, receive limit and timeout, the error
//! classes and the sequential-error disconnect, the throttle, the postBlock height, the oversize
//! unconfirmed reply, the postTransactions queues, the ping and its attestations, and the
//! verifier's two requests.
//!
//! The same cases over the real peer rules and over the connector with the mock peer are the
//! `integration` tests at the end.

mod peer_fakes;

use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use heartwood_crypto::{BlockId, TransactionId};
use heartwood_kernel::Event;
use heartwood_p2p::communicator::BlocksRequest;
use heartwood_p2p::errors::{PastDeadline, PingError};
use heartwood_p2p::seams::{PeerClient, VerifierLink};
use heartwood_p2p::{ChainView, DownloadedBlock, SessionError, Verification};
use heartwood_p2p_wire::RouteId;
use heartwood_p2p_wire::codecs::blocks::{self, ListedBlock};
use heartwood_p2p_wire::codecs::transactions::{self, WithTransactions};
use heartwood_p2p_wire::codecs::{Amount, peer as peer_codecs};
use heartwood_p2p_wire::list;
use heartwood_p2p_wire::messages::blocks::{BlockHeader, PostBlockResponse};
use heartwood_p2p_wire::messages::peer::{Common, GetCommonBlocksResponse, Peer as ListedPeer};
use heartwood_p2p_wire::messages::transactions::GetUnconfirmedTransactionsResponse;
use heartwood_p2p_wire::replies::{CommonBlock, PeerAddress};
use heartwood_p2p_wire::requests::ClientRequest;
use peer_fakes::*;
use tokio::time::Instant;

const KIB: usize = 1024;

/// A reply whose field 15 claims 5 bytes it does not have: the codec's `RangeError`.
fn truncated() -> Vec<u8> {
    vec![0x7a, 0x05]
}

/// A reply with a field of wire type 7: the codec's error of class `Error`.
fn bad_wire_type() -> Vec<u8> {
    vec![0x2f]
}

fn peers_reply(peers: &[(&str, u32)]) -> Vec<u8> {
    let peers: Vec<ListedPeer> = peers
        .iter()
        .map(|(ip, port)| ListedPeer {
            ip: Some((*ip).to_owned()),
            port: Some(*port),
            ..ListedPeer::default()
        })
        .collect();
    peer_codecs::get_peers_encode_response(&peers)
}

fn listed(height: u32, transactions: Vec<Vec<u8>>) -> ListedBlock {
    ListedBlock {
        header: BlockHeader {
            height: Some(height),
            id: Some(format!("{height:064x}")),
            ..BlockHeader::default()
        },
        total_amount: Amount::ZERO,
        total_fee: Amount::ZERO,
        reward: Amount::ZERO,
        transactions,
    }
}

fn blocks_reply(blocks: &[ListedBlock]) -> Vec<u8> {
    blocks::get_blocks_encode_response(blocks)
}

fn unconfirmed_reply(count: usize) -> Vec<u8> {
    transactions::get_unconfirmed_transactions_encode_response(&WithTransactions {
        message: GetUnconfirmedTransactionsResponse {
            pool_size: Some(u32::try_from(count).unwrap()),
            ..GetUnconfirmedTransactionsResponse::default()
        },
        transactions: (0..count)
            .map(|i| vec![u8::try_from(i % 251).unwrap(); 3])
            .collect(),
    })
}

fn post_block_reply(height: Option<u32>) -> Vec<u8> {
    blocks::post_block_encode_response(&PostBlockResponse {
        status: Some(true),
        height,
        ..PostBlockResponse::default()
    })
}

fn stored(fixture: &Fixture, last: u8) -> heartwood_p2p::SharedPeer {
    let peer = peer(last);
    fixture.repository.set(peer.clone());
    peer
}

fn ids(count: u8) -> Vec<BlockId> {
    (1..=count).map(|i| BlockId::from_array([i; 32])).collect()
}

#[tokio::test(start_paused = true)]
async fn each_route_sends_its_request_with_its_limit_and_timeout() {
    let fixture = Fixture::new();
    let client = &fixture.communicator;
    let peer = stored(&fixture, 1);
    let transport = &fixture.transport;
    transport.answer(
        RouteId::GetPeers,
        Answer::Bytes(peers_reply(&[("192.0.2.9", 4002)])),
    );
    let common = GetCommonBlocksResponse {
        common: Some(Common {
            height: Some(3),
            id: Some("ab".repeat(32)),
            ..Common::default()
        }),
        ..GetCommonBlocksResponse::default()
    };
    for _ in 0..2 {
        let reply = peer_codecs::get_common_blocks_encode_response(&common);
        transport.answer(RouteId::GetCommonBlocks, Answer::Bytes(reply));
    }
    for _ in 0..3 {
        transport.answer(
            RouteId::GetBlocks,
            Answer::Bytes(blocks_reply(&[listed(6, vec![])])),
        );
    }
    transport.answer(
        RouteId::GetUnconfirmedTransactions,
        Answer::Bytes(unconfirmed_reply(2)),
    );
    transport.answer(RouteId::PostBlock, Answer::Bytes(post_block_reply(None)));
    let accept = transactions::post_transactions_encode_response(&["aa".repeat(32)]);
    transport.answer(RouteId::PostTransactions, Answer::Bytes(accept));

    let addresses = client.get_peers(&peer, true).await.unwrap();
    assert_eq!(
        addresses,
        [PeerAddress {
            ip: "192.0.2.9".into(),
            port: 4002
        }]
    );
    let expected = CommonBlock {
        height: 3,
        id: "ab".repeat(32),
    };
    let short = Duration::from_secs(2);
    assert_eq!(
        client.has_common_blocks(&peer, &ids(9), short).await,
        Some(expected.clone())
    );
    let long = Duration::from_secs(7);
    assert_eq!(
        client.has_common_blocks(&peer, &ids(1), long).await,
        Some(expected)
    );
    let full = client
        .get_peer_blocks(&peer, BlocksRequest::after(5))
        .await
        .unwrap();
    assert_eq!(full.len(), 1);
    assert_eq!(full[0].transactions, Some(vec![]));
    assert_eq!(full[0].ip, peer.ip());
    let headers = BlocksRequest {
        block_limit: 10,
        headers_only: true,
        timeout: Some(Duration::from_secs(2)),
        ..BlocksRequest::after(7)
    };
    let fetched = client.get_peer_blocks(&peer, headers).await.unwrap();
    assert_eq!(fetched[0].transactions, None);
    let own_timeout = BlocksRequest {
        timeout: Some(Duration::from_secs(2)),
        ..BlocksRequest::after(8)
    };
    client.get_peer_blocks(&peer, own_timeout).await.unwrap();
    let exclude = [TransactionId::from_array([7; 32])];
    assert_eq!(
        client
            .get_unconfirmed_transactions(&peer, &exclude)
            .await
            .len(),
        2
    );
    client.post_block(&peer, &block(11)).await;
    client.post_transactions(&peer, Arc::new(vec![vec![1, 2, 3]]));
    tokio::time::sleep(Duration::from_secs(1)).await;

    let seconds = Duration::from_secs;
    let sent: Vec<(RouteId, Duration, usize)> = transport
        .sent()
        .iter()
        .map(|sent| (sent.route, sent.timeout, sent.limit))
        .collect();
    assert_eq!(
        sent,
        [
            (RouteId::GetPeers, seconds(5), 100 * KIB),
            (RouteId::GetCommonBlocks, seconds(2), 100 * KIB),
            (RouteId::GetCommonBlocks, seconds(5), 100 * KIB),
            (RouteId::GetBlocks, seconds(30), 20 * 1024 * KIB),
            (RouteId::GetBlocks, seconds(2), 10 * KIB),
            (RouteId::GetBlocks, seconds(2), 20 * 1024 * KIB),
            (RouteId::GetUnconfirmedTransactions, seconds(10), 2_097_152),
            (RouteId::PostBlock, seconds(10), 100 * KIB),
            (RouteId::PostTransactions, seconds(10), 100 * KIB),
        ]
    );
    assert!(
        transport
            .sent()
            .iter()
            .all(|sent| sent.address == peer.address())
    );
    let encoded = fixture.rules.encoded.lock().unwrap().clone();
    let non_zero = |n| std::num::NonZeroU32::new(n).unwrap();
    assert_eq!(encoded[0], ClientRequest::GetPeers);
    assert!(
        matches!(&encoded[1], ClientRequest::GetCommonBlocks { ids: sent } if sent.ids() == ids(9))
    );
    assert_eq!(
        encoded[3],
        ClientRequest::GetBlocks {
            last_block_height: non_zero(5),
            block_limit: non_zero(400),
            headers_only: false
        }
    );
    assert_eq!(
        encoded[4],
        ClientRequest::GetBlocks {
            last_block_height: non_zero(7),
            block_limit: non_zero(10),
            headers_only: true
        }
    );
    assert_eq!(
        encoded[6],
        ClientRequest::GetUnconfirmedTransactions {
            exclude: exclude.to_vec()
        }
    );
    let posted = heartwood_crypto::blocks::serialiser::serialise(&block(11));
    assert!(
        matches!(&encoded[7], ClientRequest::PostBlock { block } if block.len() == posted.len())
    );
    assert_eq!(
        encoded[8],
        ClientRequest::PostTransactions {
            transactions: vec![vec![1, 2, 3]]
        }
    );
    // Each request forgot the IP's last error first, and nothing failed.
    assert_eq!(
        transport
            .forgotten
            .load(std::sync::atomic::Ordering::SeqCst),
        9
    );
    assert!(transport.errors().is_empty());
    assert_eq!(peer.snapshot().sequential_errors, 0);
    assert_eq!(peer.snapshot().latency, Some(0));
}

#[tokio::test(start_paused = true)]
async fn requests_the_reference_never_writes_are_not_sent() {
    let fixture = Fixture::new();
    let client = &fixture.communicator;
    let peer = stored(&fixture, 1);
    // No common-block id, more than ten, a height or a limit of 0.
    assert_eq!(
        client
            .has_common_blocks(&peer, &[], Duration::from_secs(1))
            .await,
        None
    );
    assert_eq!(
        client
            .has_common_blocks(&peer, &ids(11), Duration::from_secs(1))
            .await,
        None
    );
    assert!(
        client
            .get_peer_blocks(&peer, BlocksRequest::after(0))
            .await
            .unwrap()
            .is_empty()
    );
    let none = BlocksRequest {
        block_limit: 0,
        ..BlocksRequest::after(3)
    };
    assert!(
        client
            .get_peer_blocks(&peer, none)
            .await
            .unwrap()
            .is_empty()
    );
    // A node without a version sends nothing at all.
    let unversioned = Fixture::with(
        Chain::at(10),
        heartwood_p2p::P2pOptions {
            version: String::new(),
            ..options()
        },
    );
    let other = stored(&unversioned, 2);
    assert_eq!(unversioned.communicator.get_peers(&other, true).await, None);
    assert!(fixture.transport.sent().is_empty());
    assert!(unversioned.transport.sent().is_empty());
    assert_eq!(other.snapshot().sequential_errors, 0);
}

#[tokio::test(start_paused = true)]
async fn the_error_class_decides_whether_the_peer_is_disconnected() {
    let fixture = Fixture::new();
    let client = &fixture.communicator;
    let transport = &fixture.transport;
    // A decoder's RangeError on getPeers: counted, and the peer is disconnected and forgotten.
    let first = stored(&fixture, 1);
    transport.answer(RouteId::GetPeers, Answer::Bytes(truncated()));
    assert_eq!(client.get_peers(&first, true).await, None);
    assert!(!fixture.repository.has(first.ip()));
    assert_eq!(transport.disconnected(), [first.ip()]);
    assert_eq!(transport.errors(), [(first.ip(), "RangeError")]);
    assert_eq!(fixture.events.all(), [Event::PeerDisconnect]);
    assert_eq!(first.snapshot().sequential_errors, 1);

    // The same class on getBlocks and getUnconfirmedTransactions is only counted.
    let second = stored(&fixture, 2);
    // A block whose height is a varint cut short.
    let cut = list::encode(&[vec![0x38, 0x80]]);
    transport.answer(RouteId::GetBlocks, Answer::Bytes(cut));
    let blocks = client
        .get_peer_blocks(&second, BlocksRequest::after(1))
        .await;
    assert!(blocks.unwrap().is_empty());
    // (The decode failure is a protocol violation, so this peer is no source any more: the
    // next source request goes to another peer.)
    let fifth = stored(&fixture, 5);
    transport.answer(
        RouteId::GetUnconfirmedTransactions,
        Answer::Bytes(truncated()),
    );
    assert!(
        client
            .get_unconfirmed_transactions(&fifth, &[])
            .await
            .is_empty()
    );
    assert!(fixture.repository.has(second.ip()) && fixture.repository.has(fifth.ip()));
    assert_eq!(second.snapshot().sequential_errors, 1);
    assert_eq!(fifth.snapshot().sequential_errors, 1);

    // Errors of class Error (a decoder's wire type, a timeout, a server error) are only counted.
    // (A server error is no violation; the decode failure after it is.)
    let third = stored(&fixture, 3);
    let server = SessionError::Server {
        status: 500,
        payload: "boom".into(),
    };
    transport.answer(RouteId::GetPeers, Answer::Fail(server));
    assert_eq!(client.get_peers(&third, true).await, None);
    assert!(third.snapshot().violations.is_empty());
    transport.answer(RouteId::GetPeers, Answer::Bytes(bad_wire_type()));
    assert_eq!(client.get_peers(&third, true).await, None);
    assert!(fixture.repository.has(third.ip()));
    assert_eq!(third.snapshot().sequential_errors, 2);

    // A reply that fails its schema resets the counter first, then counts: it never reaches 3.
    let fourth = stored(&fixture, 4);
    fourth.with(|peer| peer.sequential_errors = 2);
    fixture.rules.refuse(RouteId::GetPeers);
    transport.answer(RouteId::GetPeers, Answer::Bytes(peers_reply(&[])));
    assert_eq!(client.get_peers(&fourth, true).await, None);
    assert_eq!(fourth.snapshot().sequential_errors, 1);
    assert!(fixture.repository.has(fourth.ip()));
    let classes: Vec<&str> = transport.errors().iter().map(|(_, class)| *class).collect();
    assert_eq!(
        classes,
        [
            "RangeError",
            "RangeError",
            "RangeError",
            "Error",
            "Error",
            "CoreValidationError"
        ]
    );
    assert_eq!(transport.disconnected(), [first.ip()]);
}

#[tokio::test(start_paused = true)]
async fn three_failures_in_a_row_disconnect_and_forget_the_peer() {
    let fixture = Fixture::new();
    let client = &fixture.communicator;
    let peer = stored(&fixture, 1);
    // Two timeouts, a success that resets the counter, then three timeouts.
    let started = Instant::now();
    client.get_peers(&peer, true).await;
    client.get_peers(&peer, true).await;
    assert_eq!(peer.snapshot().sequential_errors, 2);
    fixture
        .transport
        .answer(RouteId::GetPeers, Answer::Bytes(peers_reply(&[])));
    assert_eq!(client.get_peers(&peer, true).await, Some(vec![]));
    assert_eq!(peer.snapshot().sequential_errors, 0);
    for expected in 1..=2 {
        client.get_peers(&peer, true).await;
        assert_eq!(peer.snapshot().sequential_errors, expected);
        assert!(fixture.repository.has(peer.ip()));
    }
    client.get_peers(&peer, true).await;
    assert!(!fixture.repository.has(peer.ip()));
    assert_eq!(fixture.transport.disconnected(), [peer.ip()]);
    assert_eq!(fixture.events.all(), [Event::PeerDisconnect]);
    // Each timeout took the getPeers timeout of 5 s.
    assert_eq!(started.elapsed(), Duration::from_secs(25));
}

#[tokio::test(start_paused = true)]
async fn the_throttle_waits_until_the_limiter_lets_the_request_through() {
    let fixture = Fixture::new();
    let peer = stored(&fixture, 1);
    fixture
        .rules
        .waits
        .lock()
        .unwrap()
        .extend([Duration::from_secs(1); 2]);
    fixture
        .transport
        .answer(RouteId::GetPeers, Answer::Bytes(peers_reply(&[])));
    let started = Instant::now();
    fixture.communicator.get_peers(&peer, true).await;
    assert_eq!(
        fixture.transport.sent()[0].at - started,
        Duration::from_secs(2)
    );
    // Whether a request would wait is the limiter's answer, without consuming.
    assert!(
        !fixture
            .communicator
            .would_throttle(peer.ip(), RouteId::GetBlocks)
    );
    fixture
        .rules
        .throttled
        .store(true, std::sync::atomic::Ordering::SeqCst);
    assert!(
        fixture
            .communicator
            .would_throttle(peer.ip(), RouteId::GetBlocks)
    );
}

#[tokio::test(start_paused = true)]
async fn a_post_block_reply_with_a_height_sets_the_peers_height() {
    let fixture = Fixture::new();
    let client = &fixture.communicator;
    let peer = stored(&fixture, 1);
    // A peer with a state, from a ping.
    fixture.transport.answer(
        RouteId::GetStatus,
        Answer::Bytes(devnet_status(&block(10), 125, &[])),
    );
    client
        .ping(&peer, Duration::from_secs(5), None, true, false)
        .await
        .unwrap();
    assert_eq!(peer.snapshot().height(), Some(10));
    fixture.transport.answer(
        RouteId::PostBlock,
        Answer::Bytes(post_block_reply(Some(77))),
    );
    client.post_block(&peer, &block(11)).await;
    assert_eq!(peer.snapshot().height(), Some(77));
    fixture
        .transport
        .answer(RouteId::PostBlock, Answer::Bytes(post_block_reply(None)));
    client.post_block(&peer, &block(12)).await;
    assert_eq!(peer.snapshot().height(), Some(77));
    // A peer without a state keeps none.
    let fresh = stored(&fixture, 2);
    fixture
        .transport
        .answer(RouteId::PostBlock, Answer::Bytes(post_block_reply(Some(5))));
    client.post_block(&fresh, &block(11)).await;
    assert_eq!(fresh.snapshot().state, None);
}

#[tokio::test(start_paused = true)]
async fn an_unconfirmed_reply_above_the_current_milestone_is_discarded() {
    // 150 transactions a block at our height, 300 from height 1,000: the reply check allows 300.
    let chain = Chain::with(10, milestones_with_max_transactions(1_000, 300));
    let fixture = Fixture::with(chain, options());
    let client = &fixture.communicator;
    let peer = stored(&fixture, 1);
    let route = RouteId::GetUnconfirmedTransactions;
    fixture
        .transport
        .answer(route, Answer::Bytes(unconfirmed_reply(151)));
    fixture
        .transport
        .answer(route, Answer::Bytes(unconfirmed_reply(150)));
    fixture
        .transport
        .answer(route, Answer::Bytes(unconfirmed_reply(301)));
    assert!(
        client
            .get_unconfirmed_transactions(&peer, &[])
            .await
            .is_empty()
    );
    assert_eq!(peer.snapshot().sequential_errors, 0);
    assert_eq!(
        client.get_unconfirmed_transactions(&peer, &[]).await.len(),
        150
    );
    // Above the largest of the milestones, the reply fails its schema.
    assert!(
        client
            .get_unconfirmed_transactions(&peer, &[])
            .await
            .is_empty()
    );
    assert_eq!(peer.snapshot().sequential_errors, 1);
    assert!(
        fixture
            .transport
            .sent()
            .iter()
            .all(|sent| sent.limit == 2_097_152)
    );
}

#[tokio::test(start_paused = true)]
async fn each_peer_posts_transactions_through_its_own_queue() {
    let fixture = Fixture::new();
    let client = &fixture.communicator;
    let (first, second) = (stored(&fixture, 1), stored(&fixture, 2));
    let accept = transactions::post_transactions_encode_response(&[]);
    for _ in 0..4 {
        let answer = Answer::After(
            Duration::from_millis(100),
            Box::new(Answer::Bytes(accept.clone())),
        );
        fixture.transport.answer(RouteId::PostTransactions, answer);
    }
    let started = Instant::now();
    for batch in 0..3_u8 {
        client.post_transactions(&first, Arc::new(vec![vec![batch]]));
    }
    client.post_transactions(&second, Arc::new(vec![vec![9]]));
    tokio::time::sleep(Duration::from_secs(2)).await;
    let at = |ip: IpAddr| -> Vec<Duration> {
        let sent = fixture.transport.sent_on(RouteId::PostTransactions);
        sent.iter()
            .filter(|sent| sent.address.ip() == ip)
            .map(|sent| sent.at - started)
            .collect()
    };
    // Each request takes 100 ms, then the queue waits ceil(1000 / 25) = 40 ms.
    let ms = Duration::from_millis;
    assert_eq!(at(first.ip()), [ms(0), ms(140), ms(280)]);
    assert_eq!(at(second.ip()), [ms(0)]);
    let batches: Vec<ClientRequest> = fixture.rules.encoded.lock().unwrap().clone();
    let first_batches: Vec<_> = batches
        .iter()
        .filter_map(|request| match request {
            ClientRequest::PostTransactions { transactions } if transactions[0][0] < 9 => {
                Some(transactions[0][0])
            }
            _ => None,
        })
        .collect();
    assert_eq!(first_batches, [0, 1, 2]);
}

#[tokio::test(start_paused = true)]
async fn a_removed_peers_queue_still_sends_what_it_holds() {
    let fixture = Fixture::new();
    let client = &fixture.communicator;
    let peer = stored(&fixture, 1);
    let accept = transactions::post_transactions_encode_response(&[]);
    for _ in 0..3 {
        let answer = Answer::After(
            Duration::from_millis(100),
            Box::new(Answer::Bytes(accept.clone())),
        );
        fixture.transport.answer(RouteId::PostTransactions, answer);
    }
    client.post_transactions(&peer, Arc::new(vec![vec![1]]));
    client.post_transactions(&peer, Arc::new(vec![vec![2]]));
    tokio::time::sleep(Duration::from_millis(10)).await;
    client.remove_peer(peer.ip());
    assert!(!fixture.repository.has(peer.ip()));
    assert_eq!(fixture.transport.disconnected(), [peer.ip()]);
    assert_eq!(fixture.events.all(), [Event::PeerDisconnect]);
    // A new batch after the removal goes through a new queue, in parallel with the old one.
    client.post_transactions(&peer, Arc::new(vec![vec![3]]));
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(
        fixture.transport.sent_on(RouteId::PostTransactions).len(),
        3
    );
}

// ---- ping -----------------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn a_ping_verifies_the_peer_and_records_its_state() {
    let fixture = Fixture::new();
    let client = &fixture.communicator;
    let peer = stored(&fixture, 1);
    let tip = fixture.chain.last_block();
    let signers = [delegate(1), delegate(2)];
    fixture.transport.answer(
        RouteId::GetStatus,
        Answer::Bytes(devnet_status(&tip, 125, &signers)),
    );
    fixture.probe.open.lock().unwrap().insert(4003);
    let state = client
        .ping(
            &peer,
            Duration::from_secs(60),
            Some(slot(125)),
            false,
            false,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(state.height, 10);
    assert_eq!(state.current_slot, 125);
    assert_eq!(state.header_id(), Some(tip.id().to_hex().as_str()));
    let pinged = peer.snapshot();
    assert_eq!(pinged.state, Some(state.clone()));
    assert_eq!(pinged.last_pinged, Some(START_MS));
    assert!(!pinged.stale);
    assert_eq!(pinged.version.as_deref(), Some("4.3.1"));
    assert_eq!(pinged.plugins.len(), 1);
    let keys: Vec<_> = signers
        .iter()
        .map(|k| k.public_key().to_compressed())
        .collect();
    assert_eq!(pinged.public_keys, keys);
    let online = fixture.online.0.lock().unwrap().clone();
    assert_eq!(
        online,
        keys.iter()
            .map(|key| (*key, 1, "4.3.1".to_owned()))
            .collect::<Vec<_>>()
    );
    // The verifier got the claimed state and a deadline 60 s away; getStatus took 5 s at most.
    assert_eq!(fixture.verifier.claimed.lock().unwrap()[0], state);
    assert_eq!(fixture.transport.sent()[0].timeout, Duration::from_secs(5));
    let deadline = fixture.verifier.deadlines.lock().unwrap()[0];
    assert_eq!(deadline.remaining().unwrap(), Duration::from_secs(60));

    // Plugin ports: the API answers 200.
    client.ping_ports(&peer).await;
    let ports: Vec<(String, i32)> = peer.snapshot().ports.into_iter().collect();
    assert_eq!(ports, [("@solar-network/api".to_owned(), 4003)]);
}

#[tokio::test(start_paused = true)]
async fn a_ping_skips_ignored_and_recently_pinged_peers() {
    let fixture = Fixture::new();
    let client = &fixture.communicator;
    let status = devnet_status(&block(10), 125, &[]);
    let ignored = stored(&fixture, 1);
    ignored.with(|peer| (1..=3).for_each(|i| peer.add_infraction(START_MS - i)));
    let ping = |peer, force| client.ping(peer, Duration::from_secs(5), None, force, false);
    assert_eq!(ping(&ignored, true).await, Ok(None));
    // The version defaults to our own before anything else.
    assert_eq!(ignored.snapshot().version.as_deref(), Some("4.3.1"));

    let recent = stored(&fixture, 2);
    recent.with(|peer| {
        peer.stale = false;
        peer.last_pinged = Some(START_MS - 60_000);
    });
    assert_eq!(ping(&recent, false).await, Ok(None));
    assert!(fixture.transport.sent().is_empty());
    fixture
        .transport
        .answer(RouteId::GetStatus, Answer::Bytes(status.clone()));
    assert!(ping(&recent, true).await.unwrap().is_some());
    // A stale peer is pinged however recently it was.
    recent.with(|peer| peer.stale = true);
    fixture
        .transport
        .answer(RouteId::GetStatus, Answer::Bytes(status));
    assert!(ping(&recent, false).await.unwrap().is_some());
}

#[tokio::test(start_paused = true)]
async fn a_ping_fails_as_the_reference_names_it() {
    let fixture = Fixture::new();
    let client = &fixture.communicator;
    let peer = stored(&fixture, 1);
    let transport = &fixture.transport;
    let ping = |timeout| client.ping(&peer, timeout, None, true, false);
    let five = Duration::from_secs(5);
    // No reply.
    assert_eq!(ping(five).await, Err(PingError::StatusResponse(peer.ip())));
    // Another network.
    let foreign = peer_config("4.3.1", &"00".repeat(32));
    transport.answer(
        RouteId::GetStatus,
        Answer::Bytes(status_reply(&block(10), 1, &foreign, &[])),
    );
    assert_eq!(ping(five).await, Err(PingError::VerificationFailed));
    assert_eq!(peer.snapshot().version.as_deref(), Some("4.3.1"));
    // A version the gate refuses: the peer's version is the reported one.
    fixture
        .rules
        .bad_versions
        .lock()
        .unwrap()
        .insert("3.9.9".into());
    let old = peer_config("3.9.9", &DEVNET.network.nethash);
    transport.answer(
        RouteId::GetStatus,
        Answer::Bytes(status_reply(&block(10), 1, &old, &[])),
    );
    assert_eq!(ping(five).await, Err(PingError::VerificationFailed));
    assert_eq!(peer.snapshot().version.as_deref(), Some("3.9.9"));
    // The deadline passed while getStatus was answered.
    let status = devnet_status(&block(10), 1, &[]);
    let slow = Answer::After(
        Duration::from_secs(3),
        Box::new(Answer::Bytes(status.clone())),
    );
    transport.answer(RouteId::GetStatus, slow);
    assert_eq!(
        ping(Duration::from_secs(3)).await,
        Err(PingError::PingTimeout { timeout_ms: 3_000 })
    );
    assert_eq!(
        transport.sent().last().unwrap().timeout,
        Duration::from_secs(3)
    );
    // The deadline passed during the verification.
    *fixture.verifier.full.lock().unwrap() = Err(PastDeadline);
    transport.answer(RouteId::GetStatus, Answer::Bytes(status.clone()));
    assert_eq!(ping(five).await, Err(PingError::Deadline(PastDeadline)));
    // The chain did not verify.
    *fixture.verifier.full.lock().unwrap() = Ok(None);
    transport.answer(RouteId::GetStatus, Answer::Bytes(status));
    assert_eq!(ping(five).await, Err(PingError::VerificationFailed));
    assert_eq!(peer.snapshot().verification, None);
    // None of these set a state.
    assert_eq!(peer.snapshot().state, None);
    assert!(peer.snapshot().stale);
}

#[tokio::test(start_paused = true)]
async fn a_fast_ping_still_needs_a_full_verification_on_record() {
    let fixture = Fixture::new();
    let client = &fixture.communicator;
    let peer = stored(&fixture, 1);
    let status = devnet_status(&block(10), 1, &[]);
    let fast = |peer| client.ping(peer, Duration::from_millis(2_500), None, true, true);
    fixture
        .transport
        .answer(RouteId::GetStatus, Answer::Bytes(status.clone()));
    // Never verified in full: the reference's check of the full result refuses it.
    assert_eq!(fast(&peer).await, Err(PingError::VerificationFailed));
    assert!(peer.snapshot().fast_verification.is_some());
    let verified = Verification {
        my_height: 10,
        their_height: 10,
        highest_common_height: 10,
    };
    peer.with(|peer| peer.verification = Some(verified));
    fixture
        .transport
        .answer(RouteId::GetStatus, Answer::Bytes(status.clone()));
    assert!(fast(&peer).await.unwrap().is_some());
    assert_eq!(
        fixture.transport.sent()[1].timeout,
        Duration::from_millis(2_500)
    );
    *fixture.verifier.fast.lock().unwrap() = None;
    fixture
        .transport
        .answer(RouteId::GetStatus, Answer::Bytes(status));
    assert_eq!(fast(&peer).await, Err(PingError::VerificationFailed));
    assert_eq!(peer.snapshot().fast_verification, None);
}

#[tokio::test(start_paused = true)]
async fn a_negative_slot_reads_the_peers_slot_as_signed() {
    let fixture = Fixture::new();
    let client = &fixture.communicator;
    let peer = stored(&fixture, 1);
    let status = devnet_status(&block(10), -3, &[delegate(1)]);
    fixture
        .transport
        .answer(RouteId::GetStatus, Answer::Bytes(status.clone()));
    let state = client
        .ping(&peer, Duration::from_secs(5), Some(slot(-3)), true, false)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(state.current_slot, -3);
    assert_eq!(fixture.online.0.lock().unwrap().len(), 1);
    // With our slot positive, the wire's value stands and is far outside the window.
    fixture
        .transport
        .answer(RouteId::GetStatus, Answer::Bytes(status));
    let state = client
        .ping(&peer, Duration::from_secs(5), Some(slot(3)), true, false)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(state.current_slot, 4_294_967_293);
    assert_eq!(fixture.online.0.lock().unwrap().len(), 1);
    assert!(peer.snapshot().public_keys.is_empty());
}

#[tokio::test(start_paused = true)]
async fn attestations_break_at_an_unknown_key_and_move_keys_from_forked_peers() {
    let fixture = Fixture::new();
    let client = &fixture.communicator;
    let tip = fixture.chain.last_block();
    let ping = |peer| client.ping(peer, Duration::from_secs(5), Some(slot(125)), true, false);
    // An unknown key before a delegate's ends the loop.
    let first = stored(&fixture, 1);
    let unknown = heartwood_crypto::KeyPair::from_passphrase("not a delegate").unwrap();
    let status = devnet_status(&tip, 125, &[unknown, delegate(1)]);
    fixture
        .transport
        .answer(RouteId::GetStatus, Answer::Bytes(status));
    ping(&first).await.unwrap();
    assert!(first.snapshot().public_keys.is_empty());
    assert!(fixture.online.0.lock().unwrap().is_empty());
    // A slot outside the window checks nothing.
    let status = devnet_status(&tip, 127, &[delegate(1)]);
    fixture
        .transport
        .answer(RouteId::GetStatus, Answer::Bytes(status));
    ping(&first).await.unwrap();
    assert!(first.snapshot().public_keys.is_empty());
    // Delegate 1 attested through the first peer; the first peer is then found forked, and the
    // second peer, not forked, takes the key.
    let status = devnet_status(&tip, 124, &[delegate(1)]);
    fixture
        .transport
        .answer(RouteId::GetStatus, Answer::Bytes(status.clone()));
    ping(&first).await.unwrap();
    let key = delegate(1).public_key().to_compressed();
    assert_eq!(first.snapshot().public_keys, [key]);
    first.with(|peer| {
        peer.verification = Some(Verification {
            my_height: 10,
            their_height: 12,
            highest_common_height: 8,
        });
    });
    let second = stored(&fixture, 2);
    fixture
        .transport
        .answer(RouteId::GetStatus, Answer::Bytes(status));
    ping(&second).await.unwrap();
    assert_eq!(second.snapshot().public_keys, [key]);
    assert!(first.snapshot().public_keys.is_empty());
    assert_eq!(fixture.online.0.lock().unwrap().len(), 2);
}

// ---- the verifier's requests ------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn the_verifier_asks_for_common_blocks_and_headers() {
    let fixture = Fixture::new();
    let client = &fixture.communicator;
    let peer = stored(&fixture, 1);
    let null = peer_codecs::get_common_blocks_encode_response(&GetCommonBlocksResponse::default());
    fixture
        .transport
        .answer(RouteId::GetCommonBlocks, Answer::Bytes(null));
    assert_eq!(
        VerifierLink::common_blocks(client, &peer, &ids(9), Duration::from_millis(1_234)).await,
        None
    );
    let reply = blocks_reply(&[listed(4, vec![]), listed(5, vec![])]);
    fixture
        .transport
        .answer(RouteId::GetBlocks, Answer::Bytes(reply));
    let headers: Vec<DownloadedBlock> = client.headers(&peer, 3, 2).await;
    assert_eq!(headers.len(), 2);
    assert!(headers.iter().all(|block| block.transactions.is_none()));
    let sent = fixture.transport.sent();
    assert_eq!(
        (sent[0].timeout, sent[0].limit),
        (Duration::from_millis(1_234), 100 * KIB)
    );
    assert_eq!(
        (sent[1].timeout, sent[1].limit),
        (Duration::from_secs(30), 2 * KIB)
    );
    // No valid reply: no headers.
    assert!(client.headers(&peer, 3, 2).await.is_empty());
}

#[tokio::test(start_paused = true)]
async fn downloaded_transactions_are_read_as_the_reference_reads_them() {
    let fixture = Fixture::new();
    let client = &fixture.communicator;
    let peer = stored(&fixture, 1);
    fixture
        .chain
        .idle
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let transactions = vec![transfer_bytes(1), transfer_bytes(2)];
    let reply = blocks_reply(&[listed(6, transactions.clone()), listed(7, vec![])]);
    fixture
        .transport
        .answer(RouteId::GetBlocks, Answer::Bytes(reply));
    let started = Instant::now();
    let downloaded = client
        .get_peer_blocks(&peer, BlocksRequest::after(5))
        .await
        .unwrap();
    // Idle, the reference pauses a millisecond after each transaction.
    assert_eq!(started.elapsed(), Duration::from_millis(2));
    let read = downloaded[0].transactions.as_ref().unwrap();
    let bytes: Vec<&[u8]> = read.iter().map(|transaction| transaction.bytes()).collect();
    assert_eq!(
        bytes,
        [transactions[0].as_slice(), transactions[1].as_slice()]
    );
    assert!(downloaded[0].header.transactions.is_empty());
    assert_eq!(downloaded[1].transactions, Some(vec![]));
}
