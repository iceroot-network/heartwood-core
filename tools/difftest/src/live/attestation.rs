//! The attestation's timing (recorded, not gated): from each Heartwood relay's attestation record
//! (an offer, its signatures' attachment, each status served unsigned after the wait; written by a
//! harness build), the wait from an offer to its signatures and the unsigned statuses served, per
//! relay and round (a round with none says so), beside the network-state proxy: for each tip the
//! node took (the first line of its relay's or core's file log that names a new block at that
//! height, received or forged), the first answer of its network-state record at that height that
//! counted every delegate of the round, timed from that line. A line that is not a record, or a record of a step that is not one of the three,
//! fails: the record is evidence, and a gap in it is a fault of the run's recording.

use std::collections::BTreeMap;
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
                .filter(|s| matches!(*s, "offered" | "signed" | "unsigned"));
            match (at, step, v["status"].as_str(), v["slot"].as_i64()) {
                (Some(at), Some(step), Some(status), Some(slot)) => lines.push(Line {
                    at,
                    step: step.to_owned(),
                    status: status.to_owned(),
                    slot,
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

/// The record's summary of one relay: offers, signatures attached, statuses served unsigned, and
/// the wait from an offer to its signatures (minimum, median and maximum, in milliseconds).
fn summary(lines: &[Line]) -> String {
    let count = |step: &str| lines.iter().filter(|l| l.step == step).count();
    // A status can be offered again after its signatures went on: a signature belongs to the offer
    // before it.
    let mut offered: BTreeMap<&str, i64> = BTreeMap::new();
    let mut waits: Vec<i64> = Vec::new();
    for l in lines {
        match l.step.as_str() {
            "offered" => {
                offered.insert(l.status.as_str(), l.at);
            }
            "signed" => waits.extend(offered.get(l.status.as_str()).map(|at| l.at - at)),
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
    format!(
        "{} offered, {} signed, {} served unsigned; the wait from an offer to its signatures {wait}",
        count("offered"),
        count("signed"),
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

/// The attestation record's report: one recorded case per Heartwood relay, and one for its
/// network-state proxy.
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
            cases.fail(
                SECTION,
                label,
                format!(
                    "{}: no attestation record (a harness build writes one)",
                    n.name
                ),
            );
            continue;
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
        }
    }

    #[test]
    fn a_signature_belongs_to_the_offer_before_it() {
        // The same status is offered again after its signatures went on.
        let lines = [
            line(1_000, "offered", "a"),
            line(1_400, "signed", "a"),
            line(9_000, "offered", "a"),
            line(9_250, "signed", "a"),
            line(9_300, "unsigned", "b"),
        ];
        assert_eq!(
            summary(&lines),
            "2 offered, 2 signed, 1 served unsigned; the wait from an offer to its signatures 250 / 400 / 400 ms (min / median / max)"
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

    #[test]
    fn a_signature_without_an_offer_has_no_wait() {
        let lines = [line(10, "signed", "a")];
        assert!(summary(&lines).ends_with("no offer with signatures"));
    }
}
