//! The attestation's timing (recorded, not gated): from each Heartwood relay's attestation record
//! (an offer, its signatures' attachment, each status served unsigned after the wait; written by a
//! harness build), the wait from an offer to its signatures and the unsigned statuses served, per
//! relay and round (a round with none says so), beside the network-state proxy: for each tip the
//! relay received (its log's receipt of the block at that height), the first answer of its
//! network-state record at that height that counted every delegate of the round, timed from the
//! receipt. A line that is not a record, or a record of a step that is not one of the three,
//! fails: the record is evidence, and a gap in it is a fault of the run's recording.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::Value;

use super::forging::{self, Answer, Receipt};
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

/// The network-state proxy of one relay: for each height it received a block at (its first
/// receipt), the first answer at that height counting all `delegates` (with or without quorum)
/// after the receipt, as milliseconds after it (minimum, median and maximum), and the tips with no
/// such answer.
fn proxy(receipts: &[&Receipt], answers: &[&Answer], delegates: u64) -> String {
    let mut tips: BTreeMap<u64, i64> = BTreeMap::new();
    for r in receipts {
        let at = tips.entry(r.height).or_insert(r.at);
        *at = (*at).min(r.at);
    }
    if tips.is_empty() {
        return "no receipt of a block in the relay's log".to_owned();
    }
    let mut delays: Vec<i64> = Vec::new();
    let mut without = 0_usize;
    for (height, received) in &tips {
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
            format!("{min} / {median} / {max} ms after the receipt (min / median / max)")
        }
        _ => "no answer counting them all".to_owned(),
    };
    format!(
        "the network-state proxy, the first answer counting all {delegates} delegates after each tip the relay received: {delay} over {} tips, {without} without one",
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
    // The proxy is recorded beside the record: a relay without answers or receipts says so.
    let (answers, _) = forging::answers(run.run_dir, m)?;
    let (receipts, _) = forging::receipts(run.run_dir, m);
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
        let mine = |relay: &str| relay == n.name;
        let receipts: Vec<&Receipt> = receipts.iter().filter(|r| mine(&r.relay)).collect();
        let answers: Vec<&Answer> = answers.iter().filter(|a| mine(&a.relay)).collect();
        cases.record(
            SECTION,
            format!(
                "{}: {}",
                n.name,
                proxy(&receipts, &answers, u64::from(m.round_length))
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

    fn receipt(at: i64, height: u64) -> Receipt {
        Receipt {
            relay: "node1".to_owned(),
            at,
            username: "genesis_1".to_owned(),
            height,
            id: String::new(),
            from: "127.0.0.1".to_owned(),
        }
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
    fn the_proxy_times_the_first_full_count_after_each_tip() {
        let receipts = [receipt(1_000, 2), receipt(9_000, 3), receipt(9_500, 3)];
        let answers = [
            // Before the receipt, a partial count, another height: none of them is the answer.
            answer(900, 2, 53, 0),
            answer(1_100, 2, 40, 0),
            answer(1_200, 3, 53, 0),
            answer(1_300, 2, 50, 3),
            answer(1_800, 2, 53, 0),
            answer(9_700, 3, 53, 0),
        ];
        let r: Vec<&Receipt> = receipts.iter().collect();
        let a: Vec<&Answer> = answers.iter().collect();
        assert!(
            proxy(&r, &a, 53).ends_with(
                "after each tip the relay received: 300 / 700 / 700 ms after the receipt (min / median / max) over 2 tips, 0 without one"
            ),
            "{}",
            proxy(&r, &a, 53)
        );
        let late = [receipt(1_000, 2), receipt(9_000, 4)];
        let r: Vec<&Receipt> = late.iter().collect();
        assert!(proxy(&r, &a, 53).ends_with("over 2 tips, 1 without one"));
        assert_eq!(
            proxy(&[], &a, 53),
            "no receipt of a block in the relay's log"
        );
    }

    #[test]
    fn a_signature_without_an_offer_has_no_wait() {
        let lines = [line(10, "signed", "a")];
        assert!(summary(&lines).ends_with("no offer with signatures"));
    }
}
