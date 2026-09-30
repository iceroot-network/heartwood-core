//! The public routes' answers against the reference's, and the differences that are kept.
//!
//! Class V32 (`p2p.controller`) is read from `tests/vectors` of the crate, which must hold it
//! (`make vectors-install`), or from the directory in `P2P_VECTORS`, which may lack it (the test
//! then says so and passes). Each record is answered by the server's controllers over a scripted
//! node and compared with the reference's answer, without its log lines, which the differential
//! tests compare. The rest are the answers that differ from the reference's on purpose: a block
//! refused at the receipt, a peer without a latency, and a peer configuration in the order given.

#![cfg(feature = "node")]

mod peer_fakes;

use std::path::Path;

use heartwood_crypto::blocks::factory::{self, BlockDraft};
use heartwood_crypto::crypto::hash::sha256;
use heartwood_crypto::crypto::sig::Aux;
use heartwood_crypto::transactions::builders::TransactionBuilder;
use heartwood_crypto::transactions::types::{Memo, TransferItem};
use heartwood_crypto::utils::hex;
use heartwood_crypto::{Address, Block, Transaction};
use heartwood_p2p::testing::served::{Served, controller};
use heartwood_p2p_wire::codecs::peer::get_peers_decode_response;
use heartwood_p2p_wire::messages::blocks::{PostBlockRequest, PostBlockResponse};
use heartwood_p2p_wire::{Proto, RouteId};
use peer_fakes::{DEVNET, block, delegate};
use serde_json::{Value, json};

const CLASS: &str = "V32-served";

/// The class's `p2p.controller` records, or `None` when the directory `P2P_VECTORS` names lacks
/// the class. The crate's own directory must hold it.
fn records() -> Option<Vec<Value>> {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let named = std::env::var_os("P2P_VECTORS").filter(|dir| !dir.is_empty());
    let dir = named.as_ref().map_or_else(
        || manifest.join("tests/vectors"),
        |dir| manifest.join("../..").join(dir),
    );
    let file = dir.join(format!("{CLASS}.jsonl"));
    let text = match std::fs::read_to_string(&file) {
        Ok(text) => text,
        // A directory named in the environment may lack the class; the crate's own must hold it.
        Err(_) if named.is_some() => return None,
        Err(error) => panic!("{}: {error} (make vectors-install)", file.display()),
    };
    let records = text
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap());
    Some(
        records
            .filter(|record| record["op"] == "p2p.controller")
            .collect(),
    )
}

/// The reference's answer to a record without its log lines.
fn expected(record: &Value) -> Value {
    let mut output = record["output"].clone();
    if let Some(fields) = output.as_object_mut() {
        fields.remove("lines");
        fields.remove("notes");
    }
    output
}

async fn serve(input: &Value, height: u32, milestones: &str) -> Served {
    let milestones = heartwood_crypto::Milestones::load(milestones, &DEVNET.network).unwrap();
    let network = DEVNET.network.clone();
    controller(input, height, milestones, network)
        .await
        .unwrap()
}

#[tokio::test]
async fn the_answers_match_the_reference() {
    let Some(records) = records() else {
        eprintln!("{CLASS} is not in the directory P2P_VECTORS names");
        return;
    };
    assert!(!records.is_empty(), "{CLASS} has no p2p.controller record");
    let (mut thrown, mut documented) = (0, 0);
    for record in &records {
        let name = record["name"].as_str().unwrap();
        let height = u32::try_from(record["height"].as_u64().unwrap()).unwrap();
        let milestones = record["input"]["milestones"]
            .as_str()
            .unwrap_or(&DEVNET.milestones_text);
        let served = serve(&record["input"], height, milestones).await;
        if name.contains("documented:") {
            // The reference lets the block through the receipt; Heartwood answers 500 there.
            documented += 1;
            assert_eq!(served.refusal.map(|r| r.status), Some(500), "{name}");
        } else if expected(record)["thrown"] == true {
            thrown += 1;
            assert_eq!(served.refusal.map(|r| r.status), Some(500), "{name}");
        } else {
            assert_eq!(served.refusal, None, "{name}");
            assert_eq!(served.output, expected(record), "{name}");
        }
    }
    assert!(
        thrown > 0 && documented > 0,
        "{thrown} thrown, {documented} documented"
    );
}

// ---- the peers --------------------------------------------------------------------------------

fn peers_input(caller: &str, peers: &Value) -> Value {
    json!({
        "route": RouteId::GetPeers.id(),
        "request": "",
        "ip": caller,
        "remoteAccess": ["127.0.0.1"],
        "peers": peers,
    })
}

async fn listed(input: &Value) -> Vec<(String, u32)> {
    let served = serve(input, 2, &DEVNET.milestones_text).await;
    assert_eq!(served.refusal, None);
    let bytes = hex::decode(served.output["hex"].as_str().unwrap()).unwrap();
    let peers = get_peers_decode_response(&bytes).unwrap();
    (peers.into_iter())
        .map(|peer| (peer.ip.unwrap(), peer.port.unwrap()))
        .collect()
}

#[tokio::test]
async fn a_peer_without_a_latency_is_listed_last() {
    // Heartwood sorts such a peer after every other; the reference's comparison leaves its place
    // to the order of the list.
    let peers = json!([
        {"ip": "10.0.0.1", "port": 4000, "latency": null},
        {"ip": "10.0.0.2", "port": 4000, "latency": 50},
        {"ip": "10.0.0.3", "port": 4000, "latency": 5},
    ]);
    let answer = listed(&peers_input("10.0.0.9", &peers)).await;
    let ips: Vec<&str> = answer.iter().map(|(ip, _)| ip.as_str()).collect();
    assert_eq!(ips, ["10.0.0.3", "10.0.0.2", "10.0.0.1"]);
}

#[tokio::test]
async fn equal_latencies_keep_the_repository_order_and_the_caller_is_left_out() {
    let peers = json!([
        {"ip": "10.0.0.4", "port": 4004, "latency": 7},
        {"ip": "10.0.0.9", "port": 4009, "latency": 1},
        {"ip": "10.0.0.2", "port": 4002, "latency": 7},
    ]);
    let answer = listed(&peers_input("10.0.0.9", &peers)).await;
    assert_eq!(
        answer,
        [("10.0.0.4".to_owned(), 4004), ("10.0.0.2".to_owned(), 4002)]
    );
}

#[tokio::test]
async fn a_peer_with_port_minus_one_is_not_held() {
    // The reference leaves such a peer out of its answer; a port here cannot be negative.
    let peers = json!([
        {"ip": "10.0.0.1", "port": -1, "latency": 1},
        {"ip": "10.0.0.2", "port": 4002, "latency": 2},
    ]);
    let answer = listed(&peers_input("10.0.0.9", &peers)).await;
    assert_eq!(answer, [("10.0.0.2".to_owned(), 4002)]);
}

#[tokio::test]
async fn an_ipv4_mapped_caller_is_the_address_it_maps() {
    // The reference compares the text of the two addresses; here they are one address.
    let peers = json!([
        {"ip": "10.0.0.9", "port": 4009, "latency": 1},
        {"ip": "10.0.0.2", "port": 4002, "latency": 2},
    ]);
    let answer = listed(&peers_input("::ffff:10.0.0.9", &peers)).await;
    assert_eq!(answer, [("10.0.0.2".to_owned(), 4002)]);
}

#[tokio::test]
async fn at_most_two_thousand_peers_are_listed() {
    let peers: Vec<Value> = (0..2_001_u32)
        .map(|i| {
            let ip = format!("10.{}.{}.{}", 1 + i / 65_536, (i / 256) % 256, i % 256);
            json!({"ip": ip, "port": 4000, "latency": i})
        })
        .collect();
    let answer = listed(&peers_input("192.0.2.1", &Value::Array(peers))).await;
    assert_eq!(answer.len(), 2_000);
    assert_eq!(answer[0].0, "10.1.0.0");
}

// ---- the status -------------------------------------------------------------------------------

#[tokio::test]
async fn the_status_lists_the_plugins_in_the_order_given() {
    let config = |names: &[&str]| {
        let plugins: serde_json::Map<String, Value> = names
            .iter()
            .map(|name| ((*name).to_owned(), json!({"enabled": true, "port": 4003})))
            .collect();
        json!({
            "version": "1.0.0",
            "network": {
                "version": DEVNET.network.pub_key_hash,
                "name": DEVNET.network.name,
                "nethash": DEVNET.network.nethash,
                "explorer": DEVNET.network.explorer,
                "token": {"name": DEVNET.network.token, "symbol": DEVNET.network.symbol},
            },
            "plugins": plugins,
        })
    };
    let tip = block(2);
    let ask = |names: &[&str]| {
        json!({
            "route": RouteId::GetStatus.id(),
            "request": "",
            "ip": "10.0.0.9",
            "remoteAccess": ["127.0.0.1"],
            "status": {
                "tip": {"hex": hex::encode(&tip.encode())},
                "now": 1_000,
                "peerConfig": config(names),
                "registered": [],
                "forgerKeys": [],
            },
        })
    };
    let api_first = serve(&ask(&["api", "webhooks"]), 2, &DEVNET.milestones_text).await;
    let webhooks_first = serve(&ask(&["webhooks", "api"]), 2, &DEVNET.milestones_text).await;
    assert_eq!(api_first.refusal, None);
    // The answer follows the order the node's configuration gives, so the two differ.
    assert_ne!(api_first.output, webhooks_first.output);
    let text = |served: &Served| {
        let bytes = hex::decode(served.output["hex"].as_str().unwrap()).unwrap();
        String::from_utf8_lossy(&bytes).into_owned()
    };
    let (first, second) = (text(&api_first), text(&webhooks_first));
    assert!(first.find("api").unwrap() < first.find("webhooks").unwrap());
    assert!(second.find("webhooks").unwrap() < second.find("api").unwrap());
}

// ---- postBlock --------------------------------------------------------------------------------

/// A transfer signed by delegate 1 with a memo, decoded.
fn transfer_with_memo() -> Transaction {
    let params = DEVNET.milestones.at(1);
    let recipient = Address::from_public_key(delegate(2).public_key(), DEVNET.network.pub_key_hash);
    let built = TransactionBuilder::transfer(vec![TransferItem {
        amount: 1,
        recipient,
    }])
    .network(DEVNET.network.pub_key_hash)
    .nonce(1)
    .fee(50_000_000)
    .memo(Memo::new("memo!").unwrap())
    .build(&delegate(1), Aux::random(), params)
    .unwrap();
    Transaction::decode(built.bytes(), params).unwrap()
}

/// The block after `last` by delegate 0 with `transactions`, as bytes.
fn next_block(last: &Block, transactions: Vec<Transaction>) -> Vec<u8> {
    let sum = |field: fn(&Transaction) -> u64| transactions.iter().map(field).sum();
    let payload: Vec<u8> = transactions
        .iter()
        .flat_map(|t| t.bytes().to_vec())
        .collect();
    let draft = BlockDraft {
        version: 0,
        timestamp: last.header().timestamp + 8,
        height: last.height() + 1,
        previous_block: *last.id().as_bytes(),
        number_of_transactions: u32::try_from(transactions.len()).unwrap(),
        total_amount: sum(|t| t.data().amount()),
        total_fee: sum(|t| t.data().fee),
        reward: 0,
        payload_length: u32::try_from(payload.len()).unwrap(),
        payload_hash: sha256(&payload),
    };
    let made = factory::make(
        draft,
        transactions,
        &delegate(0),
        Aux::random(),
        &DEVNET.milestones,
    );
    made.unwrap().encode()
}

/// The postBlock input for `posted`, from a peer, chained to `last`, with delegate 0 registered.
fn post(last: &Block, posted: &[u8]) -> Value {
    let request = PostBlockRequest {
        block: Some(posted.to_vec()),
        ..PostBlockRequest::default()
    };
    json!({
        "route": RouteId::PostBlock.id(),
        "request": hex::encode(&request.encode()),
        "ip": "10.0.0.9",
        "remoteAccess": ["127.0.0.1"],
        "store": {"tip": hex::encode(&last.encode())},
        "lastDownloaded": hex::encode(&last.encode()),
        "delegates": [{
            "publicKey": hex::encode(delegate(0).public_key().to_compressed().as_bytes()),
            "username": "delegate_0",
            "rank": 1,
        }],
    })
}

#[tokio::test]
async fn a_new_block_is_taken_and_answered() {
    let last = block(10);
    let posted = next_block(&last, Vec::new());
    let served = serve(&post(&last, &posted), 10, &DEVNET.milestones_text).await;
    assert_eq!(served.refusal, None);
    let bytes = hex::decode(served.output["hex"].as_str().unwrap()).unwrap();
    let answer = PostBlockResponse::decode(&bytes).unwrap();
    assert_eq!((answer.status, answer.height), (Some(true), Some(10)));
}

#[tokio::test]
async fn a_block_with_a_memo_that_is_not_utf8_is_answered_500_at_the_receipt() {
    // The reference answers `{status: true}` and refuses the block in its job.
    let last = block(10);
    let mut posted = next_block(&last, vec![transfer_with_memo()]);
    let at = posted.windows(6).position(|w| w == b"\x05memo!").unwrap();
    posted[at + 1] = 0xff;
    let served = serve(&post(&last, &posted), 10, &DEVNET.milestones_text).await;
    assert_eq!(served.refusal.map(|refusal| refusal.status), Some(500));
}

#[tokio::test]
async fn a_block_whose_header_the_schema_refuses_is_answered_500_at_the_receipt() {
    // A reward above the largest amount the schema takes, in a block that chains.
    let last = block(10);
    let mut posted = next_block(&last, Vec::new());
    posted[64..72].copy_from_slice(&(1_u64 << 63).to_le_bytes());
    let served = serve(&post(&last, &posted), 10, &DEVNET.milestones_text).await;
    assert_eq!(served.refusal.map(|refusal| refusal.status), Some(500));
    // The same block with its reward as made is taken.
    let posted = next_block(&last, Vec::new());
    let served = serve(&post(&last, &posted), 10, &DEVNET.milestones_text).await;
    assert_eq!(served.refusal, None);
}
