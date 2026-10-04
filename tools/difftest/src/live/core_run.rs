//! The core run's checks (`core`): three nodes, each one `core` process holding the relay, forger
//! and core locks; forging and attestations as a split node (the three nodes' delegates all forged,
//! every answer counted all 53 delegates); `relay start` refused while `core` runs; `devnet stop`
//! graceful within 30 seconds.
//!
//! The recipe records `locks.json` (for each node the process holding each lock file and the
//! process's children), the steps `locks`, `relay-start-refused` and `stop`, and the run's common
//! files. The chain is judged with every run's checks; here its delegates' blocks are read from the
//! nodes' block files as the mixed forging runs' are, and each block must follow, in its slot, an
//! answer to its own node's forger with a quorum. The resource series are recorded, not judged: the
//! budgets are the mixed forging runs' and the churn run's.

use std::path::Path;

use serde_json::Value;

use super::churn;
use super::forging;
use super::mixed;
use super::mixed_forging;
use super::run::{self, Manifest, Step};
use super::{Cases, ForgingRun, attestation, resources, schedule};
use crate::errors::Result;

const SECTION: &str = "core";

/// The seconds a graceful stop may take.
const STOP_LIMIT_SECONDS: f64 = 30.0;

/// The three lock files a `core` process holds in place of the two roles' processes.
const LOCKS: [&str; 3] = ["core.lock", "relay.lock", "forger.lock"];

/// The line a node logs when its pool is flushed and its store closed.
const STOPPED: &str = "The node has stopped";

/// One passed case when `problems` is empty, or a failure for each of the first few.
fn verdict(cases: &mut Cases, label: &str, problems: Vec<String>) {
    if problems.is_empty() {
        cases.pass(SECTION, label);
    }
    for p in problems.iter().take(12) {
        cases.fail(SECTION, label, p.clone());
    }
    if problems.len() > 12 {
        cases.fail(
            SECTION,
            label,
            format!("... and {} more", problems.len() - 12),
        );
    }
}

/// The problems of `locks.json`: every node has one `core` process holding every lock file and no
/// child.
fn lock_problems(manifest: &Manifest, json: &Value) -> Vec<String> {
    let mut problems = Vec::new();
    for node in &manifest.nodes {
        let entry = &json["nodes"][node.name.as_str()];
        let Some(core) = entry["corePid"].as_u64().filter(|p| *p > 0) else {
            problems.push(format!("{}: no core process recorded", node.name));
            continue;
        };
        for lock in LOCKS {
            let holders: Vec<u64> = entry["holders"][lock]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_u64)
                .collect();
            if holders != [core] {
                problems.push(format!(
                    "{}: {lock} is held by {holders:?}, not by its core process {core} alone",
                    node.name
                ));
            }
        }
        if entry["children"].as_u64() != Some(0) {
            problems.push(format!(
                "{}: the core process has children ({})",
                node.name, entry["children"]
            ));
        }
        let files = entry["runFiles"].as_array();
        for lock in LOCKS {
            if !files.is_some_and(|f| f.iter().any(|n| n == lock)) {
                problems.push(format!("{}: no {lock} in its run directory", node.name));
            }
        }
    }
    problems
}

/// The problems of the recipe's status snapshot: one process per node, a `core` one, online and
/// never restarted.
fn status_problems(manifest: &Manifest, json: &Value) -> Vec<String> {
    let mut problems = Vec::new();
    let nodes = json["nodes"].as_array().map_or(&[][..], Vec::as_slice);
    if nodes.len() != manifest.nodes.len() {
        problems.push(format!(
            "the status lists {} nodes, the run has {}",
            nodes.len(),
            manifest.nodes.len()
        ));
    }
    for n in nodes {
        let processes = n["processes"].as_array().map_or(&[][..], Vec::as_slice);
        let roles: Vec<&str> = processes
            .iter()
            .map(|p| p["role"].as_str().unwrap_or("?"))
            .collect();
        if roles != ["core"] {
            problems.push(format!(
                "node {}: its processes are {roles:?}, not one core process",
                n["node"]
            ));
        }
        if processes
            .iter()
            .any(|p| p["status"] != "online" || p["restarts"].as_u64() != Some(0))
        {
            problems.push(format!(
                "node {}: its core process is not online or restarted",
                n["node"]
            ));
        }
    }
    problems
}

/// The problems of the refused `relay start`: it exited with a failure and a message that names the
/// running core process (the reference's start aborts with its `<token>-core` process's name), and
/// no node's processes changed.
fn refusal_problems(step: Option<&Step>) -> Vec<String> {
    let Some(step) = step else {
        return vec!["no relay-start-refused step".into()];
    };
    let mut problems = Vec::new();
    if !step.ok {
        problems.push("the step relay-start-refused did not succeed".into());
    }
    if step.number("exit").is_none_or(|e| e == 0) {
        problems.push("`relay start` exited with success while core runs".into());
    }
    match step.text("message") {
        Some(m) if m.contains("-core") && m.contains("already running") => {}
        Some(m) if !m.trim().is_empty() => problems.push(format!(
            "`relay start` was refused with `{}`, which does not name the running core process",
            m.trim()
        )),
        _ => problems.push("`relay start` printed no refusal".into()),
    }
    match (step.text("pidsBefore"), step.text("pidsAfter")) {
        (Some(before), Some(after)) if !before.trim().is_empty() && before == after => {}
        _ => problems.push("the nodes' processes changed, or were not recorded".into()),
    }
    problems
}

/// The problems of the stop: it exited cleanly within the limit and left no process.
fn stop_problems(step: Option<&Step>) -> Vec<String> {
    let Some(step) = step else {
        return vec!["no stop step".into()];
    };
    let mut problems = Vec::new();
    if !step.ok || step.number("exit") != Some(0) {
        problems.push("`devnet stop` did not exit cleanly".into());
    }
    match step.record["seconds"].as_f64() {
        Some(s) if s <= STOP_LIMIT_SECONDS => {}
        Some(s) => problems.push(format!(
            "`devnet stop` took {s} seconds, more than {STOP_LIMIT_SECONDS}"
        )),
        None => problems.push("the stop records no duration".into()),
    }
    if step.text("remaining").is_some_and(|r| !r.trim().is_empty()) {
        problems.push("processes were left after the stop".into());
    }
    problems
}

/// The nodes whose log does not end with the line of a clean stop, logged during `devnet stop`.
fn log_problems(run_dir: &Path, manifest: &Manifest, stop: Option<&Step>) -> Vec<String> {
    let mut problems = Vec::new();
    let span = stop.and_then(|s| Some((s.start?, s.time("end")?)));
    for node in &manifest.nodes {
        let dir = run_dir.join("nodes").join(&node.name).join("log");
        let log = std::fs::read_dir(&dir).ok().and_then(|entries| {
            entries
                .filter_map(|e| e.ok().map(|e| e.path()))
                .find(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.ends_with("-core-current.log"))
                })
                .and_then(|p| std::fs::read_to_string(p).ok())
        });
        let Some(text) = log else {
            problems.push(format!("{}: no core log under log", node.name));
            continue;
        };
        let last = text
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("");
        if !last.contains(STOPPED) {
            problems.push(format!(
                "{}: its log does not end with a `{STOPPED}` line",
                node.name
            ));
        } else if !span.is_some_and(|(from, to)| {
            churn::log_time(last).is_some_and(|t| (from..=to).contains(&t))
        }) {
            problems.push(format!(
                "{}: its `{STOPPED}` line is not within `devnet stop`",
                node.name
            ));
        }
    }
    problems
}

/// Every block followed, in its slot, an answer with a quorum to its own node's forger.
fn split_problems(data: &mixed_forging::Data) -> Vec<String> {
    forging::quorum_before_blocks(&data.blocks, &data.answers, &data.validators, data.slots)
}

/// The core run's checks.
pub(super) fn check(run: &ForgingRun<'_>, cases: &mut Cases) -> Result<()> {
    let m = run.manifest;
    let steps = run::steps(run.run_dir)?;
    match run::read_json(&run.run_dir.join("locks.json")) {
        Ok(json) => verdict(
            cases,
            "each node runs one process holding the core, relay and forger locks",
            lock_problems(m, &json),
        ),
        Err(e) => cases.fail(SECTION, "locks.json", e.to_string()),
    }
    match run::read_json(&run.run_dir.join("status.json")) {
        Ok(json) => verdict(
            cases,
            "the status lists one online core process per node, none restarted",
            status_problems(m, &json),
        ),
        Err(e) => cases.fail(SECTION, "status.json", e.to_string()),
    }
    verdict(
        cases,
        "`relay start` was refused while core runs, and changed no process",
        refusal_problems(mixed::step(&steps, "relay-start-refused")),
    );
    match mixed_forging::load(run) {
        Ok(data) if data.problems.is_empty() => {
            verdict(
                cases,
                "every node's delegates forged in every round",
                mixed_forging::production(run, &data),
            );
            verdict(
                cases,
                "every block followed a quorum answer to its own node's forger in its slot",
                split_problems(&data),
            );
            verdict(
                cases,
                "every node counted all 53 delegates in one answer after height 10",
                mixed_forging::counts(run, &data, &steps),
            );
            cases.record(
                SECTION,
                forging::attestation_report(
                    &data.blocks,
                    &data.answers,
                    &data.validators,
                    data.slots,
                ),
            );
        }
        Ok(data) => {
            for p in data.problems.iter().take(6) {
                cases.fail(SECTION, "the nodes' block files and records", p.clone());
            }
        }
        Err(e) => cases.fail(SECTION, "the nodes' block files and records", e.to_string()),
    }
    verdict(cases, "one tip at the end", mixed_forging::one_tip(run));
    verdict(
        cases,
        "`devnet stop` stopped every node within 30 seconds",
        stop_problems(mixed::step(&steps, "stop")),
    );
    verdict(
        cases,
        "every node's log ends with the clean stop, logged during `devnet stop`",
        log_problems(run.run_dir, m, mixed::step(&steps, "stop")),
    );
    verdict(
        cases,
        "every step in its planned round and order",
        schedule::problems(&m.run, &steps, m.round_length, m.stop_height),
    );
    attestation::check(run, cases)?;
    resources::check(run, cases)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn step(name: &str, record: Value) -> Step {
        let mut record = record;
        record["step"] = json!(name);
        Step {
            name: name.into(),
            node: "all".into(),
            ok: record["ok"].as_bool().unwrap_or(true),
            start: Some(0),
            record,
        }
    }

    #[test]
    fn a_stop_over_thirty_seconds_or_with_processes_left_is_a_problem() {
        let ok = step("stop", json!({"exit": 0, "seconds": 0.45, "remaining": ""}));
        assert!(stop_problems(Some(&ok)).is_empty());
        let slow = step("stop", json!({"exit": 0, "seconds": 31.0, "remaining": ""}));
        assert_eq!(stop_problems(Some(&slow)).len(), 1);
        let left = step(
            "stop",
            json!({"exit": 0, "seconds": 2.0, "remaining": "41 42"}),
        );
        assert_eq!(stop_problems(Some(&left)).len(), 1);
        assert_eq!(stop_problems(None).len(), 1);
    }

    #[test]
    fn a_relay_start_that_succeeded_or_moved_a_process_is_a_problem() {
        let ok = step(
            "relay-start-refused",
            json!({"exit": 1, "message": "node1-devnet-core is already running", "pidsBefore": "1 2", "pidsAfter": "1 2"}),
        );
        assert!(refusal_problems(Some(&ok)).is_empty());
        let other = step(
            "relay-start-refused",
            json!({"exit": 1, "message": "no such network", "pidsBefore": "1 2", "pidsAfter": "1 2"}),
        );
        assert_eq!(refusal_problems(Some(&other)).len(), 1);
        let started = step(
            "relay-start-refused",
            json!({"exit": 0, "message": "started", "pidsBefore": "1 2", "pidsAfter": "1 2 3"}),
        );
        // A success, a message that is no refusal, and a new process.
        assert_eq!(refusal_problems(Some(&started)).len(), 3);
        assert_eq!(refusal_problems(None).len(), 1);
    }
}
