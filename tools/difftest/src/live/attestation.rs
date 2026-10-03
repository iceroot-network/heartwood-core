//! The attestation's timing (recorded, not gated): from each Heartwood relay's attestation record
//! (an offer, the attachment of its signatures when they come, each status a getStatus served
//! with them, each status served unsigned after the wait; written by a harness build), the wait
//! from an offer to the attachment of its signatures and the unsigned statuses served, per relay
//! and round (a round with none says so), beside the network-state proxy: for each tip the node
//! took (the first line of its relay's or core's file log that names a new block at that height,
//! received or forged), the first answer of its network-state record at that height that counted
//! every delegate of the round, timed from that line. A line that is not a record, or a record of
//! a step that is not one of the four, fails: the record is evidence, and a gap in it is a fault
//! of the run's recording. Only a relay whose forger runs on its host has offers to record: the
//! attestation session accepts a loopback source only, so the record of a relay whose forger
//! connects from another address (`forgerAddress` in `run.json`), or that has no forger, holds no
//! signature: a signature attached or a status served signed there fails (offers alone, which an
//! earlier relay recorded, are recorded as any other).

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde_json::Value;

use super::forging::{self, Answer};
use super::nodes;
use super::run::{self, Implementation};
use super::{Cases, ForgingRun};
use crate::errors::{Error, Result};

const SECTION: &str = "attestation";

/// One line of a start's attestation record.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Line {
    at: i64,
    step: String,
    status: String,
    slot: i64,
    /// The signatures attached or served.
    attestations: u64,
}

/// The lines of the record files `attest-NNNN.jsonl` in `dir` (a torn last line dropped) and the
/// problems of lines that are not records.
fn read(dir: &Path) -> Result<(Vec<Line>, Vec<String>)> {
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .map_err(|e| Error::io(dir, e))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("attest-") && n.ends_with(".jsonl"))
        })
        .collect();
    files.sort();
    let (mut lines, mut problems) = (Vec::new(), Vec::new());
    for path in files {
        let bytes = std::fs::read(&path).map_err(|e| Error::io(&path, e))?;
        for (i, part) in bytes.split_inclusive(|b| *b == b'\n').enumerate() {
            if !part.ends_with(b"\n") {
                break;
            }
            let v: Value = serde_json::from_slice(part).unwrap_or(Value::Null);
            let at = v["t"].as_str().and_then(run::parse_time);
            let step = v["step"]
                .as_str()
                .filter(|s| matches!(*s, "offered" | "signed" | "served" | "unsigned"));
            match (at, step, v["status"].as_str(), v["slot"].as_i64()) {
                (Some(at), Some(step), Some(status), Some(slot)) => lines.push(Line {
                    at,
                    step: step.to_owned(),
                    status: status.to_owned(),
                    slot,
                    attestations: v["attestations"].as_u64().unwrap_or(0),
                }),
                _ => problems.push(format!(
                    "{} line {}: not an attestation record",
                    path.display(),
                    i + 1
                )),
            }
        }
    }
    Ok((lines, problems))
}

/// The record's summary of one relay: offers; signatures attached, each offer counted once (an
/// offer with no key to sign it, whose empty signatures attach with it, and a second attachment
/// to one offer, counted apart); statuses served signed and served unsigned; and the wait from an
/// offer to the attachment of its signatures (minimum, median and maximum, in milliseconds).
fn summary(lines: &[Line]) -> String {
    let count = |step: &str| lines.iter().filter(|l| l.step == step).count();
    // A status can be offered again after its signatures went on: an attachment belongs to the
    // offer before it, and whether that offer has had one yet.
    let mut offered: BTreeMap<&str, (i64, bool)> = BTreeMap::new();
    let mut waits: Vec<i64> = Vec::new();
    let (mut unsigned_offers, mut again, mut no_key) = (0_usize, 0_usize, 0_usize);
    for l in lines {
        match l.step.as_str() {
            "offered" => {
                offered.insert(l.status.as_str(), (l.at, false));
            }
            "signed" => match offered.get_mut(l.status.as_str()) {
                Some((_, true)) => again += 1,
                Some((at, paired)) => {
                    *paired = true;
                    if l.attestations == 0 {
                        no_key += 1;
                    } else {
                        waits.push(l.at - *at);
                    }
                }
                None => unsigned_offers += 1,
            },
            _ => {}
        }
    }
    waits.sort_unstable();
    let wait = match (waits.first(), waits.get(waits.len() / 2), waits.last()) {
        (Some(min), Some(median), Some(max)) => {
            format!("{min} / {median} / {max} ms (min / median / max)")
        }
        _ => "no offer with signatures".to_owned(),
    };
    let mut apart = Vec::new();
    for (n, what) in [
        (no_key, "with no key to sign them"),
        (again, "attached again to one offer"),
        (unsigned_offers, "without an offer in the record"),
    ] {
        if n > 0 {
            apart.push(format!("{n} {what}"));
        }
    }
    let apart = if apart.is_empty() {
        String::new()
    } else {
        format!(" ({})", apart.join(", "))
    };
    format!(
        "{} offered, {} signed{apart}, {} served signed, {} served unsigned; the wait from an offer to the attachment of its signatures {wait}",
        count("offered"),
        waits.len(),
        count("served"),
        count("unsigned")
    )
}

/// The statuses served unsigned, by round of their slot (the chain's slots count from its epoch, so
/// a round is `round_slots` slots).
fn unsigned_by_round(lines: &[Line], round_slots: i64) -> BTreeMap<i64, usize> {
    let mut out = BTreeMap::new();
    for l in lines.iter().filter(|l| l.step == "unsigned") {
        *out.entry(l.slot.div_euclid(round_slots.max(1)) + 1)
            .or_default() += 1;
    }
    out
}

/// The height of a log line that names a new block the node took: `Received new block forged by
/// delegate NAME (#R) at height H` (a block from a peer, or from its own forger through the relay)
/// or `Delegate NAME forged a new block at height H` (a core node's own forger), at the line's
/// time (`[YYYY-MM-DD HH:MM:SS.fff]`, UTC).
fn tip_line(line: &str) -> Option<(u64, i64)> {
    if !(line.contains("Received new block forged by delegate ")
        || line.contains(" forged a new block at height "))
    {
        return None;
    }
    let height = line
        .split(" at height ")
        .nth(1)?
        .split(' ')
        .next()?
        .replace(',', "")
        .parse()
        .ok()?;
    let stamp = line.strip_prefix('[')?.split(']').next()?;
    Some((height, run::parse_time(&format!("{stamp}Z"))?))
}

/// When the node took each height: the earliest line of its relay's or core's file logs (rotated
/// files included) that names a new block at it.
fn tips(node_dir: &Path) -> BTreeMap<u64, i64> {
    let mut out: BTreeMap<u64, i64> = BTreeMap::new();
    let Ok(entries) = std::fs::read_dir(node_dir.join("log")) else {
        return out;
    };
    for path in entries.filter_map(|e| e.ok().map(|e| e.path())) {
        let wanted = path.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
            (n.contains("-relay-") || n.contains("-core-")) && nodes::is_file_log(n)
        });
        let Some(text) = wanted.then(|| nodes::log_text(&path).ok()).flatten() else {
            continue;
        };
        for (height, at) in text.lines().filter_map(tip_line) {
            let first = out.entry(height).or_insert(at);
            *first = (*first).min(at);
        }
    }
    out
}

/// The network-state proxy of one node: for each height it took (`tips`), the first answer at that
/// height counting all `delegates` (with or without quorum) after it, as milliseconds after it
/// (minimum, median and maximum), over the tips that had one. A forger asks only when its
/// delegates' slots come, so most tips have none.
fn proxy(tips: &BTreeMap<u64, i64>, answers: &[&Answer], delegates: u64) -> String {
    if tips.is_empty() {
        return "no line of a new block in the node's file logs".to_owned();
    }
    let mut delays: Vec<i64> = Vec::new();
    let mut without = 0_usize;
    for (height, received) in tips {
        let first = answers
            .iter()
            .filter(|a| {
                a.node_height == Some(*height)
                    && a.at >= *received
                    && a.has_quorum + a.no_quorum == delegates
            })
            .map(|a| a.at)
            .min();
        match first {
            Some(at) => delays.push(at - received),
            None => without += 1,
        }
    }
    delays.sort_unstable();
    let delay = match (delays.first(), delays.get(delays.len() / 2), delays.last()) {
        (Some(min), Some(median), Some(max)) => {
            format!("{min} / {median} / {max} ms after it (min / median / max)")
        }
        _ => "no answer counting them all".to_owned(),
    };
    format!(
        "the network-state proxy, the first answer counting all {delegates} delegates after the node took a tip: {delay}, at {} of the {} tips it took (the others had no such answer at their height)",
        tips.len() - without,
        tips.len()
    )
}

/// Why the relay `name` has no forger on its host, from the run's validators: `None` when one of
/// them is its forger and connects over loopback (a validator without a recorded address runs its
/// forger beside its relay, as the mixed runs do).
fn no_local_forger(validators: &[forging::Validator], name: &str) -> Option<String> {
    let Some(validator) = validators.iter().find(|v| v.name == name) else {
        return Some("it has no forger".to_owned());
    };
    let loopback = validator.forger.is_empty()
        || (validator.forger.parse::<std::net::IpAddr>()).is_ok_and(|ip| ip.is_loopback());
    (!loopback).then(|| {
        format!(
            "its forger connects from {}, not over loopback",
            validator.forger
        )
    })
}

/// The nodes of the run in `run_dir` that run the combined role (`combined` in `run.json`): their
/// forger runs in the relay's own process and reaches it over loopback, whatever address
/// `run.json` names for it.
fn combined_nodes(run_dir: &Path) -> Result<BTreeSet<String>> {
    let run = run::read_json(&run_dir.join("run.json"))?;
    Ok((run["nodes"].as_array().into_iter().flatten())
        .filter(|n| n["combined"].as_bool() == Some(true))
        .filter_map(|n| n["name"].as_str().map(str::to_owned))
        .collect())
}

/// The attestation record's report: one recorded case per Heartwood relay, and one for its
/// network-state proxy. The relay's attestation session accepts a loopback source only, so a
/// relay whose forger is on another host, or that has none, is offered no status: its empty
/// record is recorded as such, and a signature in its record fails, where a relay whose forger
/// connects over loopback (a combined node's always does) must have a record.
pub(super) fn check(run: &ForgingRun<'_>, cases: &mut Cases) -> Result<()> {
    let m = run.manifest;
    let label = "the attestation record's waits and unsigned statuses";
    let relays: Vec<_> = m
        .nodes
        .iter()
        .filter(|n| n.implementation == Implementation::Heartwood && !m.dry_run)
        .collect();
    if relays.is_empty() {
        cases.fail(
            SECTION,
            label,
            "no Heartwood relay in this mixed run (or a dry run): no attestation record to read",
        );
        return Ok(());
    }
    let validators = forging::validators(run.run_dir)?;
    let combined = combined_nodes(run.run_dir)?;
    let elsewhere = |name: &str| {
        if combined.contains(name) {
            None
        } else {
            no_local_forger(&validators, name)
        }
    };
    // The proxy is recorded beside the record: a node without answers or tips says so.
    let (answers, _) = forging::answers(run.run_dir, m)?;
    for n in relays {
        let dir = run.run_dir.join("nodes").join(&n.name).join("difftest");
        let (lines, problems) = match read(&dir) {
            Ok(read) => read,
            Err(e) => {
                cases.fail(SECTION, label, format!("{}: {e}", n.name));
                continue;
            }
        };
        for p in problems.iter().take(6) {
            cases.fail(SECTION, label, format!("{}: {p}", n.name));
        }
        if lines.is_empty() {
            match elsewhere(&n.name) {
                None => cases.fail(
                    SECTION,
                    label,
                    format!(
                        "{}: no attestation record (a harness build writes one)",
                        n.name
                    ),
                ),
                Some(why) => cases.record(
                    SECTION,
                    format!(
                        "{}: no local forger, so no attestation offered ({why}; the attestation session accepts a loopback source only)",
                        n.name
                    ),
                ),
            }
            continue;
        }
        if let Some(why) = elsewhere(&n.name) {
            let signed = (lines.iter())
                .filter(|l| l.step == "signed" || l.step == "served")
                .count();
            if signed > 0 {
                cases.fail(
                    SECTION,
                    label,
                    format!(
                        "{}: its forger is not on its host ({why}), yet its record holds {signed} signatures attached or statuses served signed: the attestation session accepts a loopback source only",
                        n.name
                    ),
                );
            }
        }
        let per_round = i64::from(m.round_length.max(1));
        let by_round = unsigned_by_round(&lines, per_round)
            .into_iter()
            .map(|(r, c)| format!("round {r}: {c}"))
            .collect::<Vec<_>>()
            .join(", ");
        let by_round = if by_round.is_empty() {
            "none in any round".to_owned()
        } else {
            by_round
        };
        cases.record(
            SECTION,
            format!(
                "{}: {}; unsigned by round of the status's slot: {by_round}",
                n.name,
                summary(&lines)
            ),
        );
        let answers: Vec<&Answer> = answers.iter().filter(|a| a.relay == n.name).collect();
        let tips = tips(&run.run_dir.join("nodes").join(&n.name));
        cases.record(
            SECTION,
            format!(
                "{}: {}",
                n.name,
                proxy(&tips, &answers, u64::from(m.round_length))
            ),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(at: i64, step: &str, status: &str) -> Line {
        Line {
            at,
            step: step.to_owned(),
            status: status.to_owned(),
            slot: 1,
            attestations: 3,
        }
    }

    #[test]
    fn a_signature_belongs_to_the_offer_before_it() {
        // The same status is offered again after its signatures went on.
        let lines = [
            line(1_000, "offered", "a"),
            line(1_400, "signed", "a"),
            line(2_400, "served", "a"),
            line(9_000, "offered", "a"),
            line(9_250, "signed", "a"),
            line(9_300, "unsigned", "b"),
        ];
        assert_eq!(
            summary(&lines),
            "2 offered, 2 signed, 1 served signed, 1 served unsigned; the wait from an offer to the attachment of its signatures 250 / 400 / 400 ms (min / median / max)"
        );
    }

    #[test]
    fn an_offer_is_counted_once_and_one_with_no_key_apart() {
        let none = Line {
            attestations: 0,
            ..line(5_000, "signed", "b")
        };
        let lines = [
            line(1_000, "offered", "a"),
            line(1_020, "signed", "a"),
            // A second attachment to the same offer is no second wait.
            line(1_900, "signed", "a"),
            line(5_000, "offered", "b"),
            none,
        ];
        assert_eq!(
            summary(&lines),
            "2 offered, 1 signed (1 with no key to sign them, 1 attached again to one offer), 0 served signed, 0 served unsigned; the wait from an offer to the attachment of its signatures 20 / 20 / 20 ms (min / median / max)"
        );
    }

    fn answer(at: i64, height: u64, has: u64, no: u64) -> Answer {
        Answer {
            relay: "node1".to_owned(),
            at,
            caller: "127.0.0.1".to_owned(),
            node_height: Some(height),
            has_quorum: has,
            no_quorum: no,
            status: Some(0),
            no_response: None,
        }
    }

    #[test]
    fn a_tip_is_read_from_a_received_or_a_forged_block_line_at_either_level() {
        assert_eq!(
            tip_line(
                "[1970-01-01 00:00:01.250] INFO: Received new block forged by delegate genesis_48 (#16) at height 2 with 1.9 dRT reward"
            ),
            Some((2, 1_250))
        );
        assert_eq!(
            tip_line(
                "[1970-01-01 00:00:02.000] INFO: Delegate genesis_15 forged a new block at height 1,006 with 0 transactions"
            ),
            Some((1_006, 2_000))
        );
        assert_eq!(
            tip_line("[1970-01-01 00:00:02.000] DEBUG: The id of the new block is ab"),
            None
        );
        assert_eq!(
            tip_line("Received new block forged by delegate x (#1) at height 3"),
            None
        );
    }

    #[test]
    fn the_proxy_times_the_first_full_count_after_each_tip() {
        let tips: BTreeMap<u64, i64> = [(2, 1_000), (3, 9_000)].into_iter().collect();
        let answers = [
            // Before the tip, a partial count, another height: none of them is the answer.
            answer(900, 2, 53, 0),
            answer(1_100, 2, 40, 0),
            answer(1_200, 3, 53, 0),
            answer(1_300, 2, 50, 3),
            answer(1_800, 2, 53, 0),
            answer(9_700, 3, 53, 0),
        ];
        let a: Vec<&Answer> = answers.iter().collect();
        assert!(
            proxy(&tips, &a, 53).ends_with(
                "after the node took a tip: 300 / 700 / 700 ms after it (min / median / max), at 2 of the 2 tips it took (the others had no such answer at their height)"
            ),
            "{}",
            proxy(&tips, &a, 53)
        );
        let late: BTreeMap<u64, i64> = [(2, 1_000), (4, 9_000)].into_iter().collect();
        assert!(
            proxy(&late, &a, 53).contains("at 1 of the 2 tips it took"),
            "{}",
            proxy(&late, &a, 53)
        );
        assert_eq!(
            proxy(&BTreeMap::new(), &a, 53),
            "no line of a new block in the node's file logs"
        );
    }

    /// A churn run's directory: four Heartwood relays whose forgers connect over loopback (one
    /// without a recorded address, as a mixed run names none), node 5 whose forger connects from
    /// another host, node 6 with no forger, and a reference relay; `attest` names the relays whose
    /// record holds an offer and its signatures.
    /// An offer and its signatures, as a relay with a loopback forger records them.
    const OFFERED_AND_SIGNED: &str = "{\"t\":\"2026-10-03T02:00:00.000Z\",\"step\":\"offered\",\"status\":\"a\",\"slot\":5}\n\
        {\"t\":\"2026-10-03T02:00:00.020Z\",\"step\":\"signed\",\"status\":\"a\",\"slot\":5,\"attestations\":11}\n";

    fn churn_run(attest: &[&str]) -> crate::vectors::testutil::TestDir {
        let records: Vec<(&str, &str)> = attest.iter().map(|n| (*n, OFFERED_AND_SIGNED)).collect();
        churn_run_with(&records)
    }

    /// Churn's node set, with the attestation record of each node of `records` (name, lines) and
    /// an empty one for every other Heartwood relay.
    fn churn_run_with(records: &[(&str, &str)]) -> crate::vectors::testutil::TestDir {
        let dir = crate::vectors::testutil::TestDir::new("live-attestation-churn");
        let validator = |name: &str, forger: Option<&str>| {
            let mut node = serde_json::json!({"name": name, "ip": format!("10.89.0.1{}", &name[4..]),
                "implementation": "heartwood", "role": "forger", "underTest": true,
                "delegates": [{"username": format!("genesis_{}", &name[4..]), "publicKey": "02aa"}]});
            if let Some(address) = forger {
                node["forgerAddress"] = serde_json::json!(address);
            }
            node
        };
        let nodes = serde_json::json!([
            validator("node1", Some("127.0.0.1")),
            validator("node2", Some("127.0.0.1")),
            validator("node3", Some("127.0.0.1")),
            validator("node4", None),
            validator("node5", Some("10.89.0.115")),
            {"name": "node6", "ip": "10.89.0.16", "implementation": "heartwood", "role": "relay",
                "underTest": true},
            {"name": "node7", "ip": "10.89.0.17", "implementation": "reference", "role": "relay"},
        ]);
        let manifest = serde_json::json!({"format": run::FORMAT, "run": "churn",
            "network": "churn-1", "seed": "1", "nodes": nodes, "status": "complete"});
        std::fs::write(dir.path().join("run.json"), manifest.to_string()).unwrap();
        for n in 1..=6 {
            let record = dir.path().join(format!("nodes/node{n}/difftest"));
            std::fs::create_dir_all(&record).unwrap();
            let name = format!("node{n}");
            let lines = (records.iter())
                .find(|(node, _)| *node == name)
                .map_or("", |(_, lines)| *lines);
            std::fs::write(record.join("attest-0001.jsonl"), lines).unwrap();
        }
        dir
    }

    /// The attestation cases of the run in `dir`: (label, failed).
    fn judged(dir: &Path) -> Vec<(String, bool)> {
        let manifest = run::Manifest::load(dir).unwrap();
        let oracle = || -> Result<crate::oracle::Oracle> { Err(Error::Usage("no oracle".into())) };
        let forging = ForgingRun {
            run_dir: dir,
            manifest: &manifest,
            windows: &[],
            corpus_root: dir,
            oracle: &oracle,
        };
        let mut cases = Cases {
            run: "churn".into(),
            results: Vec::new(),
        };
        check(&forging, &mut cases).unwrap();
        (cases.results.into_iter())
            .map(|r| {
                let failed = r.outcome.is_failure();
                let label = match r.outcome {
                    crate::report::Outcome::HarnessError(why) => format!("{}: {why}", r.label),
                    _ => r.label,
                };
                (label, failed)
            })
            .collect()
    }

    #[test]
    fn only_a_relay_whose_forger_is_on_its_host_is_offered_statuses() {
        // Churn's set: the four relays with a loopback forger recorded offers, nodes 5 and 6 none.
        let dir = churn_run(&["node1", "node2", "node3", "node4"]);
        let cases = judged(dir.path());
        assert!(cases.iter().all(|(_, failed)| !failed), "{cases:?}");
        for (node, why) in [
            ("node5", "its forger connects from 10.89.0.115"),
            ("node6", "it has no forger"),
        ] {
            assert!(
                cases.iter().any(|(label, _)| label.contains(&format!(
                    "{node}: no local forger, so no attestation offered ({why}"
                ))),
                "{node}: {cases:?}"
            );
        }
        for node in ["node1", "node2", "node3", "node4"] {
            assert!(
                cases
                    .iter()
                    .any(|(label, _)| label.starts_with(&format!("recorded: {node}: 1 offered"))),
                "{node}: {cases:?}"
            );
        }
        // A relay with a loopback forger and no record still fails, the unnamed address included.
        for missing in ["node1", "node4"] {
            let others: Vec<&str> = ["node1", "node2", "node3", "node4"]
                .into_iter()
                .filter(|n| *n != missing)
                .collect();
            let dir = churn_run(&others);
            let cases = judged(dir.path());
            assert!(
                cases.iter().any(|(label, failed)| *failed
                    && label.contains(&format!("{missing}: no attestation record"))),
                "{missing}: {cases:?}"
            );
        }
        // A record of offers of a relay whose forger is on another host, as an earlier relay
        // wrote one, is recorded as any other.
        let offered = "{\"t\":\"2026-10-03T02:00:00.000Z\",\"step\":\"offered\",\"status\":\"a\",\"slot\":5}\n";
        let loopback: Vec<(&str, &str)> = ["node1", "node2", "node3", "node4"]
            .map(|n| (n, OFFERED_AND_SIGNED))
            .to_vec();
        let mut with_offers = loopback.clone();
        with_offers.push(("node5", offered));
        let dir = churn_run_with(&with_offers);
        let cases = judged(dir.path());
        assert!(cases.iter().all(|(_, failed)| !failed), "{cases:?}");
        assert!(
            (cases.iter()).any(|(label, _)| label.starts_with("recorded: node5: 1 offered")),
            "{cases:?}"
        );
        // Signatures attached on such a relay, or statuses it served signed, fail: its session
        // accepts a loopback source only.
        for lines in [
            OFFERED_AND_SIGNED,
            "{\"t\":\"2026-10-03T02:00:01.000Z\",\"step\":\"served\",\"status\":\"a\",\"slot\":5,\"attestations\":11}\n",
        ] {
            let mut signed = loopback.clone();
            signed.push(("node5", lines));
            let dir = churn_run_with(&signed);
            let cases = judged(dir.path());
            assert!(
                cases.iter().any(|(label, failed)| *failed
                    && label.contains("node5: its forger is not on its host")),
                "{cases:?}"
            );
        }
    }

    #[test]
    fn a_signature_without_an_offer_has_no_wait() {
        let lines = [line(10, "signed", "a")];
        assert!(summary(&lines).ends_with("no offer with signatures"));
    }
}
