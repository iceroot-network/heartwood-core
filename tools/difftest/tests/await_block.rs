//! `difftest live await` on recorded fixtures: a run directory of a Heartwood relay (node 4) with
//! its await files, digest files and logs, beside two reference nodes' logs, and a stall of node 4's
//! job.
//!
//! The honest run has a bit over 90 lines of each exercised route per relay, waits of a few
//! milliseconds that drained, and one stall: node 4's job held from 10:16:40 for 12 s. Its waits end
//! `bound` (a status poll, a block post) or `closed` (a block poll whose client gave up), one wait ends
//! `drained` on the new tip, and requests after the commit carry it. Each other test changes one
//! thing.

use std::path::{Path, PathBuf};

use heartwood_difftest::live::await_block;
use heartwood_difftest::live::run::{format_time, parse_time};
use heartwood_difftest::report::{Outcome, Report};
use heartwood_difftest::store::ScratchDir;
use serde_json::{Value, json};

const RELAY: &str = "198.51.100.14";
/// The start of the run.
const BASE: &str = "2026-09-29T10:00:00.000Z";
/// When node 4's job is held, in milliseconds after the start (a block boundary).
const HOLD: i64 = 1_000_000;
/// How long the job is held.
const HELD: i64 = 12_000;
/// The block time.
const SLOT: i64 = 8_000;

fn base() -> i64 {
    parse_time(BASE).unwrap()
}

/// The tip at `t`, one block every 8 s, the held job's block landing at its commit.
fn tip_at(t: i64) -> u64 {
    let since = t - base();
    let since = if (HOLD..HOLD + HELD).contains(&since) {
        HOLD - 1
    } else {
        since
    };
    u64::try_from(1 + since.div_euclid(SLOT)).unwrap()
}

/// A record line of a wait that ended at `end_ms` after the start. As the relay writes it, `t` is
/// the time the request reached the wait, `wait` milliseconds before its end.
fn line(end_ms: i64, route: &str, wait: i64, end: &str, before: u64, after: u64) -> Value {
    json!({
        "t": format_time(base() + end_ms - wait),
        "route": route,
        "caller": "198.51.100.15",
        "waitMs": wait,
        "end": end,
        "tipBefore": before,
        "tipAfter": after,
    })
}

/// The lines a Heartwood relay records over a run that is quiet but for its blocks and the stall.
fn honest() -> Vec<Value> {
    let mut lines = Vec::new();
    for (r, route) in [
        "getStatus",
        "getBlocks",
        "getUnconfirmedTransactions",
        "getPeers",
    ]
    .iter()
    .enumerate()
    {
        for i in 0..100_i64 {
            let at = 10_000 + i * 16_000 + r as i64 * 1_000;
            if (HOLD - 5_000..=HOLD + HELD + 30_000).contains(&at) {
                continue;
            }
            if i % 7 == 0 {
                let before = tip_at(base() + at) - 1;
                lines.push(line(at, route, 5 + i % 30, "drained", before, before + 1));
            } else {
                let tip = tip_at(base() + at);
                lines.push(line(at, route, 0, "none", tip, tip));
            }
        }
    }
    lines
}

/// The stall's lines: node 4's job held from `HOLD` to `HOLD + HELD` on the tip before it.
fn stalled() -> Vec<Value> {
    let pre = tip_at(base() + HOLD - 1);
    vec![
        // Began 0.5 s and 1 s into the hold: a status poll and a block post reach the bound.
        line(HOLD + 8_510, "getStatus", 8_010, "bound", pre, pre),
        line(HOLD + 9_030, "getStatus", 8_030, "bound", pre, pre),
        line(HOLD + 8_320, "postBlock", 8_020, "bound", pre, pre),
        // A block poll whose client failed three times and closed its socket.
        line(HOLD + 8_500, "getBlocks", 6_500, "closed", pre, pre),
        // Began 5 s into the hold: drained by the commit, on the new tip.
        line(HOLD + HELD, "getStatus", 7_000, "drained", pre, pre + 1),
        line(HOLD + 13_000, "getStatus", 0, "none", pre + 1, pre + 1),
        line(HOLD + 21_000, "getStatus", 0, "none", pre + 1, pre + 1),
        line(HOLD + 30_000, "getPeers", 0, "none", pre + 2, pre + 2),
    ]
}

/// The recorded pieces of the run, which a test changes before it writes them.
struct Recorded {
    /// The await files of node 4, by start.
    awaits: Vec<(u32, Vec<Value>)>,
    /// The digest files' starts (each holds the heights up to 400).
    digests: Vec<u32>,
    /// The stall's fault line, when there is one.
    fault: Option<Value>,
    /// The lines of the first reference node's log naming node 4.
    timeouts: usize,
    /// The line of node 4's log that says the job was held.
    hold_logged: bool,
    /// The load samples.
    load: Vec<Value>,
}

impl Recorded {
    fn honest() -> Recorded {
        let mut lines = honest();
        lines.extend(stalled());
        Recorded {
            awaits: vec![(1, lines)],
            digests: vec![1],
            fault: Some(json!({
                "fault": "stall",
                "nodes": ["node4"],
                "start": format_time(base() + HOLD - 5_000),
                "recovered": format_time(base() + HOLD + HELD),
            })),
            timeouts: 3,
            hold_logged: true,
            load: vec![
                json!({"ts": format_time(base() + 500_000), "load1": 1.25}),
                json!({"ts": format_time(base() + 1_500_000), "load1": 2.5}),
            ],
        }
    }

    /// Change every line of the await files.
    fn edit(&mut self, f: impl Fn(&mut Vec<Value>)) {
        for (_, lines) in &mut self.awaits {
            f(lines);
        }
    }

    fn write(&self, dir: &Path) {
        let run = dir.join("w");
        let node = |name: &str, ip: &str, implementation: &str| json!({"name": name, "ip": ip, "implementation": implementation, "underTest": name == "node4"});
        write(
            &run.join("run.json"),
            &json!({
                "format": "heartwood-live-run/1", "run": "w", "network": "devnet",
                "seed": "await-fixture", "roundLength": 53, "blockTime": 8, "stopHeight": 212,
                "faultWindowSeconds": 30,
                "nodes": [
                    node("node1", "198.51.100.11", "reference"),
                    node("node4", RELAY, "heartwood"),
                    node("node5", "198.51.100.15", "reference"),
                ],
            })
            .to_string(),
        );
        let faults: Vec<String> = self.fault.iter().map(Value::to_string).collect();
        write(&run.join("faults.jsonl"), &faults.join("\n"));
        let difftest = run.join("nodes/node4/difftest");
        for start in &self.digests {
            let heights: Vec<String> = (1..=400)
                .map(|h| json!({"step": "apply", "h": h}).to_string())
                .collect();
            write(
                &difftest.join(format!("digest-{start:04}.jsonl")),
                &heights.join("\n"),
            );
        }
        for (start, lines) in &self.awaits {
            let text: Vec<String> = lines.iter().map(Value::to_string).collect();
            write(
                &difftest.join(format!("await-{start:04}.jsonl")),
                &text.join("\n"),
            );
        }
        if self.hold_logged {
            write(
                &run.join("nodes/node4/log/heartwood-relay-current.log"),
                &format!(
                    "[{}] DEBUG: difftest: holding the job for 12000 ms",
                    format_time(base() + HOLD)
                        .replace('T', " ")
                        .trim_end_matches('Z')
                ),
            );
        }
        let mut log = String::from("[2026-09-29 09:00:00.000] INFO: started\n");
        for i in 0..self.timeouts {
            let at = format_time(base() + HOLD + 5_000 + 2_000 * i as i64);
            log.push_str(&format!(
                "[{}] ERROR: Response error (peer {RELAY}/p2p.peer.getBlocks): timeout\n",
                at.replace('T', " ").trim_end_matches('Z')
            ));
        }
        write(&run.join("logs/node5/core-current.log"), &log);
        write(
            &run.join("logs/node1/core-current.log"),
            "[2026-09-29 09:00:00.000] INFO: started\n",
        );
        let load: Vec<String> = self.load.iter().map(Value::to_string).collect();
        write(&run.join("load.jsonl"), &load.join("\n"));
    }
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, format!("{text}\n")).unwrap();
}

/// The run directory of `recorded`, in a scratch directory that lives as long as the result.
fn run_of(recorded: &Recorded) -> (ScratchDir, PathBuf) {
    let scratch = ScratchDir::new("await-report").unwrap();
    recorded.write(scratch.path());
    let run = scratch.path().join("w");
    (scratch, run)
}

/// The report of `recorded`.
fn report_of(recorded: &Recorded) -> Report {
    let (_scratch, run) = run_of(recorded);
    await_block::report(&run).unwrap()
}

/// The failed cases: label and message.
fn failures(report: &Report) -> Vec<String> {
    report
        .results
        .iter()
        .filter(|r| r.outcome.is_failure())
        .map(|r| match &r.outcome {
            Outcome::HarnessError(m) => format!("{}: {m}", r.label),
            _ => r.label.clone(),
        })
        .collect()
}

fn recorded_lines(report: &Report) -> Vec<&str> {
    report
        .results
        .iter()
        .filter(|r| r.label.starts_with("recorded: "))
        .map(|r| r.label.as_str())
        .collect()
}

fn fails_on(report: &Report, needle: &str) {
    let failed = failures(report);
    assert!(
        failed.iter().any(|f| f.contains(needle)),
        "no failure names {needle:?}: {failed:#?}"
    );
}

#[test]
fn an_honest_run_with_a_stall_passes() {
    let report = report_of(&Recorded::honest());
    assert_eq!(failures(&report), Vec::<String>::new());
    assert!(report.passed() > 10, "{report:#?}");
    let recorded = recorded_lines(&report).join("\n");
    // The distribution per route, with the load beside the longest wait.
    assert!(recorded.contains("node4: getStatus: "), "{recorded}");
    assert!(recorded.contains("waits median"), "{recorded}");
    assert!(
        recorded.contains("load 2.50") || recorded.contains("load 1.25"),
        "{recorded}"
    );
    // The stall: what ended bound or closed, per route and its client's class.
    assert!(
        recorded.contains("postBlock 1 bound, 0 closed: answer awaited"),
        "{recorded}"
    );
    assert!(
        recorded.contains("getBlocks 0 bound, 1 closed"),
        "{recorded}"
    );
    assert!(
        recorded.contains("job held from 2026-09-29T10:16:40.000Z"),
        "{recorded}"
    );
    assert!(recorded.contains("node5 3"), "{recorded}");
    // The honest maximum is recorded, and the granularity stated.
    assert!(
        recorded.contains("longest honest wait of the run 33 ms"),
        "{recorded}"
    );
    assert!(recorded.contains("20 ms granularity"), "{recorded}");
    // The peer check of the stall is not built, which a strict run fails on.
    assert!(
        report
            .results
            .iter()
            .any(|r| r.outcome == Outcome::NoSubject
                && r.label
                    .contains("kept or regained it in the stall's window (not built)")),
        "{report:#?}"
    );
}

#[test]
fn a_bound_end_outside_the_stall_fails() {
    let mut recorded = Recorded::honest();
    let tip = tip_at(base() + 300_000);
    recorded.edit(|lines| {
        lines.push(line(300_000, "getStatus", 8_010, "bound", tip, tip));
    });
    let report = report_of(&recorded);
    fails_on(&report, "outside the stall every wait ended by the drain");
    fails_on(&report, "ended `bound` after 8010 ms");
}

#[test]
fn a_closed_end_outside_every_fault_window_fails() {
    let mut recorded = Recorded::honest();
    let tip = tip_at(base() + 300_000);
    recorded.edit(|lines| lines.push(line(300_000, "getStatus", 4, "closed", tip, tip)));
    fails_on(&report_of(&recorded), "`closed` outside a fault window");
}

#[test]
fn an_internal_route_fails() {
    let mut recorded = Recorded::honest();
    let tip = tip_at(base() + 300_000);
    recorded.edit(|lines| lines.push(line(300_000, "getCurrentRound", 0, "none", tip, tip)));
    let report = report_of(&recorded);
    fails_on(&report, "only public routes in the record");
    fails_on(&report, "getCurrentRound (internal)");
}

#[test]
fn an_empty_record_fails() {
    let mut recorded = Recorded::honest();
    recorded.edit(Vec::clear);
    let report = report_of(&recorded);
    for route in ["getStatus", "getBlocks", "getUnconfirmedTransactions"] {
        fails_on(&report, &format!("at least 80 {route} lines"));
    }
    fails_on(
        &report,
        "at least one wait outside the stall ended `drained`",
    );
    fails_on(&report, "no wait ended `bound` in the stall");
}

#[test]
fn a_start_without_its_file_fails() {
    let mut recorded = Recorded::honest();
    recorded.digests.push(2);
    let report = report_of(&recorded);
    fails_on(&report, "no await-0002.jsonl for start 2");
    // A relay built without the record has no file at all.
    let mut recorded = Recorded::honest();
    recorded.awaits.clear();
    fails_on(&report_of(&recorded), "no await-0001.jsonl for start 1");
}

#[test]
fn a_route_below_its_minimum_fails() {
    let mut recorded = Recorded::honest();
    recorded.edit(|lines| {
        let mut seen = 0;
        lines.retain(|l| {
            let is = l["route"] == "getBlocks" && l["end"] == "none";
            seen += usize::from(is);
            !(is && seen > 10)
        });
    });
    let report = report_of(&recorded);
    fails_on(&report, "at least 80 getBlocks lines");
    let failed = failures(&report);
    assert!(
        !failed.iter().any(|f| f.contains("getStatus lines")),
        "{failed:#?}"
    );
}

#[test]
fn a_stall_with_no_bound_end_fails() {
    let mut recorded = Recorded::honest();
    recorded.edit(|lines| lines.retain(|l| l["end"] != "bound"));
    fails_on(&report_of(&recorded), "no wait ended `bound` in the stall");
    // A run of W with no stall recorded fails too.
    let mut recorded = Recorded::honest();
    recorded.fault = None;
    fails_on(&report_of(&recorded), "the run stalled a relay's job");
}

#[test]
fn a_stall_wait_that_drained_early_or_on_the_old_tip_fails() {
    let pre = tip_at(base() + HOLD - 1);
    let mut recorded = Recorded::honest();
    // Began 11 s before the commit and ended drained: it should have reached the bound.
    recorded.edit(|lines| {
        lines.push(line(
            HOLD + HELD,
            "getStatus",
            11_000,
            "drained",
            pre,
            pre + 1,
        ))
    });
    fails_on(
        &report_of(&recorded),
        "began 11000 ms before the commit ended `drained`",
    );
    // Ended drained on the tip before the job.
    let mut recorded = Recorded::honest();
    recorded.edit(|lines| lines.push(line(HOLD + 6_000, "getStatus", 5_000, "drained", pre, pre)));
    fails_on(&report_of(&recorded), "`drained` on the tip before the job");
    // A request after the commit still carried the old tip.
    let mut recorded = Recorded::honest();
    recorded.edit(|lines| lines.push(line(HOLD + 15_000, "getStatus", 0, "none", pre, pre)));
    fails_on(
        &report_of(&recorded),
        "after the commit carried the tip before the job",
    );
}

#[test]
fn a_bound_end_past_8_3_s_fails() {
    let pre = tip_at(base() + HOLD - 1);
    let mut recorded = Recorded::honest();
    recorded.edit(|lines| lines.push(line(HOLD + 9_400, "getStatus", 8_900, "bound", pre, pre)));
    fails_on(&report_of(&recorded), "not between 8.0 and 8.3 s");
}

#[test]
fn a_stall_may_have_no_closed_end_when_no_client_reached_three_timeouts() {
    let mut recorded = Recorded::honest();
    recorded.edit(|lines| lines.retain(|l| l["end"] != "closed"));
    // Three timeouts in the reference log and no closed end: the record is missing one.
    fails_on(&report_of(&recorded), "no wait ended `closed`");
    // Two timeouts only: no client closed its socket.
    recorded.timeouts = 2;
    let report = report_of(&recorded);
    assert_eq!(failures(&report), Vec::<String>::new());
    assert!(recorded_lines(&report).join("\n").contains("node5 2"));
}

#[test]
fn a_wait_ended_on_a_tip_the_relay_never_applied_fails() {
    let mut recorded = Recorded::honest();
    recorded.edit(|lines| lines.push(line(300_000, "getStatus", 20, "drained", 38, 900)));
    fails_on(&report_of(&recorded), "which the digest never applied");
}

#[test]
fn a_torn_last_line_is_dropped_and_recorded() {
    let recorded = Recorded::honest();
    let (scratch, run) = run_of(&recorded);
    let file = run.join("nodes/node4/difftest/await-0001.jsonl");
    let mut text = std::fs::read_to_string(&file).unwrap();
    text.push_str("{\"t\": \"2026-09-29T10:");
    std::fs::write(&file, text).unwrap();
    let report = await_block::report(&run).unwrap();
    assert_eq!(failures(&report), Vec::<String>::new());
    assert!(
        recorded_lines(&report)
            .join("\n")
            .contains("a torn last line of await-0001.jsonl")
    );
    drop(scratch);
}

#[test]
fn a_run_without_a_heartwood_relay_has_nothing_to_check() {
    let recorded = Recorded::honest();
    let (scratch, run) = run_of(&recorded);
    let text = std::fs::read_to_string(run.join("run.json"))
        .unwrap()
        .replace("\"heartwood\"", "\"reference\"");
    std::fs::write(run.join("run.json"), text).unwrap();
    let report = await_block::report(&run).unwrap();
    assert_eq!(failures(&report), Vec::<String>::new());
    assert!(recorded_lines(&report)[0].contains("no Heartwood relay"));
    drop(scratch);
}
