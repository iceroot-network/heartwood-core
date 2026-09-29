//! The checks of a forging devnet of Heartwood relays and forgers (runs `m` and `f`), besides the
//! checks every run has:
//!
//! - one chain: every relay's last `apply`, `reapply` or `genesis` line of each height equals every
//!   other relay's;
//! - processes (`processes.jsonl`, snapshots of rounds 2 and 5): each relay and forger under its own
//!   supervisor (its parent), no forger listening, relays listening on their node address only,
//!   every process with its `<token>-<network>-<role>` log;
//! - status polls (`status.jsonl`): at most 35 s apart, consistent at the end and at every poll
//!   outside a fault window, and a divergence gone by the next poll;
//! - attestations (each relay's `difftest/network-state.jsonl`, one line per getNetworkState answer,
//!   `{t, caller, status, nodeHeight, hasQuorum, noQuorum, delegates: {hasQuorum, noQuorum,
//!   noResponse}}`): every relay that answered after height 10 (in run `m` before node 2's forger
//!   stops, in run `f` in round 1) counted all 53 delegates in one answer at least; and every block
//!   of the relays' block files was preceded, in its own slot, by an answer to its delegate's forger
//!   at the height below it with `hasQuorum` at least 27;
//! - signing slots: no key has two different blocks for one slot among the blocks every relay
//!   applied (its block files, abandoned branches included) and the blocks a relay received
//!   directly from a forger (its debug log's receipt lines, whose time gives the slot);
//! - run `f`: the failover evidence, the supervised restarts, the outages and the missed slots.
//!
//! Each validator's delegates (`username`, `publicKey`) and its forger's address (`forgerAddress`)
//! come from `run.json`'s nodes.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use heartwood_crypto::utils::hex;
use heartwood_crypto::{Block, Milestones, Network};
use serde_json::Value;

use super::run::{self, Manifest, Window};
use super::{Cases, digest, nodes};
use crate::errors::{Error, Result};

/// The delegates a forger needs counted to forge (the quorum's 27 of 53).
pub const QUORUM: u64 = 27;
/// The slots a failover may take before the block reaches the relay.
const FAILOVER_SLOTS: i64 = 25;
/// The fault whose window is every node's: the three-node outage halts the chain.
const HALT: &str = "outage-three";

fn number(v: &Value, key: &str) -> u64 {
    v[key].as_u64().unwrap_or(0)
}

fn text<'a>(v: &'a Value, key: &str) -> &'a str {
    v[key].as_str().unwrap_or("")
}

fn time(v: &Value, key: &str) -> Option<i64> {
    run::parse_time(text(v, key))
}

/// The slots of the chain: its epoch and block time, in milliseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Slots {
    /// The epoch, milliseconds since the Unix epoch.
    pub epoch_ms: i64,
    /// The block time.
    pub slot_ms: i64,
}

impl Slots {
    /// The slot of the wall-clock time `t` (milliseconds since the Unix epoch).
    pub fn at(&self, t: i64) -> i64 {
        (t - self.epoch_ms).div_euclid(self.slot_ms.max(1))
    }
}

/// A validator of the run: its forger's address and its delegates' keys (hex) with their names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Validator {
    /// `nodeK`.
    pub name: String,
    /// The address its forger connects from.
    pub forger: String,
    /// Its delegates: public key to username.
    pub keys: BTreeMap<String, String>,
}

/// The validators of `run.json` in `dir`.
pub fn validators(dir: &Path) -> Result<Vec<Validator>> {
    let v = run::read_json(&dir.join("run.json"))?;
    let mut out = Vec::new();
    for n in v["nodes"].as_array().into_iter().flatten() {
        if text(n, "role") != "forger" {
            continue;
        }
        let keys = n["delegates"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|d| {
                (
                    text(d, "publicKey").to_owned(),
                    text(d, "username").to_owned(),
                )
            })
            .collect();
        out.push(Validator {
            name: text(n, "name").to_owned(),
            forger: text(n, "forgerAddress").to_owned(),
            keys,
        });
    }
    Ok(out)
}

/// A block of a relay's block files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recorded {
    /// Its id.
    pub id: String,
    /// Its height.
    pub height: u32,
    /// The slot of its timestamp.
    pub slot: i64,
    /// Its generator's public key.
    pub key: String,
}

/// The chain's milestones, from the control node's crypto files.
fn milestones(dir: &Path, manifest: &Manifest) -> Result<Milestones> {
    let crypto = run::crypto_dir(dir, &dir.join("nodes").join(&manifest.control));
    let read = |name: &str| {
        let p = crypto.join(name);
        std::fs::read_to_string(&p).map_err(|e| Error::io(p, e))
    };
    let network = Network::load(&read("network.json")?)?;
    Ok(Milestones::load(&read("milestones.json")?, &network)?)
}

/// The block files `blocks-NNNN.bin` of `dir`, in the order of the starts.
fn block_files(dir: &Path) -> Result<Vec<std::path::PathBuf>> {
    let mut files: Vec<(u32, std::path::PathBuf)> = std::fs::read_dir(dir)
        .map_err(|e| Error::io(dir, e))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter_map(|p| {
            let n = p
                .file_name()?
                .to_str()?
                .strip_prefix("blocks-")?
                .strip_suffix(".bin")?
                .parse()
                .ok()?;
            Some((n, p))
        })
        .collect();
    files.sort();
    Ok(files.into_iter().map(|(_, p)| p).collect())
}

/// Every block of every relay's block files (a torn last line dropped), and the problems of lines
/// whose height or id is not their bytes'.
pub fn recorded_blocks(
    dir: &Path,
    manifest: &Manifest,
    milestones: &Milestones,
) -> Result<(BTreeMap<String, Recorded>, Vec<String>)> {
    let mut blocks = BTreeMap::new();
    let mut problems = Vec::new();
    let block_time = i64::from(manifest.block_time.max(1));
    for n in &manifest.nodes {
        let files =
            block_files(&dir.join("nodes").join(&n.name).join("difftest")).unwrap_or_default();
        if files.is_empty() {
            problems.push(format!("{}: no block file", n.name));
        }
        for path in files {
            let bytes = std::fs::read(&path).map_err(|e| Error::io(&path, e))?;
            for (index, part) in bytes.split_inclusive(|b| *b == b'\n').enumerate() {
                if !part.ends_with(b"\n") {
                    break;
                }
                let place = format!("{} line {}", path.display(), index + 1);
                let Ok(v) = serde_json::from_slice::<Value>(part) else {
                    problems.push(format!("{place}: not a JSON line"));
                    continue;
                };
                let decoded = hex::decode(text(&v, "hex"))
                    .ok()
                    .and_then(|b| Block::decode(&b, milestones).ok());
                let Some(block) = decoded else {
                    problems.push(format!("{place}: no block bytes"));
                    continue;
                };
                let id = block.id().to_hex();
                if number(&v, "h") != u64::from(block.height()) || text(&v, "id") != id {
                    problems.push(format!("{place}: its height or id is not its bytes'"));
                    continue;
                }
                if block.height() < 2 {
                    continue;
                }
                let key = block.header().generator_public_key.to_hex();
                blocks.entry(id.clone()).or_insert(Recorded {
                    id,
                    height: block.height(),
                    slot: i64::from(block.header().timestamp) / block_time,
                    key,
                });
            }
        }
    }
    Ok((blocks, problems))
}

/// One answer of a relay's network-state record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    /// The relay that answered.
    pub relay: String,
    /// When, in milliseconds since the Unix epoch.
    pub at: i64,
    /// The caller's address (the forger's).
    pub caller: String,
    /// The relay's height, when it had one.
    pub node_height: Option<u64>,
    /// The delegates counted for the quorum.
    pub has_quorum: u64,
    /// The round's delegates in no list, when the answer lists them.
    pub no_response: Option<usize>,
}

/// The answers of every relay's `difftest/network-state.jsonl` (a torn last line dropped), and the
/// problems of lines that are not answers.
pub fn answers(dir: &Path, manifest: &Manifest) -> Result<(Vec<Answer>, Vec<String>)> {
    let (mut out, mut problems) = (Vec::new(), Vec::new());
    for n in &manifest.nodes {
        let path = dir
            .join("nodes")
            .join(&n.name)
            .join("difftest/network-state.jsonl");
        if !path.exists() {
            problems.push(format!("{}: no network-state record", n.name));
            continue;
        }
        let bytes = std::fs::read(&path).map_err(|e| Error::io(&path, e))?;
        for (i, part) in bytes.split_inclusive(|b| *b == b'\n').enumerate() {
            // A kill can leave a torn last line: dropped.
            if !part.ends_with(b"\n") || part.iter().all(u8::is_ascii_whitespace) {
                continue;
            }
            let v: Value = serde_json::from_slice(part).unwrap_or(Value::Null);
            let (Some(at), Some(has_quorum)) = (time(&v, "t"), v["hasQuorum"].as_u64()) else {
                problems.push(format!(
                    "{} line {}: no time or no hasQuorum count",
                    path.display(),
                    i + 1
                ));
                continue;
            };
            out.push(Answer {
                relay: n.name.clone(),
                at,
                caller: text(&v, "caller").to_owned(),
                node_height: v["nodeHeight"].as_u64(),
                has_quorum,
                no_response: v
                    .pointer("/delegates/noResponse")
                    .and_then(Value::as_array)
                    .map(Vec::len),
            });
        }
    }
    Ok((out, problems))
}

/// Every relay that answered after height 10 before `cutoff` (and, with `round_one`, at most at
/// the round's last height) counted all `delegates` in one answer at least: `hasQuorum` all of them
/// and `noResponse` empty. Every validator's relay must have answered then.
pub fn full_counts(
    answers: &[Answer],
    validators: &[Validator],
    cutoff: i64,
    round_one: Option<u64>,
    delegates: u64,
) -> Vec<String> {
    let mut problems = Vec::new();
    let mut relays: BTreeMap<&str, bool> = validators
        .iter()
        .map(|v| (v.name.as_str(), false))
        .collect();
    let early = answers.iter().filter(|a| {
        a.node_height
            .is_some_and(|h| h > 10 && round_one.is_none_or(|end| h <= end))
            && a.at < cutoff
    });
    for a in early {
        let full = a.has_quorum == delegates && a.no_response == Some(0);
        let seen = relays.entry(a.relay.as_str()).or_insert(false);
        *seen |= full;
    }
    for (relay, full) in relays {
        if !full {
            problems.push(format!(
                "{relay}: no answer after height 10 counts all {delegates} delegates"
            ));
        }
    }
    problems
}

/// Every recorded block was preceded, in its slot, by an answer to its delegate's forger at the
/// height below it with `hasQuorum` at least [`QUORUM`].
pub fn quorum_before_blocks(
    blocks: &BTreeMap<String, Recorded>,
    answers: &[Answer],
    validators: &[Validator],
    slots: Slots,
) -> Vec<String> {
    let mut problems = Vec::new();
    for b in blocks.values() {
        let Some(holder) = validators.iter().find(|v| v.keys.contains_key(&b.key)) else {
            problems.push(format!(
                "block {} at height {}: its generator {} is no validator's delegate",
                b.id, b.height, b.key
            ));
            continue;
        };
        let evidence = answers.iter().any(|a| {
            a.caller == holder.forger
                && slots.at(a.at) == b.slot
                && a.node_height == Some(u64::from(b.height) - 1)
                && a.has_quorum >= QUORUM
        });
        if !evidence {
            problems.push(format!(
                "block {} at height {} (slot {}): no answer to {}'s forger ({}) in its slot at height {} with hasQuorum at least {QUORUM}",
                b.id,
                b.height,
                b.slot,
                holder.name,
                holder.forger,
                b.height - 1
            ));
        }
    }
    problems
}

/// What the relays' answers to the forgers show of the attestation, for the record and not a
/// gate: how long into its slot the first answer with a quorum (at least [`QUORUM`]) reached the
/// forger of each block, at the height below it (minimum, median and maximum in milliseconds, and
/// the blocks that have none), and how many answers to a forger after height 10 counted fewer
/// delegates than the quorum, so served a status the forger could not use.
pub fn attestation_report(
    blocks: &BTreeMap<String, Recorded>,
    answers: &[Answer],
    validators: &[Validator],
    slots: Slots,
) -> String {
    let mut latencies: Vec<i64> = Vec::new();
    let mut without = 0_usize;
    for b in blocks.values() {
        let Some(holder) = validators.iter().find(|v| v.keys.contains_key(&b.key)) else {
            continue;
        };
        let first = answers
            .iter()
            .filter(|a| {
                a.caller == holder.forger
                    && slots.at(a.at) == b.slot
                    && a.node_height == Some(u64::from(b.height) - 1)
                    && a.has_quorum >= QUORUM
            })
            .map(|a| a.at)
            .min();
        match first {
            Some(at) => {
                latencies.push(at - (slots.epoch_ms + b.slot * slots.slot_ms.max(1)));
            }
            None => without += 1,
        }
    }
    latencies.sort_unstable();
    let latency = match (
        latencies.first(),
        latencies.get(latencies.len() / 2),
        latencies.last(),
    ) {
        (Some(min), Some(median), Some(max)) => {
            format!("{min} / {median} / {max} ms into the slot (min / median / max)")
        }
        _ => "no block with a quorum answer".to_owned(),
    };
    let to_forgers = |a: &&Answer| {
        a.node_height.is_some_and(|h| h > 10) && validators.iter().any(|v| v.forger == a.caller)
    };
    let served = answers.iter().filter(to_forgers).count();
    let unsigned = answers
        .iter()
        .filter(to_forgers)
        .filter(|a| a.has_quorum < QUORUM)
        .count();
    format!(
        "attestation: first quorum answer {latency} over {} blocks, {without} without one; \
         {unsigned} of {served} answers to forgers after height 10 counted fewer than {QUORUM} delegates",
        latencies.len()
    )
}

/// A block a relay's debug log says it received: `Received new block forged by delegate NAME (#R)
/// at height H`, `The id of the new block is ID`, `It contains N transactions and was received
/// from IP`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Receipt {
    /// The relay.
    pub relay: String,
    /// When, in milliseconds since the Unix epoch.
    pub at: i64,
    /// The delegate's username.
    pub username: String,
    /// The block's height.
    pub height: u64,
    /// The block's id.
    pub id: String,
    /// The address it came from.
    pub from: String,
}

/// The time of a log line `[YYYY-MM-DD HH:MM:SS.fff] ...`, read as UTC.
fn line_time(line: &str) -> Option<i64> {
    let inner = line.strip_prefix('[')?.split(']').next()?;
    run::parse_time(&format!("{inner}Z"))
}

/// The receipts of the log text `text` of `relay`.
pub fn receipts_of(relay: &str, text: &str) -> Vec<Receipt> {
    let mut out = Vec::new();
    let mut named: Option<(String, u64)> = None;
    let mut id: Option<String> = None;
    for line in text.lines() {
        if let Some(rest) = line.split("Received new block forged by delegate ").nth(1) {
            let username = rest.split(' ').next().unwrap_or("").to_owned();
            let height = rest
                .split(" at height ")
                .nth(1)
                .and_then(|h| h.split(' ').next())
                .map(|h| h.replace(',', ""))
                .and_then(|h| h.parse().ok());
            named = height.map(|h| (username, h));
            id = None;
        } else if let Some(rest) = line.split("The id of the new block is ").nth(1) {
            id = rest.split_whitespace().next().map(str::to_owned);
        } else if let Some(rest) = line.split(" and was received from ").nth(1)
            && line.contains("It contains ")
        {
            let from = rest.split_whitespace().next().unwrap_or("").to_owned();
            if let (Some((username, height)), Some(block), Some(at)) =
                (named.take(), id.take(), line_time(line))
            {
                out.push(Receipt {
                    relay: relay.to_owned(),
                    at,
                    username,
                    height,
                    id: block,
                    from,
                });
            }
        }
    }
    out
}

/// The receipts of every relay's logs (`nodes/<node>/log`, rotated files included), and the logs
/// that could not be read.
pub fn receipts(dir: &Path, manifest: &Manifest) -> (Vec<Receipt>, Vec<String>) {
    let (mut out, mut unread) = (Vec::new(), Vec::new());
    for n in &manifest.nodes {
        let log = dir.join("nodes").join(&n.name).join("log");
        let mut paths: Vec<_> = std::fs::read_dir(&log)
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| {
                p.file_name()
                    .and_then(|s| s.to_str())
                    .is_some_and(|s| s.contains("-relay-") && nodes::is_file_log(s))
            })
            .collect();
        paths.sort();
        for path in paths {
            match nodes::log_text(&path) {
                Ok(text) => out.extend(receipts_of(&n.name, &text)),
                Err(e) => unread.push(format!("{}: {e}", path.display())),
            }
        }
    }
    (out, unread)
}

/// No key has two different blocks for one slot: among `blocks` (the slot of their timestamp) and
/// the `receipts` directly from a validator's forger (the slot of the receipt's time, as a forger
/// signs only its current slot). Also returns how many direct receipts named a block no relay
/// applied.
pub fn signing_slots(
    blocks: &BTreeMap<String, Recorded>,
    receipts: &[Receipt],
    validators: &[Validator],
    slots: Slots,
) -> (Vec<String>, usize) {
    let mut by_slot: BTreeMap<(String, i64), BTreeSet<String>> = BTreeMap::new();
    for b in blocks.values() {
        by_slot
            .entry((b.key.clone(), b.slot))
            .or_default()
            .insert(b.id.clone());
    }
    let mut unapplied = BTreeSet::new();
    for r in receipts {
        let Some(holder) = validators.iter().find(|v| v.forger == r.from) else {
            continue;
        };
        let Some(key) = holder
            .keys
            .iter()
            .find(|(_, name)| **name == r.username)
            .map(|(k, _)| k.clone())
        else {
            continue;
        };
        if !blocks.contains_key(&r.id) {
            unapplied.insert(r.id.clone());
        }
        by_slot
            .entry((key, slots.at(r.at)))
            .or_default()
            .insert(r.id.clone());
    }
    let problems = by_slot
        .iter()
        .filter(|(_, ids)| ids.len() > 1)
        .map(|((key, slot), ids)| {
            format!(
                "the key {key} has {} different blocks in slot {slot}: {}",
                ids.len(),
                ids.iter().cloned().collect::<Vec<_>>().join(", ")
            )
        })
        .collect();
    (problems, unapplied.len())
}

/// Check snapshots taken in rounds two and five. Each child must have a distinct supervisor,
/// an observed parent relationship, a role log and only the relay may listen.
pub fn processes(manifest: &Manifest, snapshots: &[Value]) -> Vec<String> {
    let mut problems = Vec::new();
    for round in [2, 5] {
        let reports: Vec<_> = snapshots
            .iter()
            .filter(|r| number(r, "round") == round)
            .collect();
        if reports.is_empty() {
            problems.push(format!("no process report in round {round}"));
        }
        for report in reports {
            let mut roles = BTreeSet::new();
            let mut pids = BTreeSet::new();
            let mut supervisors = BTreeSet::new();
            let rows = report["processes"]
                .as_array()
                .map(Vec::as_slice)
                .unwrap_or_default();
            for p in rows {
                let (node, role) = (text(p, "node"), text(p, "role"));
                let Some(n) = manifest.node(node) else {
                    problems.push(format!("unknown process node {node}"));
                    continue;
                };
                if !matches!(role, "relay" | "forger")
                    || (role == "forger" && !n.forger)
                    || !roles.insert((node, role))
                {
                    problems.push(format!("unexpected or duplicate {node} {role}"));
                }
                let (pid, parent, supervisor) = (
                    number(p, "pid"),
                    number(p, "parent"),
                    number(p, "supervisor"),
                );
                if pid == 0
                    || supervisor == 0
                    || pid == supervisor
                    || parent != supervisor
                    || !pids.insert(pid)
                    || !supervisors.insert(supervisor)
                {
                    problems.push(format!("{node} {role}: no distinct parent supervisor"));
                }
                let expected_log = format!("{node}-{}-{role}-", manifest.network);
                if !text(p, "log").starts_with(&expected_log) || p["logExists"] != true {
                    problems.push(format!(
                        "{node} {role}: no log file named {expected_log}..."
                    ));
                }
                let Some(listeners) = p["listeners"].as_array() else {
                    problems.push(format!("{node} {role}: listeners not recorded"));
                    continue;
                };
                if role == "forger" && !listeners.is_empty() {
                    problems.push(format!("{node}: forger has a listening socket"));
                }
                if role == "relay"
                    && (listeners.is_empty()
                        || listeners
                            .iter()
                            .any(|l| text(l, "ip") != n.ip || number(l, "port") == 0))
                {
                    problems.push(format!(
                        "{node}: relay listeners are absent or not bound to {}",
                        n.ip
                    ));
                }
            }
            for n in &manifest.nodes {
                for role in ["relay", "forger"]
                    .into_iter()
                    .filter(|r| *r == "relay" || n.forger)
                {
                    if !roles.contains(&(n.name.as_str(), role)) {
                        problems.push(format!("round {round}: missing {} {role}", n.name));
                    }
                }
            }
            if pids.iter().any(|pid| supervisors.contains(pid)) {
                problems.push("a node process is another node's supervisor".into());
            }
        }
    }
    problems
}

/// A divergence must disappear by the next poll, and a consistent poll must contain equal tips.
/// Fault windows permit an inconsistent poll, but never missing, out-of-order or sparse evidence.
pub fn statuses(records: &[Value], windows: &[Window], nodes: usize) -> Vec<String> {
    let mut problems = Vec::new();
    let mut previous = None;
    let mut diverged = false;
    if records.len() < 2 {
        problems.push("fewer than two status polls".into());
    }
    for r in records {
        let Some(at) = time(r, "ts") else {
            problems.push("status poll without UTC time".into());
            continue;
        };
        if previous.is_some_and(|p| at <= p || at - p > 35_000) {
            problems.push(format!(
                "status polls are not ordered at most 35 seconds apart (at {})",
                run::format_time(at)
            ));
        }
        previous = Some(at);
        let in_window = windows.iter().any(|w| w.contains(at));
        let consistent = r["consistent"] == true;
        let tips = r["tips"].as_object();
        let mut equal = tips.is_some_and(|t| {
            t.len() == nodes
                && t.values().next().is_some_and(|first| {
                    number(first, "height") > 0
                        && !text(first, "id").is_empty()
                        && t.values().all(|v| v == first)
                })
        });
        if let Some(ids) = r["commonIds"].as_object() {
            equal = number(r, "commonHeight") > 0
                && !ids.is_empty()
                && tips.is_some_and(|t| t.len() == ids.len())
                && (in_window || ids.len() == nodes)
                && ids.values().next().is_some_and(|first| {
                    first.as_str().is_some_and(|s| !s.is_empty())
                        && ids.values().all(|id| id == first)
                });
        }
        if consistent && !equal {
            problems.push(format!(
                "the poll at {} is consistent with missing or unequal common-height block ids",
                run::format_time(at)
            ));
        }
        if !consistent && (diverged || !in_window) {
            problems.push(format!(
                "the poll at {} is not consistent outside a fault window or after one that was not",
                run::format_time(at)
            ));
        }
        diverged = !consistent;
    }
    if records.last().is_none_or(|r| r["consistent"] != true) {
        problems.push("the final status is not consistent".into());
    }
    problems
}

/// Compare the final applied chain of all relays directly, including reapply records.
/// The separate judge comparison checks reverted and restarted states too.
pub fn one_chain(dir: &Path, manifest: &Manifest) -> Result<Vec<String>> {
    let mut problems = Vec::new();
    let mut control: Option<BTreeMap<u32, Value>> = None;
    for n in &manifest.nodes {
        let d = digest::read(&dir.join("nodes").join(&n.name).join("difftest"))?;
        let mut applied = BTreeMap::new();
        for l in d.lines {
            if matches!(l.step(), "apply" | "reapply" | "genesis") {
                applied.insert(l.height(), l.value);
            }
        }
        if applied.is_empty() {
            problems.push(format!("{}: no applied chain", n.name));
        }
        match &control {
            Some(expected) => {
                let heights: BTreeSet<&u32> = expected.keys().chain(applied.keys()).collect();
                let differing: Vec<u32> = heights
                    .into_iter()
                    .filter(|h| match (expected.get(h), applied.get(h)) {
                        (Some(a), Some(b)) => {
                            !crate::restart::differing_keys(a, b, &crate::restart::SAME_STEP)
                                .is_empty()
                        }
                        _ => true,
                    })
                    .copied()
                    .collect();
                if let Some(first) = differing.first() {
                    problems.push(format!(
                        "{}: final applied chain diverges at height {first} ({} heights differ from {}'s)",
                        n.name,
                        differing.len(),
                        manifest.control
                    ));
                }
            }
            None => control = Some(applied),
        }
    }
    Ok(problems)
}

fn verdict(cases: &mut Cases, label: &str, problems: Vec<String>) {
    if problems.is_empty() {
        cases.pass("forging", label);
    }
    for p in problems.iter().take(20) {
        cases.fail("forging", label, p.clone());
    }
    if problems.len() > 20 {
        cases.fail(
            "forging",
            label,
            format!("... and {} more", problems.len() - 20),
        );
    }
}

/// The problems of `found`, or its error as the one problem.
fn or_problem(found: Result<Vec<String>>) -> Vec<String> {
    found.unwrap_or_else(|e| vec![e.to_string()])
}

pub(super) fn check(
    dir: &Path,
    manifest: &Manifest,
    windows: &[Window],
    cases: &mut Cases,
) -> Result<()> {
    if !matches!(manifest.run.as_str(), "m" | "f") {
        return Ok(());
    }
    verdict(
        cases,
        "one applied chain on every relay",
        or_problem(one_chain(dir, manifest)),
    );
    verdict(
        cases,
        "processes and listeners",
        or_problem(run::read_lines(&dir.join("processes.jsonl")).map(|r| processes(manifest, &r))),
    );
    verdict(
        cases,
        "status polls converge",
        or_problem(
            run::read_lines(&dir.join("status.jsonl"))
                .map(|r| statuses(&r, windows, manifest.nodes.len())),
        ),
    );
    let validators = validators(dir)?;
    let delegates: usize = validators.iter().map(|v| v.keys.len()).sum();
    let delegates_count = u64::from(manifest.round_length);
    if validators.len() != 5
        || u64::try_from(delegates).ok() != Some(delegates_count)
        || validators.iter().any(|v| v.forger.is_empty())
    {
        cases.fail(
            "forging",
            "the validators",
            "run.json does not name five validators with their forger's address and the round's delegates",
        );
        return Ok(());
    }
    let chain = milestones(dir, manifest).and_then(|m| {
        let params = m.at(1);
        let epoch_ms = heartwood_crypto::crypto::slots::epoch_unix_ms(params.epoch())
            .ok_or_else(|| Error::Usage("the chain's epoch is not a time".into()))?;
        let slot_ms = i64::try_from(params.block_time()).unwrap_or(8) * 1_000;
        Ok((m, Slots { epoch_ms, slot_ms }))
    });
    let (milestones, slots) = match chain {
        Ok(chain) => chain,
        Err(e) => {
            cases.fail("forging", "the chain's crypto files", e.to_string());
            return Ok(());
        }
    };
    let (answers, mut problems) = answers(dir, manifest)?;
    // Run m: before node 2's forger stops for the scripted votes; run f: round 1.
    let cutoff = run::steps(dir)?
        .iter()
        .find(|s| s.name == "stop-forger-node2")
        .and_then(|s| s.start)
        .unwrap_or(i64::MAX);
    let round_one = (manifest.run == "f").then_some(u64::from(manifest.round_length));
    problems.extend(full_counts(
        &answers,
        &validators,
        cutoff,
        round_one,
        delegates_count,
    ));
    verdict(cases, "attestations: all 53 delegates counted", problems);
    let (blocks, mut problems) = match recorded_blocks(dir, manifest, &milestones) {
        Ok(found) => found,
        Err(e) => (BTreeMap::new(), vec![e.to_string()]),
    };
    if blocks.is_empty() {
        problems.push("no block in the relays' block files".into());
    }
    problems.extend(quorum_before_blocks(&blocks, &answers, &validators, slots));
    verdict(
        cases,
        "attestations: a quorum answer before every block",
        problems,
    );
    cases.record(
        "forging",
        attestation_report(&blocks, &answers, &validators, slots),
    );
    let (receipts, unread) = receipts(dir, manifest);
    let direct = receipts
        .iter()
        .filter(|r| validators.iter().any(|v| v.forger == r.from))
        .count();
    let (mut problems, unapplied) = signing_slots(&blocks, &receipts, &validators, slots);
    problems.extend(unread.into_iter().map(|u| format!("unreadable log {u}")));
    if direct == 0 {
        problems.push("no relay's debug log shows a block received from a forger".into());
    }
    verdict(cases, "no slot with two blocks of one key", problems);
    cases.record(
        "forging",
        format!(
            "{} blocks applied, {direct} received directly from a forger, {unapplied} of those never applied",
            blocks.len()
        ),
    );
    if manifest.run == "f" {
        faults(dir, manifest, windows, &validators, cases)?;
    }
    Ok(())
}

/// Whether `relay`'s log shows the block `id` received from `forger` no earlier than the step's
/// start and within `FAILOVER_SLOTS` slots of it: the failover's evidence read from the relay's
/// own log instead of the recipe's record of it.
pub fn failover_received(
    receipts: &[Receipt],
    relay: &str,
    forger: &str,
    id: &str,
    start: Option<i64>,
    slot_ms: i64,
) -> bool {
    let Some(start) = start else {
        return false;
    };
    let until = start + FAILOVER_SLOTS * slot_ms;
    receipts.iter().any(|r| {
        r.relay == relay && r.from == forger && r.id == id && r.at >= start && r.at <= until
    })
}

/// The height each step started at, in the order the steps ran: the runner's recorded
/// `startHeight`, raised to the highest of the steps before it. The runner records its last
/// reading of the chain, which can lag a height wait that has already returned; the steps run one
/// after another, so none starts lower than a step before it.
pub fn start_heights(steps: &[run::Step]) -> Vec<(&str, u64)> {
    let mut floor = 0;
    (steps.iter())
        .map(|s| {
            floor = floor.max(number(&s.record, "startHeight"));
            (s.name.as_str(), floor)
        })
        .collect()
}

fn faults(
    dir: &Path,
    manifest: &Manifest,
    windows: &[Window],
    validators: &[Validator],
    cases: &mut Cases,
) -> Result<()> {
    let steps = run::steps(dir)?;
    let (receipts, unread) = receipts(dir, manifest);
    let mut problems: Vec<String> = unread
        .into_iter()
        .map(|u| format!("unreadable log {u}"))
        .collect();
    let slot_ms = i64::from(manifest.block_time) * 1_000;
    let forger_one = validators
        .iter()
        .find(|v| v.name == "node1")
        .map_or("", |v| v.forger.as_str());
    for (name, relay) in [
        ("failover-second", "node6"),
        ("failover-third", "node7"),
        ("failover-return", "node1"),
    ] {
        let step = steps.iter().find(|s| s.name == name);
        if step.is_none_or(|s| {
            let r = &s.record;
            !s.ok
                || text(r, "relay") != relay
                || text(r, "source") != forger_one
                || r["delegateNode"] != 1
                || text(r, "blockId").is_empty()
                || !text(r, "logLine").contains(forger_one)
                || r["heldSlots"].as_u64().is_none_or(|h| h > 25)
                || !failover_received(
                    &receipts,
                    relay,
                    forger_one,
                    text(r, "blockId"),
                    s.start,
                    slot_ms,
                )
        }) {
            problems.push(format!(
                "{name}: no block of a node-1 delegate received by {relay} from {forger_one} within 25 slots"
            ));
        }
    }
    for name in ["crash-forger-node2", "crash-relay-node3"] {
        if steps.iter().find(|s| s.name == name).is_none_or(|s| {
            number(&s.record, "restartAfter") <= number(&s.record, "restartBefore")
                || number(&s.record, "pidBefore") == number(&s.record, "pidAfter")
                || number(&s.record, "supervisor") == 0
                || number(&s.record, "supervisor") != number(&s.record, "supervisorAfter")
        }) {
            problems.push(format!(
                "{name}: no restart under the same supervisor with its counter raised"
            ));
        }
    }
    let round_of = |h: u64| h.saturating_sub(1) / u64::from(manifest.round_length) + 1;
    let started = start_heights(&steps);
    for (name, round) in [
        ("failover-second", 2),
        ("failover-third", 2),
        ("failover-return", 2),
        ("crash-forger-node2", 3),
        ("crash-relay-node3", 3),
        ("restart-node3", 3),
        ("recover-node4", 3),
        ("replay-node5", 3),
        ("outage-two", 4),
        ("outage-three", 4),
        ("resume", 4),
        ("fresh-node5", 5),
    ] {
        if (started.iter())
            .find(|(step, _)| *step == name)
            .is_none_or(|(_, height)| round_of(*height) != round)
        {
            problems.push(format!("{name}: did not start in round {round}"));
        }
    }
    for s in steps
        .iter()
        .filter(|s| matches!(s.name.as_str(), "outage-two" | "outage-three" | "resume"))
    {
        let (before, after) = (
            number(&s.record, "heightBefore"),
            number(&s.record, "heightAfter"),
        );
        let halted = s.name == HALT;
        if before == 0
            || (halted
                && (before != after
                    || number(&s.record, "heldSlots") < 6
                    || !text(&s.record, "logLine")
                        .contains("Not peered with enough delegates to get quorum")))
            || (!halted && after <= before)
        {
            problems.push(format!("{}: no halt or progress evidence", s.name));
        }
    }
    let missed_path = dir.join("missed-slots.jsonl");
    if !missed_path.exists() {
        problems.push("no missed-slot report".into());
    }
    let availability = run::read_lines(&dir.join("availability.jsonl"))?;
    if availability.is_empty() {
        problems.push("no readiness samples".into());
    }
    let first_round = u64::from(manifest.round_length);
    for m in run::read_lines(&missed_path)? {
        let node = text(&m, "node");
        let Some(at) = time(&m, "ts") else {
            problems.push(format!("{node}: a missed slot without a time"));
            continue;
        };
        // The first round's misses are the launch's (the forgers meet their relays).
        if number(&m, "height") <= first_round {
            continue;
        }
        let window = windows
            .iter()
            .find(|w| w.contains(at) && (w.concerns(node) || w.fault == HALT));
        if window.is_none() {
            problems.push(format!(
                "{node}: slot at {} missed outside its fault windows",
                run::format_time(at)
            ));
        }
        // Node 1's forger may miss a slot only while the relay it used was not ready.
        // The samples of the slot's first half, when the forger decides whether to forge.
        let samples: Vec<_> = availability
            .iter()
            .filter(|r| time(r, "ts").is_some_and(|t| t >= at && t < at + slot_ms / 2))
            .collect();
        if node == "node1"
            && window.is_none_or(|w| w.fault != HALT)
            && (samples.is_empty() || samples.iter().any(|s| s["forgers"][node]["ready"] != false))
        {
            problems.push(format!(
                "node1: slot at {} missed while the relay its forger used was ready",
                run::format_time(at)
            ));
        }
    }
    verdict(cases, "faults and failover", problems);
    Ok(())
}
