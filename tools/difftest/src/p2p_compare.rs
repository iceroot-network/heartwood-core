//! `difftest p2p compare RUN_DIR --left NODE --right NODE`: the reference-client probe's records of
//! two relays of a live run, compared case by case: equal reply verdicts, errors, socket outcomes
//! and ban effects, and answers equal after the masks (the version, the attestations, the slot
//! fields across a slot, the ports), with getPeers compared as sets under the run's route table.
//!
//! The probe writes `NODE/cases.jsonl` for each node it asked (under `RUN_DIR/p2p/`, or under
//! `RUN_DIR` itself). An optional `topology.json` next to them names the run's routes:
//!
//! ```json
//! {"addresses": {"node1": "10.0.0.11"}, "reach": {"node1": ["node2"]},
//!  "notAtTip": ["node6"], "expected": ["node2", "node3"]}
//! ```
//!
//! Without it, or without a route set for a compared node, a getPeers case fails: the rule cannot
//! be applied, and a comparison of the two answers alone would pass a peer that neither target
//! should list.
//!
//! The right node's records are the expected side and the left node's the actual one, as in the
//! other comparisons.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

use crate::compare::{Difference, diff};
use crate::errors::{Error, Result};
use crate::live::run::{read_json, read_lines};
use crate::report::{CaseResult, Outcome, Report};

const GET_STATUS: &str = "p2p.peer.getStatus";
const GET_PEERS: &str = "p2p.peer.getPeers";

/// The run's routes, as the recipe recorded them.
#[derive(Debug, Default)]
struct Topology {
    /// Whether the run wrote one.
    found: bool,
    addresses: BTreeMap<String, String>,
    reach: BTreeMap<String, BTreeSet<String>>,
    not_at_tip: BTreeSet<String>,
    expected: Option<BTreeSet<String>>,
}

fn names(value: Option<&Value>) -> BTreeSet<String> {
    let list = value.and_then(Value::as_array).into_iter().flatten();
    list.filter_map(|v| v.as_str().map(str::to_owned)).collect()
}

impl Topology {
    fn load(root: &Path) -> Result<Topology> {
        let path = root.join("topology.json");
        if !path.exists() {
            return Ok(Topology::default());
        }
        let v = read_json(&path)?;
        let text = |v: &Value| v.as_str().map(str::to_owned);
        let object = |key: &str| v.get(key).and_then(Value::as_object).into_iter().flatten();
        Ok(Topology {
            found: true,
            addresses: object("addresses")
                .filter_map(|(k, v)| Some((k.clone(), text(v)?)))
                .collect(),
            reach: object("reach")
                .map(|(k, v)| (k.clone(), names(Some(v))))
                .collect(),
            not_at_tip: names(v.get("notAtTip")),
            expected: v.get("expected").map(|e| names(Some(e))),
        })
    }

    fn address(&self, name: &str) -> Option<&str> {
        self.addresses.get(name).map(String::as_str)
    }

    fn name_of(&self, ip: &str) -> Option<&str> {
        let found = self.addresses.iter().find(|(_, a)| a.as_str() == ip);
        found.map(|(n, _)| n.as_str())
    }
}

/// One target of the comparison: its name, its address, and its getPeers answer of a case.
struct Side<'a> {
    name: &'a str,
    ip: &'a str,
    peers: Vec<String>,
}

fn problem(what: &str, expected: Value, actual: Value) -> Difference {
    Difference::Value {
        path: format!("$.getPeers.{what}"),
        expected,
        actual,
    }
}

/// The getPeers rule: each answer lies within its target's route set and does not list the target;
/// after removing both targets, every node whose reach differs and every node not at the tip, the
/// two sets are equal, and equal to the expected set. Returns the masked sets and the problems.
fn peers_rule(
    topology: &Topology,
    left: &Side,
    right: &Side,
) -> (Vec<String>, Vec<String>, Vec<Difference>) {
    let mut problems = Vec::new();
    if !topology.found {
        problems.push(problem("topology", json!("topology.json"), json!(null)));
    }
    for side in [left, right] {
        let route_set = topology.reach.get(side.name);
        if topology.found && route_set.is_none() {
            problems.push(problem(
                &format!("{}.route set", side.name),
                json!("a route set"),
                json!(null),
            ));
        }
        for ip in &side.peers {
            let own = ip == side.ip;
            let outside =
                route_set.is_some_and(|set| topology.name_of(ip).is_none_or(|n| !set.contains(n)));
            if own || outside {
                let what = if own {
                    "lists itself"
                } else {
                    "outside the route set"
                };
                problems.push(problem(
                    &format!("{}.{what}", side.name),
                    json!(null),
                    json!(ip),
                ));
            }
        }
    }
    let empty = BTreeSet::new();
    let reach = |side: &Side| topology.reach.get(side.name).unwrap_or(&empty);
    let mut removed: BTreeSet<String> = [left.ip, right.ip]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
    let differing = reach(left).symmetric_difference(reach(right));
    for node in differing.chain(topology.not_at_tip.iter()) {
        removed.extend(topology.address(node).map(str::to_owned));
    }
    let mask = |side: &Side| -> Vec<String> {
        let set: BTreeSet<&String> = side
            .peers
            .iter()
            .filter(|ip| !removed.contains(*ip))
            .collect();
        set.into_iter().cloned().collect()
    };
    let (l, r) = (mask(left), mask(right));
    if let Some(expected) = &topology.expected {
        for name in expected.iter().filter(|n| topology.address(n).is_none()) {
            problems.push(problem(
                &format!("expected.{name}"),
                json!("an address"),
                json!(null),
            ));
        }
        let want: BTreeSet<String> = expected
            .iter()
            .filter_map(|n| topology.address(n))
            .filter(|ip| !removed.contains(*ip))
            .map(str::to_owned)
            .collect();
        for (side, got) in [(left, &l), (right, &r)] {
            if got.iter().cloned().collect::<BTreeSet<_>>() != want {
                problems.push(problem(
                    &format!("{}.expected", side.name),
                    json!(want),
                    json!(got),
                ));
            }
        }
    }
    (l, r, problems)
}

/// The version without the build suffix the relay under test adds.
fn release(version: &Value) -> Value {
    match version.as_str() {
        Some(v) => json!(v.split_once("-heartwood.").map_or(v, |(head, _)| head)),
        None => version.clone(),
    }
}

/// A status answer without what is masked: the attestations (judged as "each verifies"), the
/// version's suffix, and the slot fields when the two answers fall in different slots.
fn mask_status(answer: &Value, other: &Value) -> Value {
    let mut out = answer.clone();
    if let Some(o) = out.as_object_mut() {
        o.remove("publicKeys");
        o.remove("signatures");
    }
    if let Some(v) = out.pointer_mut("/config/version") {
        *v = release(v);
    }
    let slot = |v: &Value| v.pointer("/state/currentSlot").cloned();
    if slot(answer) != slot(other)
        && let Some(state) = out.get_mut("state").and_then(Value::as_object_mut)
    {
        state.remove("currentSlot");
        state.remove("forgingAllowed");
    }
    out
}

/// Whether every attestation a record counts verified (a relay with no forger has none).
fn attestations_verify(record: &Value) -> Option<bool> {
    let a = record.get("attestations")?;
    Some(a.get("signed") == a.get("verified"))
}

/// A record reduced to what the comparison judges.
fn project(record: &Value, other: &Value, peers: &BTreeMap<usize, Vec<String>>) -> Value {
    let mut out = Map::new();
    for key in [
        "kind",
        "configValid",
        "ports",
        "configuredPorts",
        "accepted",
        "socket",
        "banned",
        "pingsSeen",
        "outcome",
        "probeError",
    ] {
        if let Some(v) = record.get(key) {
            out.insert(key.to_owned(), v.clone());
        }
    }
    // The hello's socket id differs by design (the reference's carries a time, a host and a
    // counter); its heartbeat is compared.
    if let Some(hb) = record.pointer("/hello/heartbeat") {
        out.insert("heartbeat".into(), hb.clone());
    }
    if let Some(n) = record.get("disconnects").and_then(Value::as_array) {
        out.insert("disconnects".into(), json!(n.len()));
    }
    if let Some(ok) = attestations_verify(record) {
        out.insert("attestationsVerify".into(), json!(ok));
    }
    if let Some(ban) = record.get("ban") {
        out.insert("ban".into(), ban.clone());
    }
    let calls = record
        .get("calls")
        .and_then(Value::as_array)
        .into_iter()
        .flatten();
    let others: Vec<&Value> = other
        .get("calls")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .collect();
    let mut projected = Vec::new();
    for (i, call) in calls.enumerate() {
        let mut c = Map::new();
        for key in [
            "endpoint",
            "requestSha256",
            "error",
            "verdict",
            "decodeError",
        ] {
            if let Some(v) = call.get(key).filter(|v| !v.is_null()) {
                c.insert(key.to_owned(), v.clone());
            }
        }
        let answer = call.get("answer");
        match (call.get("endpoint").and_then(Value::as_str), answer) {
            (Some(GET_STATUS), Some(a)) => {
                let theirs = others
                    .get(i)
                    .and_then(|o| o.get("answer"))
                    .unwrap_or(&Value::Null);
                c.insert("answer".into(), mask_status(a, theirs));
            }
            (Some(GET_PEERS), Some(_)) => {
                c.insert("answer".into(), json!(peers.get(&i)));
            }
            (_, Some(a)) => {
                c.insert("answer".into(), a.clone());
                c.extend(
                    call.get("responseSha256")
                        .map(|h| ("responseSha256".to_owned(), h.clone())),
                );
            }
            _ => {}
        }
        projected.push(Value::Object(c));
    }
    out.insert("calls".into(), Value::Array(projected));
    Value::Object(out)
}

fn peer_list(call: &Value) -> Vec<String> {
    let list = call
        .get("answer")
        .and_then(Value::as_array)
        .into_iter()
        .flatten();
    list.filter_map(|p| p.get("ip").and_then(Value::as_str).map(str::to_owned))
        .collect()
}

fn calls_of(record: &Value) -> Vec<&Value> {
    record
        .get("calls")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .collect()
}

fn judge(topology: &Topology, left: (&str, &Value), right: (&str, &Value)) -> Outcome {
    let (lname, l) = left;
    let (rname, r) = right;
    for (name, record) in [(lname, l), (rname, r)] {
        if let Some(error) = record.get("probeError") {
            return Outcome::HarnessError(format!("{name}: {error}"));
        }
    }
    let ip = |name: &str, record: &Value| {
        let recorded = record.get("ip").and_then(Value::as_str);
        recorded
            .or_else(|| topology.address(name))
            .unwrap_or("")
            .to_owned()
    };
    let (lip, rip) = (ip(lname, l), ip(rname, r));
    let mut extra = Vec::new();
    let (mut lpeers, mut rpeers) = (BTreeMap::new(), BTreeMap::new());
    for (i, (lc, rc)) in calls_of(l).into_iter().zip(calls_of(r)).enumerate() {
        if lc.get("endpoint").and_then(Value::as_str) != Some(GET_PEERS)
            || lc.get("answer").is_none()
            || rc.get("answer").is_none()
        {
            continue;
        }
        let ls = Side {
            name: lname,
            ip: &lip,
            peers: peer_list(lc),
        };
        let rs = Side {
            name: rname,
            ip: &rip,
            peers: peer_list(rc),
        };
        let (lm, rm, problems) = peers_rule(topology, &ls, &rs);
        extra.extend(problems);
        lpeers.insert(i, lm);
        rpeers.insert(i, rm);
    }
    let mut differences = diff(&project(r, l, &rpeers), &project(l, r, &lpeers));
    differences.extend(extra);
    if differences.is_empty() {
        Outcome::Pass
    } else {
        Outcome::Mismatch(differences)
    }
}

fn records(root: &Path, node: &str) -> Result<Vec<Value>> {
    let path = root.join(node).join("cases.jsonl");
    let lines = read_lines(&path)?;
    if lines.is_empty() {
        return Err(Error::Corpus {
            path,
            message: "no probe records".into(),
        });
    }
    Ok(lines)
}

/// Compare the probe's records of the nodes `left` and `right` of the run in `run_dir`.
pub fn compare(run_dir: &Path, left: &str, right: &str) -> Result<Report> {
    let root: PathBuf = if run_dir.join("p2p").is_dir() {
        run_dir.join("p2p")
    } else {
        run_dir.to_path_buf()
    };
    let topology = Topology::load(&root)?;
    let (lrecs, rrecs) = (records(&root, left)?, records(&root, right)?);
    let id = |r: &Value| {
        r.get("case")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned()
    };
    let mut results = Vec::new();
    let mut seen = BTreeSet::new();
    for (line, l) in lrecs.iter().enumerate() {
        seen.insert(id(l));
        let outcome = match rrecs.iter().find(|r| id(r) == id(l)) {
            Some(r) => judge(&topology, (left, l), (right, r)),
            None => Outcome::Mismatch(vec![Difference::Extra {
                path: format!("$.{}", id(l)),
                actual: json!(left),
            }]),
        };
        results.push(result(left, line + 1, &id(l), l, outcome));
    }
    for (line, r) in rrecs
        .iter()
        .enumerate()
        .filter(|(_, r)| !seen.contains(&id(r)))
    {
        let missing = Difference::Missing {
            path: format!("$.{}", id(r)),
            expected: json!(right),
        };
        results.push(result(
            right,
            line + 1,
            &id(r),
            r,
            Outcome::Mismatch(vec![missing]),
        ));
    }
    Ok(Report {
        title: "P2P probe".into(),
        source: format!(
            "{left} (actual) against {right} (expected), records under {}",
            root.display()
        ),
        files: 2,
        results,
    })
}

fn result(node: &str, line: usize, case: &str, record: &Value, outcome: Outcome) -> CaseResult {
    let kind = record.get("kind").and_then(Value::as_str).unwrap_or("case");
    CaseResult {
        file: format!("{node}/cases.jsonl"),
        line,
        op: format!("p2p.{kind}"),
        label: case.to_owned(),
        height: 0,
        outcome,
    }
}
