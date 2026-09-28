//! The protocol violations: which failures record one, what a violation changes (the peer is no
//! source of blocks, transactions or peers, counts toward the ping's ignore rule and is not
//! accepted again within its window), and what the reference's own infractions do not change (an
//! honest slow peer with three cleanse infractions stays a source).

mod peer_fakes;

use std::sync::Arc;
use std::time::Duration;

use heartwood_p2p::communicator::BlocksRequest;
use heartwood_p2p::errors::{DownloadError, ProtocolError};
use heartwood_p2p::seams::{PeerAcceptor, PeerClient};
use heartwood_p2p::{AcceptOptions, PeerProcessor, ProcessorParts, SessionError, SharedPeer};
use heartwood_p2p_wire::RouteId;
use heartwood_p2p_wire::codecs::blocks::{self, ListedBlock};
use heartwood_p2p_wire::codecs::peer as peer_codecs;
use heartwood_p2p_wire::codecs::transactions;
use heartwood_p2p_wire::messages::blocks::{BlockHeader, PostBlockResponse};
use heartwood_p2p_wire::replies::PeerAddress;
use peer_fakes::*;

fn stored(fixture: &Fixture, last: u8) -> SharedPeer {
    let peer = peer(last);
    fixture.repository.set(peer.clone());
    peer
}

fn one_block(transactions: Vec<Vec<u8>>) -> Vec<u8> {
    blocks::get_blocks_encode_response(&[ListedBlock {
        header: BlockHeader {
            height: Some(6),
            ..BlockHeader::default()
        },
        transactions,
        ..ListedBlock::default()
    }])
}

/// Script one answer of each source route.
fn answer_sources(fixture: &Fixture) {
    let transport = &fixture.transport;
    transport.answer(
        RouteId::GetPeers,
        Answer::Bytes(peer_codecs::get_peers_encode_response(&[])),
    );
    transport.answer(RouteId::GetBlocks, Answer::Bytes(one_block(vec![])));
    let pool = transactions::get_unconfirmed_transactions_encode_response(&Default::default());
    transport.answer(RouteId::GetUnconfirmedTransactions, Answer::Bytes(pool));
}

/// Ask `peer` for peers, blocks and transactions; how many of the requests were sent.
async fn ask_as_source(fixture: &Fixture, peer: &SharedPeer) -> usize {
    let before = fixture.transport.sent().len();
    let client = &fixture.communicator;
    client.get_peers(peer, true).await;
    client
        .get_peer_blocks(peer, BlocksRequest::after(5))
        .await
        .unwrap();
    client.get_unconfirmed_transactions(peer, &[]).await;
    fixture.transport.sent().len() - before
}

#[tokio::test(start_paused = true)]
async fn a_violation_makes_the_peer_no_source() {
    let fixture = Fixture::new();
    let peer = stored(&fixture, 1);
    // A getPeers reply that fails its schema.
    fixture.rules.refuse(RouteId::GetPeers);
    fixture.transport.answer(
        RouteId::GetPeers,
        Answer::Bytes(peer_codecs::get_peers_encode_response(&[])),
    );
    assert_eq!(fixture.communicator.get_peers(&peer, true).await, None);
    assert_eq!(peer.snapshot().violations, [START_MS]);
    answer_sources(&fixture);
    assert_eq!(ask_as_source(&fixture, &peer).await, 0);
    assert_eq!(fixture.communicator.get_peers(&peer, false).await, None);
    // It is still pinged (one violation is not three faults) and still told of blocks.
    let status = devnet_status(&block(10), 125, &[]);
    fixture
        .transport
        .answer(RouteId::GetStatus, Answer::Bytes(status));
    let pinged = fixture
        .communicator
        .ping(&peer, Duration::from_secs(5), false, true, false)
        .await;
    assert!(pinged.unwrap().is_some());
    let reply = blocks::post_block_encode_response(&PostBlockResponse::default());
    fixture
        .transport
        .answer(RouteId::PostBlock, Answer::Bytes(reply));
    fixture.communicator.post_block(&peer, &block(11)).await;
    assert_eq!(fixture.transport.sent_on(RouteId::PostBlock).len(), 1);
    // After the 600 s window it is a source again.
    fixture.rules.refuse.lock().unwrap().clear();
    fixture.clock.advance(600_001);
    assert_eq!(ask_as_source(&fixture, &peer).await, 3);
}

#[tokio::test(start_paused = true)]
async fn an_honest_slow_peer_with_three_cleanse_infractions_stays_a_source() {
    let fixture = Fixture::new();
    let slow = stored(&fixture, 1);
    slow.with(|peer| (1..=3).for_each(|i| peer.add_infraction(START_MS - i * 1_000)));
    // The reference's ignore rule: the ping skips it.
    let ping = fixture
        .communicator
        .ping(&slow, Duration::from_secs(5), false, true, false);
    assert_eq!(ping.await, Ok(None));
    assert!(fixture.transport.sent_on(RouteId::GetStatus).is_empty());
    // But it is asked for peers, blocks and transactions as before.
    answer_sources(&fixture);
    assert_eq!(ask_as_source(&fixture, &slow).await, 3);
    assert!(slow.snapshot().violations.is_empty());
}

#[tokio::test(start_paused = true)]
async fn infractions_and_violations_count_together_toward_the_ignore_rule() {
    let fixture = Fixture::new();
    let peer = stored(&fixture, 1);
    peer.with(|peer| {
        peer.add_infraction(START_MS - 2_000);
        peer.add_infraction(START_MS - 1_000);
        peer.add_violation(START_MS);
    });
    let ping = fixture
        .communicator
        .ping(&peer, Duration::from_secs(5), false, true, false);
    assert_eq!(ping.await, Ok(None));
    assert!(fixture.transport.sent().is_empty());
}

#[tokio::test(start_paused = true)]
async fn only_what_no_honest_peer_does_is_a_violation() {
    let fixture = Fixture::new();
    let client = &fixture.communicator;
    let transport = &fixture.transport;
    let violations = |peer: &SharedPeer| peer.snapshot().violations.len();
    // Timeouts, disconnects, server errors, heartbeat timeouts, a response after our timeout
    // and pings bunched by a stall: honest peers have them.
    let honest = stored(&fixture, 1);
    let server = SessionError::Server {
        status: 500,
        payload: "boom".into(),
    };
    let quiet = [
        ProtocolError::HeartbeatTimeout,
        ProtocolError::LateResponse,
        ProtocolError::PingFlood,
    ];
    let failures = [SessionError::Disconnect, server];
    for error in failures
        .into_iter()
        .chain(quiet.map(SessionError::Protocol))
    {
        transport.answer(RouteId::GetPeers, Answer::Fail(error));
    }
    for _ in 0..6 {
        client.get_peers(&honest, true).await;
    }
    assert_eq!(violations(&honest), 0);
    // Any other nes protocol error is one; so is a reply the codec refuses.
    let texting = stored(&fixture, 2);
    let text = SessionError::Protocol(ProtocolError::TextFrame);
    transport.answer(RouteId::GetPeers, Answer::Fail(text));
    client.get_peers(&texting, true).await;
    assert_eq!(violations(&texting), 1);
    let garbled = stored(&fixture, 3);
    transport.answer(RouteId::GetPeers, Answer::Bytes(vec![0x7a, 0x05]));
    client.get_peers(&garbled, true).await;
    assert_eq!(violations(&garbled), 1);
    // A downloaded transaction the reference's fromBytesUnsafe refuses fails the download and is
    // one, without counting as a failed request.
    let bad = stored(&fixture, 4);
    transport.answer(
        RouteId::GetBlocks,
        Answer::Bytes(one_block(vec![vec![0xff; 3]])),
    );
    let download = client.get_peer_blocks(&bad, BlocksRequest::after(5)).await;
    assert!(matches!(
        download,
        Err(DownloadError::Transaction { height: 6, .. })
    ));
    assert_eq!(violations(&bad), 1);
    assert_eq!(bad.snapshot().sequential_errors, 0);
    assert!(fixture.repository.has(bad.ip()));
    // A memo that is not UTF-8 fails the download too, but the reference accepts it, so honest
    // peers serve such a block: no violation.
    let memo = stored(&fixture, 5);
    let block = one_block(vec![transfer_with_non_utf8_memo()]);
    transport.answer(RouteId::GetBlocks, Answer::Bytes(block));
    let download = client.get_peer_blocks(&memo, BlocksRequest::after(5)).await;
    assert!(matches!(
        download,
        Err(DownloadError::Transaction {
            source: heartwood_crypto::errors::DecodeError::InvalidMemo,
            ..
        })
    ));
    assert_eq!(violations(&memo), 0);
}

#[tokio::test(start_paused = true)]
async fn a_new_peer_found_violating_is_not_accepted_again_within_the_window() {
    let fixture = Fixture::new();
    let client = Arc::new(fixture.communicator);
    let processor = PeerProcessor::new(ProcessorParts {
        client: Arc::clone(&client),
        rules: Arc::clone(&fixture.rules),
        repository: Arc::clone(&fixture.repository),
        events: fixture.events.clone(),
        options: Arc::clone(&fixture.options),
        interfaces: Vec::new(),
        clock: fixture.clock.clone(),
    });
    let candidate = PeerAddress {
        ip: "192.0.2.1".into(),
        port: 4002,
    };
    // Its getStatus reply does not decode: the ping fails and the violation is kept.
    fixture
        .transport
        .answer(RouteId::GetStatus, Answer::Bytes(vec![0x7a, 0x05]));
    processor
        .validate_and_accept(candidate.clone(), AcceptOptions::default())
        .await;
    let ip = "192.0.2.1".parse().unwrap();
    assert!(!fixture.repository.has(ip));
    // (Disconnected for the decoder's RangeError, and again for the failed acceptance.)
    assert_eq!(fixture.transport.disconnected(), [ip, ip]);
    assert!(fixture.repository.has_violation(ip, START_MS));
    assert!(!processor.validate_peer_ip(&mut candidate.clone(), AcceptOptions::default()));
    // Once the window has passed, a good status gets it accepted.
    fixture.clock.advance(600_001);
    let status = devnet_status(&block(10), 125, &[]);
    fixture
        .transport
        .answer(RouteId::GetStatus, Answer::Bytes(status));
    processor
        .validate_and_accept(candidate, AcceptOptions::default())
        .await;
    assert!(fixture.repository.has(ip));
}
