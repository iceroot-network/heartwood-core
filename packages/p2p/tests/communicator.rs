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
use heartwood_p2p::errors::{DownloadError, PingError, VerificationStopped};
use heartwood_p2p::seams::{PeerClient, VerifierLink};
use heartwood_p2p::{ChainView, DownloadedBlock, P2pOptions, SessionError, Verification};
use heartwood_p2p_wire::RouteId;
use heartwood_p2p_wire::codecs::blocks::{self, ListedBlock};
use heartwood_p2p_wire::codecs::transactions::{self, WithTransactions};
use heartwood_p2p_wire::codecs::{Amount, peer as peer_codecs};
use heartwood_p2p_wire::errors::RequestError;
use heartwood_p2p_wire::list;
use heartwood_p2p_wire::messages::Proto;
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
async fn a_getblocks_without_its_own_timeout_takes_the_configured_one() {
    // The sync's download has no timeout of its own, as the reference's; a zero is none too.
    let options = P2pOptions {
        get_blocks_timeout: Duration::from_secs(20),
        ..options()
    };
    let fixture = Fixture::with(Chain::at(10), options);
    let peer = stored(&fixture, 1);
    let timeouts = [None, Some(Duration::ZERO), Some(Duration::from_secs(2))];
    for timeout in timeouts {
        let reply = blocks_reply(&[listed(6, vec![])]);
        fixture
            .transport
            .answer(RouteId::GetBlocks, Answer::Bytes(reply));
        let request = BlocksRequest {
            timeout,
            ..BlocksRequest::after(5)
        };
        fixture
            .communicator
            .get_peer_blocks(&peer, request)
            .await
            .unwrap();
    }
    let sent: Vec<Duration> = (fixture.transport.sent().iter())
        .map(|sent| sent.timeout)
        .collect();
    let seconds = Duration::from_secs;
    assert_eq!(sent, [seconds(20), seconds(20), seconds(2)]);
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
    let limit = |n| heartwood_p2p_wire::requests::BlockLimit::new(n).unwrap();
    assert_eq!(encoded[0], ClientRequest::GetPeers);
    assert!(
        matches!(&encoded[1], ClientRequest::GetCommonBlocks { ids: sent } if sent.ids() == ids(9))
    );
    assert_eq!(
        encoded[3],
        ClientRequest::GetBlocks {
            last_block_height: non_zero(5),
            block_limit: limit(400),
            headers_only: false
        }
    );
    assert_eq!(
        encoded[4],
        ClientRequest::GetBlocks {
            last_block_height: non_zero(7),
            block_limit: limit(10),
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
    assert!(fixture.transport.sent().is_empty());
    // A limit above 400 asks for 400, the most a reference server accepts; 1 and 400 go as they
    // are.
    let asked = [1, 400, 401, 500, u32::MAX];
    for block_limit in asked {
        let reply = Answer::Bytes(blocks_reply(&[]));
        fixture.transport.answer(RouteId::GetBlocks, reply);
        let many = BlocksRequest {
            block_limit,
            ..BlocksRequest::after(3)
        };
        client.get_peer_blocks(&peer, many).await.unwrap();
    }
    let encoded = fixture.rules.encoded.lock().unwrap().clone();
    let limit = |request: &ClientRequest| match request {
        ClientRequest::GetBlocks { block_limit, .. } => Some(block_limit.get()),
        _ => None,
    };
    let wanted = [1, 400, 400, 400, 400].map(Some);
    assert_eq!(encoded.iter().map(limit).collect::<Vec<_>>(), wanted);
    assert_eq!(fixture.transport.sent()[0].limit, 20 * KIB * KIB);
    // Every request carries the node's version: a communicator cannot be made without one.
    let unversioned = heartwood_p2p::P2pOptions {
        version: String::new(),
        ..options()
    };
    let made = Fixture::try_over(
        Transport::default(),
        Rules::default(),
        Chain::at(10),
        unversioned,
    );
    assert_eq!(made.err(), Some(RequestError::EmptyVersion));
}

#[tokio::test(start_paused = true)]
async fn a_request_answered_with_nothing_fails_as_the_references_type_error() {
    let fixture = Fixture::new();
    let client = &fixture.communicator;
    let transport = &fixture.transport;
    // A hello in answer to getPeers: the reference's TypeError, so the peer is disconnected and
    // forgotten, and it is a violation.
    let first = stored(&fixture, 1);
    transport.answer(RouteId::GetPeers, Answer::Fail(SessionError::NoPayload));
    assert_eq!(client.get_peers(&first, true).await, None);
    assert!(!fixture.repository.has(first.ip()));
    assert_eq!(transport.disconnected(), [first.ip()]);
    assert_eq!(transport.errors(), [(first.ip(), "TypeError")]);
    assert_eq!(first.snapshot().violations.len(), 1);
    // On getBlocks it is only counted: the reference's getBlocks disconnects no peer for it.
    let second = stored(&fixture, 2);
    transport.answer(RouteId::GetBlocks, Answer::Fail(SessionError::NoPayload));
    let blocks = client
        .get_peer_blocks(&second, BlocksRequest::after(1))
        .await;
    assert!(blocks.unwrap().is_empty());
    assert!(fixture.repository.has(second.ip()));
    assert_eq!(second.snapshot().sequential_errors, 1);
    assert_eq!(transport.disconnected(), [first.ip()]);
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
        .ping(&peer, Duration::from_secs(5), false, true, false)
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
        .ping(&peer, Duration::from_secs(60), true, false, false)
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
async fn port_probes_end_by_their_deadline_and_ask_each_port_once() {
    use heartwood_p2p_wire::messages::peer::Plugin;
    let fixture = Fixture::new();
    let client = &fixture.communicator;
    let peer = stored(&fixture, 1);
    fixture.probe.open.lock().unwrap().insert(4003);
    fixture.probe.silent.lock().unwrap().insert(4004);
    let plugin = |port| {
        Some(Plugin {
            port: Some(port),
            ..Plugin::default()
        })
    };
    peer.with(|p| {
        p.ports.insert("dropped".to_owned(), 4000);
        p.plugins.insert("api".to_owned(), plugin(4003));
        p.plugins.insert("api-again".to_owned(), plugin(4003));
        p.plugins.insert("silent".to_owned(), plugin(4004));
        for n in 0..20 {
            p.plugins.insert(format!("other-{n}"), plugin(5000 + n));
        }
    });
    let started = Instant::now();
    let probes = tokio::time::timeout(Duration::from_secs(60), client.ping_ports(&peer));
    probes.await.expect("the probes end");
    assert_eq!(started.elapsed(), Duration::from_secs(3));
    let ports = peer.snapshot().ports;
    // The names the peer lists now, each port asked once, at most 8 distinct ports.
    assert_eq!(ports.len(), 23);
    assert!(!ports.contains_key("dropped"));
    assert_eq!(ports.get("api"), Some(&4003));
    assert_eq!(ports.get("api-again"), Some(&4003));
    assert_eq!(ports.get("silent"), Some(&-1));
    assert_eq!(ports.get("other-19"), Some(&-1));
    let probed: Vec<u16> = fixture
        .probe
        .probed
        .lock()
        .unwrap()
        .iter()
        .map(|p| p.1)
        .collect();
    assert_eq!(probed, [4003, 4004, 5000, 5001, 5002, 5003, 5004, 5005]);
}

#[tokio::test(start_paused = true)]
async fn a_ping_skips_ignored_and_recently_pinged_peers() {
    let fixture = Fixture::new();
    let client = &fixture.communicator;
    let status = devnet_status(&block(10), 125, &[]);
    let ignored = stored(&fixture, 1);
    ignored.with(|peer| (1..=3).for_each(|i| peer.add_infraction(START_MS - i)));
    let ping = |peer, force| client.ping(peer, Duration::from_secs(5), false, force, false);
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
    let ping = |timeout| client.ping(&peer, timeout, false, true, false);
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
    *fixture.verifier.full.lock().unwrap() = Err(VerificationStopped::Deadline);
    transport.answer(RouteId::GetStatus, Answer::Bytes(status.clone()));
    let stopped = PingError::Stopped(VerificationStopped::Deadline);
    assert_eq!(ping(five).await, Err(stopped));
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
    let fast = |peer| client.ping(peer, Duration::from_millis(2_500), false, true, true);
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
    // The node could not read its chain: the ping fails, and the last result stays.
    let last = peer.snapshot().fast_verification;
    assert!(last.is_some());
    *fixture.verifier.fast.lock().unwrap() = Err(VerificationStopped::Lookup);
    fixture
        .transport
        .answer(RouteId::GetStatus, Answer::Bytes(status.clone()));
    let stopped = PingError::Stopped(VerificationStopped::Lookup);
    assert_eq!(fast(&peer).await, Err(stopped));
    assert_eq!(peer.snapshot().fast_verification, last);
    *fixture.verifier.fast.lock().unwrap() = Ok(None);
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
    at_slot(&fixture.clock, -3);
    let state = client
        .ping(&peer, Duration::from_secs(5), true, true, false)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(state.current_slot, -3);
    assert_eq!(fixture.online.0.lock().unwrap().len(), 1);
    // With our slot positive, the wire's value stands and is far outside the window.
    fixture
        .transport
        .answer(RouteId::GetStatus, Answer::Bytes(status));
    at_slot(&fixture.clock, 3);
    let state = client
        .ping(&peer, Duration::from_secs(5), true, true, false)
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
    let ping = |peer| client.ping(peer, Duration::from_secs(5), true, true, false);
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

#[tokio::test(start_paused = true)]
async fn the_attestations_use_our_slot_after_the_verification() {
    // The verification takes 6 s, and our slot moves from 125 to 126 at 4 s.
    let fixture = Fixture::new();
    let client = &fixture.communicator;
    let tip = fixture.chain.last_block();
    *fixture.verifier.delay.lock().unwrap() = Duration::from_secs(6);
    let key = delegate(1).public_key().to_compressed();
    // Read when the ping started, slot 124 would be in the window and 127 outside it.
    for (last, theirs, attributed) in [(1, 124, false), (2, 127, true)] {
        at_slot(&fixture.clock, 125);
        let clock = Arc::clone(&fixture.clock);
        let tick = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(4)).await;
            at_slot(&clock, 126);
        });
        let peer = stored(&fixture, last);
        let status = devnet_status(&tip, theirs, &[delegate(1)]);
        fixture
            .transport
            .answer(RouteId::GetStatus, Answer::Bytes(status));
        let ping = client.ping(&peer, Duration::from_secs(10), true, true, false);
        assert!(ping.await.unwrap().is_some());
        tick.await.unwrap();
        let holds = peer.snapshot().public_keys.contains(&key);
        assert_eq!(holds, attributed, "their slot {theirs}");
    }
    assert_eq!(fixture.online.0.lock().unwrap().len(), 1);
}

#[test]
fn pings_on_two_threads_attest_one_at_a_time() {
    // Two relays of one forger, both at our last block: the ping that attests second takes the
    // key from the other, so exactly one holds it, whichever thread ran first.
    let runtime = || {
        let mut builder = tokio::runtime::Builder::new_current_thread();
        builder.enable_time().build().unwrap()
    };
    let key = delegate(1).public_key().to_compressed();
    for round in 0..200 {
        let fixture = {
            let main = runtime();
            let _entered = main.enter();
            Fixture::new()
        };
        let status = devnet_status(&fixture.chain.last_block(), 125, &[delegate(1)]);
        let peers = [stored(&fixture, 1), stored(&fixture, 2)];
        let start = std::sync::Barrier::new(peers.len());
        std::thread::scope(|scope| {
            for peer in &peers {
                let answer = Answer::Bytes(status.clone());
                fixture.transport.answer(RouteId::GetStatus, answer);
                let (fixture, start) = (&fixture, &start);
                scope.spawn(move || {
                    let client = &fixture.communicator;
                    let ping = client.ping(peer, Duration::from_secs(5), true, true, false);
                    start.wait();
                    assert!(runtime().block_on(ping).unwrap().is_some());
                });
            }
        });
        let holders = peers.iter().filter(|peer| {
            let keys = peer.snapshot().public_keys;
            keys.contains(&key)
        });
        assert_eq!(holders.count(), 1, "round {round}");
        assert_eq!(fixture.online.0.lock().unwrap().len(), 2, "round {round}");
    }
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

#[tokio::test(start_paused = true)]
async fn an_idle_download_pauses_half_a_second_at_most_and_keeps_no_raw_list() {
    let chain = Chain::with(10, milestones_with_max_transactions(1_000, 300));
    let fixture = Fixture::with(chain, options());
    let client = &fixture.communicator;
    let peer = stored(&fixture, 1);
    fixture
        .chain
        .idle
        .store(true, std::sync::atomic::Ordering::SeqCst);
    // 4 blocks of 250 transactions: the reference would pause for a second.
    let transfer = transfer_bytes(1);
    let blocks: Vec<ListedBlock> = (6..10)
        .map(|height| listed(height, vec![transfer.clone(); 250]))
        .collect();
    fixture
        .transport
        .answer(RouteId::GetBlocks, Answer::Bytes(blocks_reply(&blocks)));
    let started = Instant::now();
    let downloaded = client
        .get_peer_blocks(&peer, BlocksRequest::after(5))
        .await
        .unwrap();
    assert_eq!(started.elapsed(), Duration::from_millis(500));
    assert_eq!(downloaded.len(), 4);
    for block in &downloaded {
        assert_eq!(block.transactions.as_ref().map(Vec::len), Some(250));
        // The reply's raw list is not kept beside the transactions read from it.
        assert_eq!(block.header.header.transactions.as_deref(), Some(&[][..]));
    }
}

#[tokio::test(start_paused = true)]
async fn a_block_with_more_transactions_than_any_milestone_allows_fails_the_download() {
    let fixture = Fixture::new();
    let client = &fixture.communicator;
    let peer = stored(&fixture, 1);
    let transfer = transfer_bytes(1);
    // The devnet's blocks hold at most 150 transactions; the reference reads 501 per block.
    let blocks = [listed(6, vec![transfer.clone(); 151])];
    fixture
        .transport
        .answer(RouteId::GetBlocks, Answer::Bytes(blocks_reply(&blocks)));
    let failed = client.get_peer_blocks(&peer, BlocksRequest::after(5)).await;
    assert_eq!(
        failed,
        Err(DownloadError::TooManyTransactions {
            height: 6,
            count: 151,
            most: 150
        })
    );
    assert_eq!(peer.snapshot().violations.len(), 1);
    // As many as a milestone allows are read.
    let peer = stored(&fixture, 2);
    let blocks = [listed(6, vec![transfer; 150])];
    fixture
        .transport
        .answer(RouteId::GetBlocks, Answer::Bytes(blocks_reply(&blocks)));
    let downloaded = client.get_peer_blocks(&peer, BlocksRequest::after(5)).await;
    assert_eq!(downloaded.map(|blocks| blocks.len()), Ok(1));
    assert!(peer.snapshot().violations.is_empty());
}

#[tokio::test(start_paused = true)]
async fn an_amount_beyond_128_bits_is_read_before_it_is_refused() {
    let fixture = Fixture::new();
    let peer = stored(&fixture, 1);
    peer.with(|peer| peer.sequential_errors = 2);
    let header = BlockHeader {
        height: Some(6),
        total_amount: Some(format!("1{}", "0".repeat(40))),
        total_fee: Some("0".into()),
        reward: Some("0".into()),
        transactions: Some(list::encode::<Vec<u8>>(&[])),
        ..BlockHeader::default()
    };
    let reply = list::encode(&[header.encode()]);
    let late = Answer::After(Duration::from_millis(40), Box::new(Answer::Bytes(reply)));
    fixture.transport.answer(RouteId::GetBlocks, late);
    let request = BlocksRequest::after(5);
    let downloaded = fixture.communicator.get_peer_blocks(&peer, request).await;
    assert!(downloaded.unwrap().is_empty());
    // The reference reads it, resets the counter and records the latency, then its reply schema
    // refuses it: counted once from zero, so the peer stays.
    let snapshot = peer.snapshot();
    assert_eq!(
        (snapshot.sequential_errors, snapshot.latency),
        (1, Some(40))
    );
    assert_eq!(snapshot.violations.len(), 1);
    assert!(fixture.repository.has(peer.ip()));
    let refused = (peer.ip(), "CoreValidationError");
    assert_eq!(fixture.transport.errors(), [refused]);
}

// ---- integration: the real peer rules, and the connector against the mock peer ----------------

/// The communicator over the scripted transport with the real peer rules.
fn with_real_rules(chain: Chain) -> Fixture<Transport, heartwood_p2p::utils::rules::Rules> {
    let rules = heartwood_p2p::utils::rules::Rules::new(100, 25);
    Fixture::over(Transport::default(), rules, chain, options())
}

/// The headers every request carries: our version and port.
fn our_headers() -> Option<heartwood_p2p_wire::messages::shared::Headers> {
    Some(heartwood_p2p_wire::messages::shared::Headers {
        version: Some("4.3.1".to_owned()),
        port: Some(4002),
        ..Default::default()
    })
}

#[tokio::test(start_paused = true)]
#[ignore = "integration"]
async fn each_route_writes_the_reference_clients_request() {
    let fixture = with_real_rules(Chain::at(10));
    let client = &fixture.communicator;
    let peer = peer(1);
    fixture.repository.set(peer.clone());
    let status = devnet_status(&block(10), 125, &[]);
    fixture
        .transport
        .answer(RouteId::GetStatus, Answer::Bytes(status));
    client
        .ping(&peer, Duration::from_secs(5), false, true, false)
        .await
        .ok();
    client.get_peers(&peer, true).await;
    client
        .has_common_blocks(&peer, &ids(2), Duration::from_secs(1))
        .await;
    client
        .get_peer_blocks(&peer, BlocksRequest::after(5))
        .await
        .ok();
    let headers = BlocksRequest {
        block_limit: 3,
        headers_only: true,
        ..BlocksRequest::after(6)
    };
    client.get_peer_blocks(&peer, headers).await.ok();
    client.get_unconfirmed_transactions(&peer, &[]).await;
    client.post_block(&peer, &block(11)).await;
    client.post_transactions(&peer, Arc::new(vec![vec![1, 2]]));
    tokio::time::sleep(Duration::from_secs(30)).await;

    let payload = |route| fixture.transport.sent_on(route)[0].payload.clone();
    let status = peer_codecs::get_status_decode_request(&payload(RouteId::GetStatus)).unwrap();
    assert_eq!(status.headers, our_headers());
    let peers = peer_codecs::get_peers_decode_request(&payload(RouteId::GetPeers)).unwrap();
    assert_eq!(peers.headers, our_headers());
    let common =
        peer_codecs::get_common_blocks_decode_request(&payload(RouteId::GetCommonBlocks)).unwrap();
    let expected_ids: Vec<String> = ids(2).iter().map(BlockId::to_hex).collect();
    assert_eq!((common.ids, common.headers), (expected_ids, our_headers()));
    let sent = fixture.transport.sent_on(RouteId::GetBlocks);
    let full = blocks::get_blocks_decode_request(&sent[0].payload).unwrap();
    assert_eq!(
        (
            full.last_block_height,
            full.block_limit,
            full.headers_only,
            full.serialised
        ),
        (Some(5), Some(400), None, Some(true))
    );
    assert_eq!(full.headers, our_headers());
    let only = blocks::get_blocks_decode_request(&sent[1].payload).unwrap();
    assert_eq!(
        (only.last_block_height, only.block_limit, only.headers_only),
        (Some(6), Some(3), Some(true))
    );
    let route = RouteId::GetUnconfirmedTransactions;
    let pool = transactions::get_unconfirmed_transactions_decode_request(&payload(route)).unwrap();
    // `exclude` is written even when empty, and `countOnly` never.
    assert_eq!(pool.request.count_only, None);
    assert_eq!(pool.request.exclude, Some(vec![]));
    assert_eq!(pool.request.headers, our_headers());
    let posted = blocks::post_block_decode_request(&payload(RouteId::PostBlock)).unwrap();
    let bytes = heartwood_crypto::blocks::serialiser::serialise(&block(11));
    assert_eq!(posted.block.map(|block| block.len()), Some(bytes.len()));
    let route = RouteId::PostTransactions;
    let batch = transactions::post_transactions_decode_request(&payload(route)).unwrap();
    assert_eq!(batch.transactions, [vec![1, 2]]);
    assert_eq!(batch.message.headers, our_headers());
}

#[tokio::test(start_paused = true)]
#[ignore = "integration"]
async fn the_real_limiter_spaces_requests_to_one_peer() {
    let fixture = with_real_rules(Chain::at(10));
    let client = &fixture.communicator;
    let (first, second) = (peer(1), peer(2));
    let started = Instant::now();
    // getPeers: 1 a second; getBlocks: 1 in 2 s; another peer has limits of its own.
    for _ in 0..2 {
        fixture
            .transport
            .answer(RouteId::GetPeers, Answer::Bytes(peers_reply(&[])));
    }
    client.get_peers(&first, true).await;
    client.get_peers(&first, true).await;
    for _ in 0..3 {
        fixture
            .transport
            .answer(RouteId::GetBlocks, Answer::Bytes(blocks_reply(&[])));
    }
    client
        .get_peer_blocks(&first, BlocksRequest::after(1))
        .await
        .unwrap();
    client
        .get_peer_blocks(&first, BlocksRequest::after(1))
        .await
        .unwrap();
    client
        .get_peer_blocks(&second, BlocksRequest::after(1))
        .await
        .unwrap();
    let at: Vec<Duration> = fixture
        .transport
        .sent()
        .iter()
        .map(|sent| sent.at - started)
        .collect();
    let seconds = Duration::from_secs;
    assert_eq!(
        at,
        [seconds(0), seconds(1), seconds(1), seconds(3), seconds(3)]
    );
    assert!(client.would_throttle(first.ip(), RouteId::GetBlocks));
    assert!(!client.would_throttle(first.ip(), RouteId::GetStatus));
}

#[tokio::test(start_paused = true)]
#[ignore = "integration"]
async fn the_real_reply_check_discards_an_oversize_unconfirmed_reply() {
    let fixture = with_real_rules(Chain::with(
        10,
        milestones_with_max_transactions(1_000, 300),
    ));
    let client = &fixture.communicator;
    let peer = peer(1);
    fixture.repository.set(peer.clone());
    let route = RouteId::GetUnconfirmedTransactions;
    for count in [151, 150, 301] {
        fixture
            .transport
            .answer(route, Answer::Bytes(unconfirmed_reply(count)));
    }
    assert!(
        client
            .get_unconfirmed_transactions(&peer, &[])
            .await
            .is_empty()
    );
    assert_eq!(peer.snapshot().sequential_errors, 0);
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(
        client.get_unconfirmed_transactions(&peer, &[]).await.len(),
        150
    );
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert!(
        client
            .get_unconfirmed_transactions(&peer, &[])
            .await
            .is_empty()
    );
    assert_eq!(peer.snapshot().sequential_errors, 1);
    assert_eq!(peer.snapshot().violations.len(), 1);
}

#[cfg(feature = "difftest")]
mod over_the_mock_peer {
    //! The communicator over the connector, against the mock peer, in real time (the sockets are
    //! real, so tokio's clock is not paused).

    use heartwood_p2p::Connector;
    use heartwood_p2p::testing::{MockFrame, MockPeer, Reply, Script};
    use heartwood_p2p::utils::rules::Rules as RealRules;

    use super::*;

    async fn start(last: u8, script: Script) -> (MockPeer, heartwood_p2p::SharedPeer) {
        let mock = MockPeer::scripted(IpAddr::from([127, 0, 0, last]), script)
            .await
            .unwrap();
        let address = mock.address();
        let peer =
            heartwood_p2p::SharedPeer::new(heartwood_p2p::Peer::new(address.ip(), address.port()));
        (mock, peer)
    }

    fn fixture() -> Fixture<Connector, RealRules> {
        Fixture::over(
            Connector::new(),
            RealRules::new(100, 25),
            Chain::at(10),
            options(),
        )
    }

    #[tokio::test]
    #[ignore = "integration"]
    async fn requests_and_replies_travel_over_nes() {
        let status = devnet_status(&block(10), 125, &[delegate(1)]);
        let script = Script::new()
            .on(RouteId::GetStatus, Reply::Payload(status))
            .on(
                RouteId::GetPeers,
                Reply::Payload(peers_reply(&[("192.0.2.9", 4002)])),
            )
            .on(
                RouteId::PostBlock,
                Reply::Payload(post_block_reply(Some(42))),
            );
        let (mock, peer) = start(11, script).await;
        let fixture = fixture();
        fixture.repository.set(peer.clone());
        let client = &fixture.communicator;
        let state = client
            .ping(&peer, Duration::from_secs(5), true, true, false)
            .await;
        assert_eq!(state.unwrap().unwrap().height, 10);
        assert_eq!(
            peer.snapshot().public_keys,
            [delegate(1).public_key().to_compressed()]
        );
        let peers = client.get_peers(&peer, true).await.unwrap();
        assert_eq!(peers[0].ip, "192.0.2.9");
        client.post_block(&peer, &block(11)).await;
        assert_eq!(peer.snapshot().height(), Some(42));
        let routes: Vec<RouteId> = mock.requests().iter().map(|(route, _)| *route).collect();
        assert_eq!(
            routes,
            [RouteId::GetStatus, RouteId::GetPeers, RouteId::PostBlock]
        );
        // One session carried every request.
        assert_eq!(mock.connections(), 1);
        let (_, payload) = &mock.requests()[1];
        let request = peer_codecs::get_peers_decode_request(payload).unwrap();
        assert_eq!(request.headers, our_headers());
    }

    #[tokio::test]
    #[ignore = "integration"]
    async fn failures_over_nes_count_and_disconnect_as_the_reference_does() {
        let error = || Reply::Error {
            status: 500,
            payload: b"boom".to_vec(),
        };
        // Three server errors in a row: class Error, counted, and the third forgets the peer.
        let script = Script::new()
            .on(RouteId::GetPeers, error())
            .on(RouteId::GetPeers, error())
            .on(RouteId::GetPeers, error());
        let (_mock, peer) = start(12, script).await;
        let fixture = fixture();
        fixture.repository.set(peer.clone());
        for _ in 0..2 {
            assert_eq!(fixture.communicator.get_peers(&peer, true).await, None);
            assert!(fixture.repository.has(peer.ip()));
        }
        assert_eq!(fixture.communicator.get_peers(&peer, true).await, None);
        assert!(!fixture.repository.has(peer.ip()));
        assert_eq!(fixture.events.all(), [Event::PeerDisconnect]);
        assert!(peer.snapshot().violations.is_empty());
        // A reply the codec refuses with a RangeError: disconnected at once, and a violation.
        let script = Script::new().on(RouteId::GetPeers, Reply::Payload(truncated()));
        let (_mock, other) = start(13, script).await;
        fixture.repository.set(other.clone());
        assert_eq!(fixture.communicator.get_peers(&other, true).await, None);
        assert!(!fixture.repository.has(other.ip()));
        assert_eq!(other.snapshot().violations.len(), 1);
        // A text frame breaks the nes protocol: the connector forgets the session and the request
        // fails with class Error, so the peer is only counted, and it is a violation.
        let text = Reply::Frames(vec![MockFrame::Text("{}".into())]);
        let (_mock, third) = start(15, Script::new().on(RouteId::GetPeers, text)).await;
        fixture.repository.set(third.clone());
        assert_eq!(fixture.communicator.get_peers(&third, true).await, None);
        assert!(fixture.repository.has(third.ip()));
        assert_eq!(third.snapshot().sequential_errors, 1);
        assert_eq!(third.snapshot().violations.len(), 1);
    }

    #[tokio::test]
    #[ignore = "integration"]
    async fn a_silent_peer_times_out_on_the_routes_timeout() {
        let (_mock, peer) = start(14, Script::new().on(RouteId::GetPeers, Reply::Silence)).await;
        let fixture = fixture();
        let started = Instant::now();
        assert_eq!(fixture.communicator.get_peers(&peer, true).await, None);
        let waited = started.elapsed();
        assert!(
            waited >= Duration::from_secs(5) && waited < Duration::from_secs(7),
            "{waited:?}"
        );
        assert_eq!(peer.snapshot().sequential_errors, 1);
    }

    #[tokio::test(start_paused = true)]
    #[ignore = "integration"]
    async fn an_honest_late_reply_while_another_request_waits_is_no_violation() {
        // getCommonBlocks times out after 1 s and its reply comes at 1.5 s, while getPeers waits.
        // As in the reference, the session is dropped and the waiting request fails; the peer is
        // counted, records no violation, and stays a source.
        let late = |at: u64, reply| Reply::After(Duration::from_millis(at), Box::new(reply));
        let script = Script::new()
            .on(
                RouteId::GetCommonBlocks,
                late(1_500, Reply::Payload(vec![])),
            )
            .on(
                RouteId::GetPeers,
                late(3_000, Reply::Payload(peers_reply(&[]))),
            )
            .on(RouteId::GetBlocks, Reply::Payload(blocks_reply(&[])));
        let mock = MockPeer::scripted(IpAddr::from([127, 0, 0, 16]), script)
            .await
            .unwrap();
        let (ip, port) = (mock.address().ip(), mock.address().port());
        let peer = heartwood_p2p::SharedPeer::new(heartwood_p2p::Peer::new(ip, port));
        let fixture = Fixture::over(
            Connector::with_dial(MockPeer::dial(&[&mock])),
            RealRules::new(100, 25),
            Chain::at(10),
            options(),
        );
        fixture.repository.set(peer.clone());
        let client = &fixture.communicator;
        let started = Instant::now();
        let asked = ids(1);
        let common = client.has_common_blocks(&peer, &asked, Duration::from_secs(1));
        assert_eq!(common.await, None);
        assert_eq!(client.get_peers(&peer, true).await, None);
        assert_eq!(started.elapsed(), Duration::from_millis(1_500));
        let snapshot = peer.snapshot();
        assert!(snapshot.violations.is_empty());
        assert_eq!(snapshot.sequential_errors, 2);
        assert!(fixture.repository.has(ip));
        // Still a source: the next getBlocks is sent, on a new session after the 10 s pause.
        let blocks = client.get_peer_blocks(&peer, BlocksRequest::after(5)).await;
        assert!(blocks.unwrap().is_empty());
        let routes: Vec<RouteId> = mock.requests().iter().map(|(route, _)| *route).collect();
        let sent = [
            RouteId::GetCommonBlocks,
            RouteId::GetPeers,
            RouteId::GetBlocks,
        ];
        assert_eq!(routes, sent);
        assert_eq!(mock.connections(), 2);
        assert_eq!(peer.snapshot().sequential_errors, 0);
    }
}
