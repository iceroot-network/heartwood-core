//! The reference-client probe's comparison (`difftest p2p compare`) on fixture records: what the
//! masks let through, what the getPeers rule accepts, and the differences that fail.

use std::fs;
use std::path::Path;

use heartwood_difftest::p2p_compare::compare;
use heartwood_difftest::report::{Outcome, Report};
use heartwood_difftest::store::ScratchDir;
use serde_json::{Value, json};

const STATUS: &str = "p2p.peer.getStatus";
const PEERS: &str = "p2p.peer.getPeers";
const BLOCKS: &str = "p2p.blocks.getBlocks";

/// One node's records: its address, and what its case list produced.
struct Node {
    name: &'static str,
    ip: &'static str,
    peers: Vec<&'static str>,
    version: &'static str,
    slot: u64,
    socket_id: &'static str,
    blocks: Vec<u64>,
    banned: bool,
    idle_socket: &'static str,
    post: Value,
    /// The attestations its status carries (a relay without a forger has none).
    signers: usize,
}

fn node(name: &'static str, ip: &'static str) -> Node {
    Node {
        name,
        ip,
        peers: Vec::new(),
        version: "4.3.1",
        slot: 40,
        socket_id: "1790682791534:node1:80:abc:1",
        blocks: vec![1, 2, 3],
        banned: true,
        idle_socket: "open",
        post: json!([]),
        signers: 2,
    }
}

fn call(endpoint: &str, answer: Value) -> Value {
    json!({"endpoint": endpoint, "requestSha256": "aa", "verdict": "valid", "answer": answer})
}

fn records(n: &Node) -> Vec<Value> {
    let peers: Vec<Value> = n
        .peers
        .iter()
        .map(|ip| json!({"ip": ip, "port": 4000}))
        .collect();
    let blocks: Vec<Value> = n
        .blocks
        .iter()
        .map(|h| json!({"id": format!("b{h}"), "height": h}))
        .collect();
    let keys: Vec<String> = (0..n.signers).map(|i| format!("k{i}")).collect();
    let signatures: Vec<String> = (0..n.signers).map(|i| format!("s{i}")).collect();
    let status = json!({
        "state": {"height": 40, "currentSlot": n.slot, "forgingAllowed": true},
        "config": {"version": n.version, "network": {"name": "devnet"}},
        "publicKeys": keys, "signatures": signatures,
    });
    vec![
        json!({"case": "hello", "kind": "hello", "ip": n.ip,
               "hello": {"socketId": n.socket_id, "heartbeat": 20000}}),
        json!({"case": "ping", "kind": "ping", "ip": n.ip, "configValid": true,
               "attestations": {"signed": n.signers, "verified": n.signers}, "calls": [call(STATUS, status)]}),
        json!({"case": "get-peers", "kind": "call", "ip": n.ip, "accepted": true, "socket": "open",
               "calls": [call(PEERS, Value::Array(peers))]}),
        json!({"case": "blocks", "kind": "call", "ip": n.ip, "accepted": true, "socket": "open",
               "calls": [call(BLOCKS, json!({"count": blocks.len(), "blocks": blocks}))]}),
        json!({"case": "post", "kind": "call", "ip": n.ip, "accepted": false, "socket": "open",
               "calls": [call("p2p.transactions.postTransactions", n.post.clone())]}),
        json!({"case": "idle", "kind": "idle", "ip": n.ip, "pings": 3, "pingsSeen": true,
               "socket": n.idle_socket, "disconnects": []}),
        json!({"case": "refuse", "kind": "refusal", "ip": n.ip, "socket": "closed", "banned": n.banned,
               "disconnects": [{"code": 1006}],
               "outcome": {"answered": false, "error": {"type": "disconnect"}},
               "calls": [{"endpoint": STATUS, "requestSha256": "bb",
                          "error": {"type": "disconnect", "message": "Request failed - server disconnected"}}]}),
    ]
}

fn write_node(root: &Path, n: &Node) {
    let dir = root.join(n.name);
    fs::create_dir_all(&dir).unwrap();
    let lines: Vec<String> = records(n).iter().map(Value::to_string).collect();
    fs::write(dir.join("cases.jsonl"), lines.join("\n") + "\n").unwrap();
}

/// A four-node run: `left` and `right` are the two probed relays, node3 is reachable from both,
/// node4 from `left` only, node5 is behind the tip.
fn topology() -> Value {
    json!({
        "addresses": {"left": "10.0.0.1", "right": "10.0.0.2", "node3": "10.0.0.3",
                      "node4": "10.0.0.4", "node5": "10.0.0.5"},
        "reach": {"left": ["right", "node3", "node4", "node5"], "right": ["left", "node3", "node5"]},
        "notAtTip": ["node5"],
        "expected": ["left", "right", "node3", "node4", "node5"],
    })
}

/// The two probed relays alone, each reaching the other.
fn two_nodes() -> Value {
    json!({
        "addresses": {"left": "10.0.0.1", "right": "10.0.0.2"},
        "reach": {"left": ["right"], "right": ["left"]},
        "expected": [],
    })
}

fn run(left: &Node, right: &Node, topology: Option<Value>) -> Report {
    let scratch = ScratchDir::new("p2p-compare").unwrap();
    let root = scratch.path().join("p2p");
    write_node(&root, left);
    write_node(&root, right);
    if let Some(t) = topology {
        fs::write(root.join("topology.json"), t.to_string()).unwrap();
    }
    compare(scratch.path(), left.name, right.name).unwrap()
}

fn outcome<'a>(report: &'a Report, case: &str) -> &'a Outcome {
    &report
        .results
        .iter()
        .find(|r| r.label == case)
        .unwrap_or_else(|| panic!("no case {case}"))
        .outcome
}

fn failing(report: &Report) -> Vec<&str> {
    let bad = report
        .results
        .iter()
        .filter(|r| !matches!(r.outcome, Outcome::Pass));
    bad.map(|r| r.label.as_str()).collect()
}

#[test]
fn a_masked_version_passes() {
    let mut left = node("left", "10.0.0.1");
    left.version = "4.3.1-heartwood.0.1.0";
    left.socket_id = "heartwood:7";
    left.slot = 41;
    left.signers = 0;
    let report = run(&left, &node("right", "10.0.0.2"), Some(two_nodes()));
    assert_eq!(failing(&report), Vec::<&str>::new());
}

#[test]
fn a_peer_only_one_target_reaches_passes() {
    let mut left = node("left", "10.0.0.1");
    left.peers = vec!["10.0.0.2", "10.0.0.3", "10.0.0.4"];
    let mut right = node("right", "10.0.0.2");
    right.peers = vec!["10.0.0.1", "10.0.0.3", "10.0.0.5"];
    let report = run(&left, &right, Some(topology()));
    assert_eq!(failing(&report), Vec::<&str>::new());
}

#[test]
fn an_unexpected_extra_peer_fails() {
    let mut left = node("left", "10.0.0.1");
    left.peers = vec!["10.0.0.2", "10.0.0.3", "10.0.0.4", "10.0.0.99"];
    let mut right = node("right", "10.0.0.2");
    right.peers = vec!["10.0.0.1", "10.0.0.3"];
    let report = run(&left, &right, Some(topology()));
    assert_eq!(failing(&report), ["get-peers"]);
}

#[test]
fn a_missing_common_peer_fails() {
    let mut left = node("left", "10.0.0.1");
    left.peers = vec!["10.0.0.2"];
    let mut right = node("right", "10.0.0.2");
    right.peers = vec!["10.0.0.1", "10.0.0.3"];
    let report = run(&left, &right, Some(topology()));
    assert_eq!(failing(&report), ["get-peers"]);
}

#[test]
fn a_target_listing_itself_fails() {
    let mut left = node("left", "10.0.0.1");
    left.peers = vec!["10.0.0.1", "10.0.0.2", "10.0.0.3"];
    let mut right = node("right", "10.0.0.2");
    right.peers = vec!["10.0.0.1", "10.0.0.3"];
    let report = run(&left, &right, Some(topology()));
    assert_eq!(failing(&report), ["get-peers"]);
}

#[test]
fn a_changed_block_listing_fails() {
    let mut left = node("left", "10.0.0.1");
    left.blocks = vec![1, 2];
    let report = run(&left, &node("right", "10.0.0.2"), Some(two_nodes()));
    assert_eq!(failing(&report), ["blocks"]);
}

#[test]
fn a_missing_ban_fails() {
    let mut left = node("left", "10.0.0.1");
    left.banned = false;
    let report = run(&left, &node("right", "10.0.0.2"), Some(two_nodes()));
    assert_eq!(failing(&report), ["refuse"]);
}

#[test]
fn a_closed_socket_where_the_other_kept_it_open_fails() {
    let mut left = node("left", "10.0.0.1");
    left.idle_socket = "closed";
    let report = run(&left, &node("right", "10.0.0.2"), Some(two_nodes()));
    assert_eq!(failing(&report), ["idle"]);
}

#[test]
fn a_status_false_where_the_other_terminated_all_fails() {
    let (left, right) = (node("left", "10.0.0.1"), node("right", "10.0.0.2"));
    let scratch = ScratchDir::new("p2p-compare").unwrap();
    let root = scratch.path().join("p2p");
    write_node(&root, &left);
    write_node(&root, &right);
    fs::write(root.join("topology.json"), two_nodes().to_string()).unwrap();
    // The oversized postBlock: the reference terminates the socket and bans the caller; the left
    // node answers `{status: false}`, keeps the socket open and does not ban.
    let terminated = json!({"case": "refuse-post-block", "kind": "refusal", "ip": "10.0.0.2",
        "socket": "closed", "banned": true, "disconnects": [{"code": 1006}],
        "outcome": {"answered": false, "error": {"type": "disconnect"}},
        "calls": [{"endpoint": "p2p.blocks.postBlock", "requestSha256": "cc",
                   "error": {"type": "disconnect", "message": "Request failed - server disconnected"}}]});
    let answered = json!({"case": "refuse-post-block", "kind": "refusal", "ip": "10.0.0.1",
        "socket": "open", "banned": false, "disconnects": [],
        "outcome": {"answered": true},
        "calls": [{"endpoint": "p2p.blocks.postBlock", "requestSha256": "cc", "verdict": "valid",
                   "answer": {"status": false}}]});
    for (name, record) in [("left", answered), ("right", terminated)] {
        let file = root.join(name).join("cases.jsonl");
        let mut text = fs::read_to_string(&file).unwrap();
        text.push_str(&format!("{record}\n"));
        fs::write(file, text).unwrap();
    }
    let report = compare(scratch.path(), "left", "right").unwrap();
    assert_eq!(failing(&report), ["refuse-post-block"]);
    let Outcome::Mismatch(differences) = outcome(&report, "refuse-post-block") else {
        panic!("not a mismatch");
    };
    // The socket, the ban and the answer all differ.
    assert!(differences.len() >= 3, "{differences:?}");
}

#[test]
fn a_getpeers_case_without_a_topology_fails() {
    let mut left = node("left", "10.0.0.1");
    left.peers = vec!["10.0.0.2", "10.0.0.3"];
    let mut right = node("right", "10.0.0.2");
    right.peers = vec!["10.0.0.1", "10.0.0.3"];
    assert_eq!(failing(&run(&left, &right, None)), ["get-peers"]);
    // A topology without a route set for one of the two targets fails the same way.
    let mut partial = two_nodes();
    partial["reach"] = json!({"left": ["right"]});
    assert_eq!(failing(&run(&left, &right, Some(partial))), ["get-peers"]);
}

#[test]
fn a_case_one_side_lacks_fails() {
    let scratch = ScratchDir::new("p2p-compare").unwrap();
    let root = scratch.path().join("p2p");
    let (left, right) = (node("left", "10.0.0.1"), node("right", "10.0.0.2"));
    write_node(&root, &left);
    write_node(&root, &right);
    let mut lines: Vec<String> = records(&right).iter().map(Value::to_string).collect();
    lines.pop();
    fs::write(
        root.join("right").join("cases.jsonl"),
        lines.join("\n") + "\n",
    )
    .unwrap();
    fs::write(root.join("topology.json"), two_nodes().to_string()).unwrap();
    let report = compare(scratch.path(), "left", "right").unwrap();
    assert_eq!(failing(&report), ["refuse"]);
}
