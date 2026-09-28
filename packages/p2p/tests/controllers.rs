//! The controllers over fakes of the node's services and the peer side: every public and internal
//! route's answer and what it asks of the node, `emitEvent`'s four names, and the network state
//! with the delegates on this node.
//!
//! The case with the node's communicator against the whole server (its transport and these
//! controllers) is an integration test, run once the server's transport is written.

#![cfg(feature = "node")]

mod node_fakes;
mod peer_fakes;

use std::net::IpAddr;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use heartwood_crypto::blocks::factory::{self, BlockDraft};
use heartwood_crypto::{Aux, Block, KeyPair, TransactionId};
use heartwood_kernel::{ChainLink, Clock, ManualClock};
use heartwood_p2p::Peer;
use heartwood_p2p::node_services::ForgerEvent;
use heartwood_p2p::server::controllers::remote_access_warnings;
use heartwood_p2p::server::seams::{RouteHandlers, ServerRefusal};
use heartwood_p2p::view::ChainView;
use heartwood_p2p::{P2pOptions, ServerOptions};
use heartwood_p2p_wire::codecs::Amount;
use heartwood_p2p_wire::codecs::blocks::{BlockListing, get_blocks_decode_response};
use heartwood_p2p_wire::codecs::transactions::{
    UnconfirmedRequestInput, get_unconfirmed_transactions_decode_response,
    get_unconfirmed_transactions_encode_request, post_transactions_decode_response,
};
use heartwood_p2p_wire::internal::{
    self, EmitEvent, NetworkState, NetworkStateRequest, NetworkStateStatus, SlotNumber,
    SlotNumberRequest,
};
use heartwood_p2p_wire::messages::Proto;
use heartwood_p2p_wire::messages::blocks::{GetBlocksRequest, PostBlockRequest, PostBlockResponse};
use heartwood_p2p_wire::messages::peer::{
    GetCommonBlocksRequest, GetCommonBlocksResponse, GetPeersRequest, GetPeersResponse,
};
use heartwood_p2p_wire::messages::transactions::PostTransactionsRequest;
use heartwood_p2p_wire::{RouteId, list};
use node_fakes::{Node, Server, forger, request, round};
use peer_fakes::{Chain, DEVNET, START_MS, block, delegate, milestones_with_max_transactions};
use serde_json::json;

fn peer_ip(last: u8) -> IpAddr {
    IpAddr::from([192, 0, 2, last])
}

fn server(node: Node) -> Server {
    Server::new(node, Arc::new(ManualClock::new(START_MS)))
}

async fn call(
    server: &Server,
    route: RouteId,
    payload: Vec<u8>,
    ip: IpAddr,
) -> Result<Vec<u8>, ServerRefusal> {
    let from_forger = ip == forger();
    let request = request(payload, ip, from_forger);
    server.controllers.handle(route, request).await
}

/// The block after `previous` by delegate `signer`, a slot later, with `transactions`.
fn next_block(previous: &Block, signer: &KeyPair, reward: u64) -> Block {
    let header = previous.header();
    let draft = BlockDraft {
        version: 0,
        timestamp: header.timestamp + 8,
        height: header.height + 1,
        previous_block: *previous.id().as_bytes(),
        number_of_transactions: 0,
        total_amount: 0,
        total_fee: 0,
        reward,
        payload_length: 0,
        payload_hash: heartwood_crypto::crypto::hash::sha256(&[]),
    };
    factory::make(draft, Vec::new(), signer, Aux::random(), &DEVNET.milestones).unwrap()
}

fn post_block(block: &Block) -> Vec<u8> {
    PostBlockRequest {
        block: Some(block.encode()),
        ..PostBlockRequest::default()
    }
    .encode()
}

fn post_answer(bytes: &[u8]) -> (bool, u32) {
    let answer = PostBlockResponse::decode(bytes).unwrap();
    (answer.status.unwrap(), answer.height.unwrap())
}

// ---- the peer routes -------------------------------------------------------------------------

#[tokio::test]
async fn get_peers_lists_the_other_peers_by_latency_at_most_two_thousand() {
    let server = server(Node::at(10));
    let mut peers = Vec::new();
    for n in 0..2_005_u32 {
        let mut peer = Peer::new(IpAddr::from((0x0a00_0000 + n).to_be_bytes()), 4002);
        peer.latency = Some(5_000 - n);
        peers.push(peer);
    }
    let mut caller = Peer::new(peer_ip(1), 4002);
    caller.latency = Some(0);
    peers.push(caller);
    *server.peers.peers.lock().unwrap() = peers;
    let payload = GetPeersRequest::default().encode();
    let answer = call(&server, RouteId::GetPeers, payload, peer_ip(1))
        .await
        .unwrap();
    let listed = GetPeersResponse::decode(&answer).unwrap().peers;
    assert_eq!(listed.len(), 2_000);
    // The fastest first; the caller is not listed.
    assert_eq!(listed[0].ip.as_deref(), Some("10.0.7.212"));
    assert_eq!(listed[0].port, Some(4002));
    assert!(
        listed
            .iter()
            .all(|peer| peer.ip.as_deref() != Some("192.0.2.1"))
    );
}

#[tokio::test]
async fn get_common_blocks_answers_the_highest_known_block() {
    let server = server(Node::at(10));
    let (low, high, unknown) = (block(3), block(7), block(9));
    *server.node.common.lock().unwrap() = vec![high.header().clone(), low.header().clone()];
    let ids = [&low, &unknown, &high].map(|b| b.id().to_hex()).to_vec();
    let payload = GetCommonBlocksRequest {
        ids: ids.clone(),
        ..GetCommonBlocksRequest::default()
    }
    .encode();
    let answer = call(&server, RouteId::GetCommonBlocks, payload, peer_ip(1))
        .await
        .unwrap();
    let common = GetCommonBlocksResponse::decode(&answer)
        .unwrap()
        .common
        .unwrap();
    assert_eq!(
        (common.height, common.id),
        (Some(7), Some(high.id().to_hex()))
    );
    // An id in capitals names no block, as the reference matches the text; none known: no common.
    let payload = GetCommonBlocksRequest {
        ids: vec![high.id().to_hex().to_uppercase(), unknown.id().to_hex()],
        ..GetCommonBlocksRequest::default()
    }
    .encode();
    let answer = call(&server, RouteId::GetCommonBlocks, payload, peer_ip(1))
        .await
        .unwrap();
    assert_eq!(
        GetCommonBlocksResponse::decode(&answer).unwrap().common,
        None
    );
    // The id in capitals still counts, so the node reads the store, as the reference does.
    let asked = server.node.asked_common.lock().unwrap()[1].clone();
    assert_eq!(asked, (vec![*unknown.id()], 2));
}

// ---- getBlocks --------------------------------------------------------------------------------

fn listing(height: u32, transactions: Vec<Vec<u8>>) -> BlockListing {
    let block = block(height);
    let header = block.header();
    BlockListing {
        id: block.id().to_hex(),
        version: 0,
        timestamp: header.timestamp,
        previous_block: (height > 1)
            .then(|| heartwood_crypto::utils::hex::encode(&header.previous_block)),
        height,
        number_of_transactions: transactions.len() as u32,
        total_amount: Amount::ZERO,
        total_fee: Amount::ZERO,
        reward: Amount::from(header.reward),
        payload_length: 0,
        payload_hash: heartwood_crypto::utils::hex::encode(&header.payload_hash),
        generator_public_key: header.generator_public_key.to_hex(),
        block_signature: header.block_signature.to_hex(),
        transactions,
    }
}

fn get_blocks(last: u32, limit: Option<u32>, headers_only: Option<bool>) -> Vec<u8> {
    GetBlocksRequest {
        last_block_height: Some(last),
        block_limit: limit,
        headers_only,
        ..GetBlocksRequest::default()
    }
    .encode()
}

#[tokio::test]
async fn get_blocks_lists_from_the_height_after_the_one_asked_up_to_the_tip() {
    let server = server(Node::at(10));
    *server.node.stored.lock().unwrap() = (1..=10).map(|h| listing(h, Vec::new())).collect();
    let answer = call(
        &server,
        RouteId::GetBlocks,
        get_blocks(4, None, None),
        peer_ip(1),
    )
    .await;
    let heights: Vec<u32> = (get_blocks_decode_response(&answer.unwrap()).unwrap().iter())
        .map(|block| block.header.height.unwrap())
        .collect();
    assert_eq!(heights, [5, 6, 7, 8, 9, 10]);
    // blockLimit or 400, and headers only when asked.
    call(
        &server,
        RouteId::GetBlocks,
        get_blocks(1, Some(3), Some(true)),
        peer_ip(1),
    )
    .await
    .unwrap();
    assert_eq!(
        *server.node.downloads.lock().unwrap(),
        [(5, 400, false), (2, 3, true)]
    );
    // Nothing above the tip, and the store is not read.
    let answer = call(
        &server,
        RouteId::GetBlocks,
        get_blocks(10, None, None),
        peer_ip(1),
    )
    .await;
    assert_eq!(answer.unwrap(), Vec::<u8>::new());
    assert_eq!(server.node.downloads.lock().unwrap().len(), 2);
    // The bytes are the reference's for the store's blocks.
    let answer = call(
        &server,
        RouteId::GetBlocks,
        get_blocks(9, None, None),
        peer_ip(1),
    )
    .await;
    let tenth = server.node.stored.lock().unwrap()[9].clone();
    let expected = heartwood_p2p_wire::codecs::blocks::get_blocks_encode_listing(&[tenth]);
    assert_eq!(answer.unwrap(), expected);
}

#[tokio::test]
async fn get_blocks_stops_once_the_transactions_text_reaches_twenty_mebibytes_less_a_margin() {
    let server = server(Node::at(10));
    // Each block carries 4 MiB of transactions, 8 MiB of hex text: the estimate is under the
    // limit after two blocks and over it after three, so three are sent.
    let big = vec![vec![7_u8; 4 * 1024 * 1024]];
    *server.node.stored.lock().unwrap() = (2..=6).map(|h| listing(h, big.clone())).collect();
    let answer = call(
        &server,
        RouteId::GetBlocks,
        get_blocks(1, None, None),
        peer_ip(1),
    )
    .await;
    let blocks = get_blocks_decode_response(&answer.unwrap()).unwrap();
    let heights: Vec<u32> = blocks
        .iter()
        .map(|block| block.header.height.unwrap())
        .collect();
    assert_eq!(heights, [2, 3, 4]);
    assert_eq!(blocks[0].transactions, big);
}

// ---- postBlock --------------------------------------------------------------------------------

#[tokio::test]
async fn a_peer_s_block_is_answered_at_once_when_pinged_and_refused_when_not_chained() {
    let server = server(Node::at(10));
    let tip = server.node.chain.last_block();
    let next = next_block(&tip, &delegate(1), 0);
    // A pinged block: `{status: true, height}`, and nothing handed to the node.
    *server.node.pinged.lock().unwrap() = Some(*next.id());
    let answer = call(&server, RouteId::PostBlock, post_block(&next), peer_ip(1))
        .await
        .unwrap();
    assert_eq!(post_answer(&answer), (true, 10));
    assert!(server.node.incoming.lock().unwrap().is_empty());
    // Not chained to the last downloaded block: `{status: false, height}`.
    *server.node.pinged.lock().unwrap() = None;
    *server.node.last_downloaded.lock().unwrap() = ChainLink::of(&block(9));
    let answer = call(&server, RouteId::PostBlock, post_block(&next), peer_ip(1))
        .await
        .unwrap();
    assert_eq!(post_answer(&answer), (false, 10));
    // Chained: handed to the node as a peer's block.
    *server.node.last_downloaded.lock().unwrap() = ChainLink::of(&tip);
    let answer = call(&server, RouteId::PostBlock, post_block(&next), peer_ip(1))
        .await
        .unwrap();
    assert_eq!(post_answer(&answer), (true, 10));
    let incoming = server.node.incoming.lock().unwrap();
    assert_eq!(incoming.len(), 1);
    assert_eq!(
        (incoming[0].0.id(), incoming[0].1, incoming[0].2),
        (next.id(), false, peer_ip(1))
    );
}

#[tokio::test]
async fn the_forger_s_block_skips_the_ping_and_the_chain_checks() {
    let server = server(Node::at(10));
    let unchained = block(12);
    *server.node.pinged.lock().unwrap() = Some(*unchained.id());
    let answer = call(
        &server,
        RouteId::PostBlock,
        post_block(&unchained),
        forger(),
    )
    .await
    .unwrap();
    assert_eq!(post_answer(&answer), (true, 10));
    let incoming = server.node.incoming.lock().unwrap();
    assert_eq!(
        (incoming[0].0.id(), incoming[0].1, incoming[0].2),
        (unchained.id(), true, forger())
    );
}

#[tokio::test]
async fn a_block_of_no_ranked_delegate_is_refused_and_too_many_transactions_is_an_error() {
    let server = server(Node::at(10));
    let tip = server.node.chain.last_block();
    let stranger = next_block(
        &tip,
        &KeyPair::from_passphrase("not a delegate").unwrap(),
        0,
    );
    let answer = call(&server, RouteId::PostBlock, post_block(&stranger), forger())
        .await
        .unwrap();
    assert_eq!(post_answer(&answer), (false, 10));
    assert!(server.node.incoming.lock().unwrap().is_empty());
    // More transactions than the milestone at the tip allows: the handler's error, a 500.
    let chain = Chain::with(10, milestones_with_max_transactions(2, 1));
    let server = self::server(Node::over(chain));
    let mut draft_block = next_block(&tip, &delegate(1), 0).encode();
    // The header's transaction count (bytes 44 to 47) says two.
    draft_block[44] = 2;
    let payload = PostBlockRequest {
        block: Some(draft_block),
        ..PostBlockRequest::default()
    }
    .encode();
    let refused = call(&server, RouteId::PostBlock, payload, peer_ip(1))
        .await
        .unwrap_err();
    assert_eq!(refused.status, 500);
    // Bytes that are no block: the same.
    let payload = PostBlockRequest::default().encode();
    let refused = call(&server, RouteId::PostBlock, payload, peer_ip(1))
        .await
        .unwrap_err();
    assert_eq!(
        refused,
        ServerRefusal::new(500, "An internal server error occurred")
    );
}

// ---- the transactions routes ------------------------------------------------------------------

#[tokio::test]
async fn unconfirmed_transactions_are_collated_validated_only_for_the_forger() {
    let server = server(Node::at(10));
    *server.node.pool.lock().unwrap() = vec![vec![1, 2], vec![3]];
    let excluded = [9_u8; 32];
    let payload = get_unconfirmed_transactions_encode_request(&UnconfirmedRequestInput {
        exclude: vec![excluded],
        ..UnconfirmedRequestInput::default()
    });
    for (ip, validate) in [(peer_ip(1), false), (forger(), true)] {
        let answer = call(
            &server,
            RouteId::GetUnconfirmedTransactions,
            payload.clone(),
            ip,
        )
        .await;
        let answer = get_unconfirmed_transactions_decode_response(&answer.unwrap()).unwrap();
        assert_eq!(answer.message.pool_size, Some(2));
        assert_eq!(answer.transactions, [vec![1, 2], vec![3]]);
        let last = server
            .node
            .collations
            .lock()
            .unwrap()
            .last()
            .cloned()
            .unwrap();
        assert_eq!(last, (validate, vec![TransactionId::from_array(excluded)]));
    }
    // countOnly: no collation, and the pool's size.
    let payload = get_unconfirmed_transactions_encode_request(&UnconfirmedRequestInput {
        count_only: Some(true),
        ..UnconfirmedRequestInput::default()
    });
    let answer = call(
        &server,
        RouteId::GetUnconfirmedTransactions,
        payload,
        peer_ip(1),
    )
    .await;
    let answer = get_unconfirmed_transactions_decode_response(&answer.unwrap()).unwrap();
    assert_eq!(
        (answer.message.pool_size, answer.transactions.len()),
        (Some(9), 0)
    );
    assert_eq!(server.node.collations.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn posted_transactions_go_to_the_pool_and_the_accepted_ids_are_answered() {
    let server = server(Node::at(10));
    let accepted = TransactionId::from_array([4; 32]);
    *server.node.accepted.lock().unwrap() = vec![accepted];
    let payload = PostTransactionsRequest {
        transactions: Some(list::encode(&[vec![1_u8, 2], vec![3]])),
        ..PostTransactionsRequest::default()
    }
    .encode();
    let answer = call(&server, RouteId::PostTransactions, payload, peer_ip(1))
        .await
        .unwrap();
    assert_eq!(
        post_transactions_decode_response(&answer).unwrap(),
        [accepted.to_hex()]
    );
    assert_eq!(
        *server.node.posted.lock().unwrap(),
        [vec![vec![1, 2], vec![3]]]
    );
}

// ---- the internal routes ----------------------------------------------------------------------

#[tokio::test]
async fn emit_event_dispatches_the_four_forger_events_and_refuses_any_other_gracefully() {
    let server = server(Node::at(10));
    let names = [
        "forger.started",
        "forger.failed",
        "block.forged",
        "transaction.forged",
    ];
    for name in names {
        let payload = internal::encode(&EmitEvent {
            event: name.into(),
            body: json!({"id": name}),
        });
        let answer = call(&server, RouteId::EmitEvent, payload, forger()).await;
        assert_eq!(answer.unwrap(), b"true");
    }
    let body = |name: &str| json!({"id": name});
    assert_eq!(
        *server.node.events.lock().unwrap(),
        [
            ForgerEvent::Started(body(names[0])),
            ForgerEvent::Failed(body(names[1])),
            ForgerEvent::BlockForged(body(names[2])),
            ForgerEvent::TransactionForged(body(names[3])),
        ]
    );
    // Another name: a graceful error (499, the socket kept, never a ban), not dispatched.
    let payload = internal::encode(&EmitEvent {
        event: "block.applied".into(),
        body: json!({}),
    });
    let refused = call(&server, RouteId::EmitEvent, payload, forger())
        .await
        .unwrap_err();
    assert_eq!(refused.status, 499);
    assert_eq!(server.node.events.lock().unwrap().len(), 4);
}

#[tokio::test]
async fn the_round_the_slot_and_the_wake_up() {
    let clock = Arc::new(ManualClock::new(START_MS + 3_000));
    let server = Server::new(Node::at(10), clock.clone());
    *server.node.round.lock().unwrap() = Some(round(10));
    let answer = call(&server, RouteId::GetCurrentRound, b"{}".to_vec(), forger())
        .await
        .unwrap();
    assert_eq!(answer, internal::encode(&round(10)));
    assert_eq!(*server.node.round_times.lock().unwrap(), [clock.now_ms()]);
    // The slot of a timestamp, or of now (1,003 s after the epoch: slot 125).
    let asked = |timestamp| internal::encode(&SlotNumberRequest { timestamp });
    let answer = call(&server, RouteId::GetSlotNumber, asked(Some(17)), forger())
        .await
        .unwrap();
    assert_eq!(answer, internal::encode(&SlotNumber(2)));
    let answer = call(&server, RouteId::GetSlotNumber, asked(None), forger())
        .await
        .unwrap();
    assert_eq!(answer, b"125");
    let answer = call(&server, RouteId::SyncBlockchain, b"{}".to_vec(), forger())
        .await
        .unwrap();
    assert_eq!(
        (
            answer.as_slice(),
            server.node.wakeups.load(Ordering::SeqCst)
        ),
        (&b"true"[..], 1)
    );
    // A body the route's reader refuses.
    let refused = call(&server, RouteId::GetSlotNumber, b"[]".to_vec(), forger())
        .await
        .unwrap_err();
    assert_eq!(refused, ServerRefusal::new(400, "Validation failed"));
}

#[tokio::test]
async fn get_network_state_cleanses_then_analyses_and_takes_this_node_s_delegates_from_the_peers() {
    let clock = Arc::new(ManualClock::new(START_MS));
    let server = Server::new(Node::at(10), clock);
    let key = |i: usize| delegate(i).public_key().to_compressed();
    *server.node.active.lock().unwrap() = (0..5).map(key).collect();
    let mut peer = Peer::new(peer_ip(1), 4002);
    peer.public_keys = vec![key(0), key(1), key(2)];
    *server.peers.peers.lock().unwrap() = vec![peer];
    let payload = internal::encode(&NetworkStateRequest { log: true });
    // Without an attestation session there is no delegate on this node.
    let answer = call(&server, RouteId::GetNetworkState, payload.clone(), forger())
        .await
        .unwrap();
    let state: NetworkState = internal::decode(&answer).unwrap();
    assert_eq!(state.status, NetworkStateStatus::BelowMinimumDelegates);
    assert!(server.peers.removed.lock().unwrap().is_empty());
    let cleanse = server.peers.cleanses.lock().unwrap()[0];
    assert!(cleanse.fast && cleanse.force_ping && cleanse.log && cleanse.skip_common_blocks);
    assert_eq!(cleanse.peer_count, None);
    let served = server.node.served.lock().unwrap().clone();
    assert_eq!(served, [(forger(), state.clone())]);
    assert_eq!(answer, internal::encode(&state));
    // A cold start answers ColdStart once and ends.
    server.peers.cold.store(true, Ordering::SeqCst);
    let answer = call(&server, RouteId::GetNetworkState, payload.clone(), forger())
        .await
        .unwrap();
    let state: NetworkState = internal::decode(&answer).unwrap();
    assert_eq!(state.status, NetworkStateStatus::ColdStart);
    assert!(server.peers.cold_ended.load(Ordering::SeqCst));
    let answer = call(&server, RouteId::GetNetworkState, payload, forger())
        .await
        .unwrap();
    let state: NetworkState = internal::decode(&answer).unwrap();
    assert_eq!(state.status, NetworkStateStatus::BelowMinimumDelegates);
}

#[test]
fn remote_access_entries_that_admit_no_forger_are_warned_of_at_start() {
    let with = |entries: &[&str]| P2pOptions {
        remote_access: entries.iter().map(|entry| (*entry).to_owned()).collect(),
        ..P2pOptions::default()
    };
    assert!(remote_access_warnings(&with(&["127.0.0.1", "::1"])).is_empty());
    // A prefix entry matches no source; with no exact address left, no forger is admitted.
    let warnings = remote_access_warnings(&with(&["127.0.0.1", "10.0.0.0/8"]));
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("10.0.0.0/8"));
    assert_eq!(remote_access_warnings(&with(&["10.0.0.0/8"])).len(), 2);
    assert_eq!(remote_access_warnings(&with(&[])).len(), 1);
    let exact = ServerOptions::from_p2p(&with(&["10.0.0.0/8"]));
    assert!(exact.remote_access.is_empty());
}

// ---- the whole server, with the communicator ---------------------------------------------

#[cfg(feature = "difftest")]
mod over_the_server {
    //! The node's communicator against the whole server (its transport and these controllers),
    //! bound on 127.12.0.1, in real time.

    use std::time::Duration;

    use heartwood_p2p::seams::PeerClient;
    use heartwood_p2p::utils::rules::Rules as RealRules;
    use heartwood_p2p::{BlocksRequest, Connector, P2pServer, ServerParts, SharedPeer};
    use peer_fakes::{Fixture, options};
    use tokio::sync::watch;

    use super::*;

    #[tokio::test]
    #[ignore = "integration"]
    async fn the_communicator_pings_downloads_and_posts_against_this_server() {
        let clock: Arc<dyn Clock> = Arc::new(heartwood_kernel::SystemClock);
        let server = Server::new(Node::at(10), clock);
        *server.node.stored.lock().unwrap() = (1..=10).map(|h| listing(h, Vec::new())).collect();
        let host = IpAddr::from([127, 12, 0, 1]);
        let p2p = P2pOptions {
            host,
            port: 0,
            remote_access: Vec::new(),
            ..P2pOptions::default()
        };
        let handlers: Arc<dyn RouteHandlers> = Arc::new(server.controllers);
        let listening = P2pServer::bind(ServerParts {
            options: ServerOptions::from_p2p(&p2p),
            node: server.node.clone(),
            peers: server.peers.clone(),
            handlers,
            events: server.session.clone(),
        })
        .await
        .unwrap();
        let address = listening.local_addr();
        let (stop, shutdown) = watch::channel(false);
        let listening = Arc::new(listening);
        let serving = Arc::clone(&listening);
        let task = tokio::spawn(async move { serving.serve(shutdown).await });
        let fixture = Fixture::over(
            Connector::new(),
            RealRules::new(100, 25),
            Chain::at(10),
            options(),
        );
        let peer = SharedPeer::new(Peer::new(address.ip(), address.port()));
        fixture.repository.set(peer.clone());
        let client = &fixture.communicator;
        // Pinging: the served status, which the communicator checks and takes.
        let state = client
            .ping(&peer, Duration::from_secs(5), true, true, false)
            .await;
        assert_eq!(state.unwrap().unwrap().height, 10);
        // Downloading: the stored blocks after height 4.
        let blocks = client
            .get_peer_blocks(&peer, BlocksRequest::after(4))
            .await
            .unwrap();
        assert_eq!(blocks.len(), 6);
        // Posting: a block chained to the tip, handed to the node.
        let tip = server.node.chain.last_block();
        client
            .post_block(&peer, &next_block(&tip, &delegate(1), 0))
            .await;
        assert_eq!(server.node.incoming.lock().unwrap().len(), 1);
        let _ = stop.send(true);
        let _ = task.await;
    }
}
