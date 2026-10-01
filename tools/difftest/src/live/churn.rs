//! The churn run's checks (`churn`): every scripted event with its evidence; missed slots only
//! inside fault windows; the supervised crashes restarted, no process errored; the two-node outage
//! forging, the three-node outage halting and resuming; the split-host forger's delegates forging
//! and a vote for one refused as offline; the relay-only node serving every route; the fresh
//! reference relay reaching the tip from one Heartwood relay alone, its judged digests equal to the
//! Heartwood lines; the pool filled to its limit and drained.
//!
//! Beside the checks of every run, the relay-only node's route corpus (its answers against the
//! reference relay's at the pause) is judged by status: the bodies of the delegate routes differ by
//! design, since the reference relay learns delegate versions from its one peer.
//!
//! The recipe restarts nodes 1 to 5 in turn (both roles, by the product's stop and start), kills a
//! relay and a forger under their supervisors, fills the transaction pool with the forgers paused,
//! stops two and then three nodes, and starts an empty reference relay that may only reach node 2.
//! Every event records its evidence in a step and its window in `faults.jsonl`; the chain is judged
//! with every run's checks, and its delegates' blocks are read from the nodes' block files as the
//! mixed forging runs' are.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::Value;

use super::forging::{self, Recorded};
use super::mixed;
use super::mixed_forging::{self, Data};
use super::run::{self, Manifest, Step, Window};
use super::{Cases, ForgingRun, attestation, schedule};
use crate::errors::Result;

const SECTION: &str = "churn";

/// The seconds after which the supervisor ends a stop by SIGKILL: a graceful stop takes less (the
/// recipe counts whole seconds, so a stop the SIGKILL ended reads 30 or more).
const STOP_LIMIT_SECONDS: u64 = 30;

/// The nodes restarted in turn in round 2.
const CYCLED: [&str; 5] = ["node1", "node2", "node3", "node4", "node5"];

/// The transactions the pool holds at its limit.
const POOL_LIMIT: u64 = 15_000;

/// The message of a forger that cannot reach enough delegates: the chain's halt.
const HALT_LINE: &str = "Not peered with enough delegates to get quorum";

/// The refusal of a vote for a delegate whose forger is on another host than its relay.
const OFFLINE: &str = "is not operating a node on the network";

/// The node that holds the delegate whose forger runs on its own address, and the node that only
/// relays.
const SPLIT: &str = "node5";
const SYNCED: &str = "node7";
const SEED_RELAY: &str = "node2";

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

/// The step `name`, or the problem that the run has none.
fn required<'s>(steps: &'s [Step], name: &str, problems: &mut Vec<String>) -> Option<&'s Step> {
    let step = mixed::step(steps, name);
    if step.is_none() {
        problems.push(format!("no {name} step"));
    }
    step
}

/// Whether the step's record holds `key` as `true`.
fn flag(step: &Step, key: &str) -> bool {
    step.record
        .get(key)
        .or_else(|| step.record.get("detail").and_then(|d| d.get(key)))
        .and_then(Value::as_bool)
        == Some(true)
}

/// The problems of the five restarts: each stop exited cleanly within the limit and left no
/// process, and the node answered and synced again.
fn cycle_problems(steps: &[Step]) -> Vec<String> {
    let mut problems = Vec::new();
    for node in CYCLED {
        let name = format!("cycle-{node}");
        let Some(s) = required(steps, &name, &mut problems) else {
            continue;
        };
        if !s.ok {
            problems.push(format!("{name}: the step did not succeed"));
        }
        if s.number("stopExit") != Some(0) {
            problems.push(format!("{name}: the stop did not exit cleanly"));
        }
        match s.number("stopSeconds") {
            Some(t) if t < STOP_LIMIT_SECONDS => {}
            Some(t) => problems.push(format!(
                "{name}: the stop took {t} seconds, not less than {STOP_LIMIT_SECONDS}: the supervisor's SIGKILL ended it"
            )),
            None => problems.push(format!("{name}: the stop records no duration")),
        }
        if s.text("remaining").is_some_and(|r| !r.trim().is_empty()) {
            problems.push(format!("{name}: processes were left after the stop"));
        }
        if !flag(s, "synced") || s.time("recovered").is_none() {
            problems.push(format!(
                "{name}: the node did not answer and sync again after its start"
            ));
        }
        if s.number("heightRecovered").is_none_or(|h| h == 0) {
            problems.push(format!("{name}: no height after the start"));
        }
    }
    problems
}

/// The line a relay logs when its stop has flushed the pool and closed the store.
const STOPPED_LINE: &str = "The node has stopped";

/// The relay's log, below a node's directory.
const RELAY_LOG: &str = "log/solar-testnet-relay-current.log";

/// The milliseconds since the epoch of a log line's `[YYYY-MM-DD HH:MM:SS.fff]` prefix (UTC).
pub(super) fn log_time(line: &str) -> Option<i64> {
    let stamp = line.strip_prefix('[')?.split_once(']')?.0;
    run::parse_time(&format!("{stamp}Z"))
}

/// The problems of the graceful stops: the harness cannot tell a supervisor the stop left alive and
/// then killed from one that exited, so each restarted relay must also have logged that its stop
/// completed, between the start and the end of its restart step.
fn stop_log_problems(run_dir: &Path, steps: &[Step]) -> Vec<String> {
    let mut problems = Vec::new();
    for node in CYCLED {
        let Some(s) = steps.iter().find(|s| s.name == format!("cycle-{node}")) else {
            continue;
        };
        let Some(start) = s.start else { continue };
        let end = s.time("end").unwrap_or(i64::MAX);
        let log = run_dir.join("nodes").join(node).join(RELAY_LOG);
        let Ok(text) = std::fs::read_to_string(&log) else {
            problems.push(format!("{node}: its relay's log is missing"));
            continue;
        };
        let stopped = text.lines().any(|l| {
            l.contains(STOPPED_LINE) && log_time(l).is_some_and(|t| (start..=end).contains(&t))
        });
        if !stopped {
            problems.push(format!(
                "{node}: its relay's log has no `{STOPPED_LINE}` line within its restart"
            ));
        }
    }
    problems
}

/// The problems of the two kills: the role's process was replaced under the same supervisor, its
/// restart counter raised.
fn crash_problems(steps: &[Step]) -> Vec<String> {
    let mut problems = Vec::new();
    for (name, role) in [
        ("crash-relay-node3", "relay"),
        ("crash-forger-node2", "forger"),
    ] {
        let Some(s) = required(steps, name, &mut problems) else {
            continue;
        };
        let n = |key: &str| s.number(key).unwrap_or(0);
        if !s.ok
            || s.text("role").as_deref() != Some(role)
            || n("restartAfter") <= n("restartBefore")
            || n("pidBefore") == 0
            || n("pidBefore") == n("pidAfter")
            || n("supervisor") == 0
            || n("supervisor") != n("supervisorAfter")
        {
            problems.push(format!(
                "{name}: no restart of the {role} under the same supervisor with its counter raised"
            ));
        }
    }
    problems
}

/// The problems of the outages: two nodes down and the chain forging on; three down and the chain
/// halted, with the forgers' own log line; all back and the chain forging again; both returning
/// nodes synced.
fn outage_problems(steps: &[Step]) -> Vec<String> {
    let mut problems = Vec::new();
    for name in ["outage-two", "outage-three", "resume"] {
        let Some(s) = required(steps, name, &mut problems) else {
            continue;
        };
        let (before, after) = (
            s.number("heightBefore").unwrap_or(0),
            s.number("heightAfter").unwrap_or(0),
        );
        let halted = name == "outage-three";
        let bad = before == 0
            || if halted {
                before != after
                    || s.number("heldSlots").unwrap_or(0) < 6
                    || !s.text("logLine").is_some_and(|l| l.contains(HALT_LINE))
            } else {
                after <= before
            };
        if bad || !s.ok {
            problems.push(format!(
                "{name}: no {} evidence (height {before} to {after})",
                if halted { "halt" } else { "progress" }
            ));
        }
    }
    for node in ["node4", "node5"] {
        let name = format!("return-{node}");
        if let Some(s) = required(steps, &name, &mut problems)
            && (!s.ok || !flag(s, "synced") || s.time("recovered").is_none())
        {
            problems.push(format!("{name}: the node did not answer and sync again"));
        }
    }
    problems
}

/// The problems of the pool's fill: every transaction accepted, the pool at its limit with the
/// forgers paused, and drained by the forgers afterwards.
fn pool_problems(steps: &[Step]) -> Vec<String> {
    let mut problems = Vec::new();
    let Some(s) = required(steps, "full-pool", &mut problems) else {
        return problems;
    };
    if !s.ok {
        problems.push("full-pool: the step did not succeed".into());
    }
    let (expected, accepted, peak) = (
        s.number("expected").unwrap_or(0),
        s.number("accepted").unwrap_or(0),
        s.number("peak").unwrap_or(0),
    );
    if expected != POOL_LIMIT {
        problems.push(format!(
            "full-pool: it filled {expected} transactions, the pool's limit is {POOL_LIMIT}"
        ));
    }
    if accepted != expected {
        problems.push(format!(
            "full-pool: {accepted} of {expected} transactions were accepted"
        ));
    }
    if peak < POOL_LIMIT {
        problems.push(format!(
            "full-pool: the pool peaked at {peak}, below its limit {POOL_LIMIT}"
        ));
    }
    if !flag(s, "drained") {
        problems.push("full-pool: the pool did not drain".into());
    }
    match (s.start, s.time("drainedAt")) {
        (Some(start), Some(drained)) if drained > start => {}
        _ => problems.push("full-pool: no drain time after the start".into()),
    }
    problems
}

/// The problems of the split host: the forger of `node5` runs at an address of its own, and the
/// vote for its delegate was refused because its relay holds no version of it.
fn split_problems(
    manifest: &Manifest,
    steps: &[Step],
    forger_address: Option<&str>,
) -> Vec<String> {
    let mut problems = Vec::new();
    match (manifest.node(SPLIT), forger_address) {
        (Some(n), Some(a)) if !a.is_empty() && a != n.ip && a != "127.0.0.1" => {}
        _ => problems.push(format!(
            "{SPLIT}: its forger does not run at an address of its own"
        )),
    }
    if let Some(s) = required(steps, "vote-node5", &mut problems) {
        let refused = s.text("outcome").as_deref() == Some("rejected")
            && s.text("message").is_some_and(|m| m.contains(OFFLINE));
        if !s.ok || !refused {
            problems.push(format!(
                "vote-node5: the vote for {SPLIT}'s delegate was not refused with `{OFFLINE}`"
            ));
        }
    }
    problems
}

/// The problems of the fresh reference relay: it reached the tip, and saw only `node2` among its
/// peers throughout.
fn sync_problems(manifest: &Manifest, steps: &[Step]) -> Vec<String> {
    let mut problems = Vec::new();
    let Some(s) = required(steps, "sync-node7", &mut problems) else {
        return problems;
    };
    let ip = mixed::ip_of(manifest, SEED_RELAY).unwrap_or("");
    if !s.ok {
        problems.push("sync-node7: the step did not succeed".into());
    }
    if s.number("heightSynced").is_none_or(|h| h == 0) {
        problems.push("sync-node7: no height after the sync".into());
    }
    let peers: Vec<&str> = s
        .record
        .get("peers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    if !peers.contains(&ip) || peers.iter().any(|p| *p != ip) {
        problems.push(format!(
            "sync-node7: its peers were {peers:?}, not {SEED_RELAY}'s address {ip} alone"
        ));
    }
    if manifest
        .node(SYNCED)
        .is_none_or(|n| n.under_test || n.forger)
    {
        problems.push(format!(
            "{SYNCED} is not a reference relay of the run, or it forges"
        ));
    }
    problems
}

/// Where the recipe records the route corpus of the relay-only node against the reference relay.
const ROUTES: &str = "api-node6";

/// The fewest answered routes a full route corpus holds (it sends about 1,900).
const MIN_ROUTES: usize = 1_000;

/// The problems of the relay-only node's routes: node 6 (left) and the reference relay (right)
/// answered the whole route corpus at the pause, and node 6 answered every route with the
/// reference's status or a documented difference. The bodies are not compared: the reference
/// relay learnt the delegates' versions from node 2 alone, so its delegate answers differ by
/// design.
fn route_problems(run_dir: &Path) -> Vec<String> {
    let dir = run_dir.join(ROUTES);
    let mut problems = Vec::new();
    match run::read_json(&dir.join("route-record.json")) {
        Ok(r) if r["left"] == "node6" && r["right"] == SYNCED => {}
        Ok(r) => problems.push(format!(
            "the route corpus compared {} with {}, not node6 with {SYNCED}",
            r["left"], r["right"]
        )),
        Err(e) => problems.push(format!("no route corpus of node6: {e}")),
    }
    let text = match std::fs::read_to_string(dir.join("answers.jsonl")) {
        Ok(t) => t,
        Err(e) => {
            problems.push(format!("no answers of node6's route corpus: {e}"));
            return problems;
        }
    };
    let mut answered = 0_usize;
    for (index, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(record) = serde_json::from_str::<Value>(line) else {
            problems.push(format!("line {} of the answers is not JSON", index + 1));
            continue;
        };
        if record.get("skipped").is_some() {
            continue;
        }
        let name = record["name"].as_str().unwrap_or("?");
        let (left, right) = (
            record["left"]["status"].as_u64(),
            record["right"]["status"].as_u64(),
        );
        match (left, right) {
            (Some(l), Some(r)) if l == r || record.get("documented").is_some() => answered += 1,
            (Some(l), Some(r)) => problems.push(format!(
                "{name}: node6 answered {l}, the reference relay {r}"
            )),
            (None, _) => problems.push(format!("{name}: node6 gave no answer")),
            (Some(_), None) => problems.push(format!("{name}: the reference relay gave no answer")),
        }
    }
    if answered < MIN_ROUTES {
        problems.push(format!(
            "node6 answered {answered} routes, a full corpus answers at least {MIN_ROUTES}"
        ));
    }
    problems
}

/// The problems of the processes' final status: every role of every Heartwood node online and
/// stable, and none errored.
fn status_problems(manifest: &Manifest, json: &Value) -> Vec<String> {
    let mut problems = Vec::new();
    let nodes = json["nodes"].as_array().map_or(&[][..], Vec::as_slice);
    let heartwood = mixed::heartwood(manifest);
    if nodes.len() != heartwood.len() {
        problems.push(format!(
            "the status lists {} nodes, the run has {} Heartwood nodes",
            nodes.len(),
            heartwood.len()
        ));
    }
    for n in &heartwood {
        let Some(entry) = nodes.iter().find(|e| e["node"] == n.name.as_str()) else {
            problems.push(format!("{}: not in the status", n.name));
            continue;
        };
        let processes = entry["processes"].as_array().map_or(&[][..], Vec::as_slice);
        let roles: Vec<&str> = processes
            .iter()
            .map(|p| p["role"].as_str().unwrap_or("?"))
            .collect();
        let want: &[&str] = if n.forger {
            &["relay", "forger"]
        } else {
            &["relay"]
        };
        if roles != want {
            problems.push(format!(
                "{}: its processes are {roles:?}, not {want:?}",
                n.name
            ));
        }
        for p in processes {
            if p["status"] != "online" || p["unstable"] == true {
                problems.push(format!(
                    "{}: its {} is {} ({})",
                    n.name,
                    p["role"].as_str().unwrap_or("?"),
                    p["status"].as_str().unwrap_or("not online"),
                    if p["unstable"] == true {
                        "unstable"
                    } else {
                        "stable"
                    }
                ));
            }
        }
    }
    problems
}

/// Whether the run has a window of `fault` that concerns every node of `nodes`.
fn has_window(windows: &[Window], fault: &str, nodes: &[&str]) -> bool {
    windows
        .iter()
        .any(|w| w.fault == fault && nodes.iter().all(|n| w.nodes.iter().any(|m| m == n)))
}

/// The problems of the fault windows: every scripted event has its own.
fn window_problems(windows: &[Window]) -> Vec<String> {
    let mut problems = Vec::new();
    let mut need = |fault: &str, nodes: &[&str]| {
        if !has_window(windows, fault, nodes) {
            problems.push(format!("no `{fault}` window for {}", nodes.join(", ")));
        }
    };
    for node in CYCLED {
        need("restart", &[node]);
    }
    need("supervised-relay", &["node3"]);
    need("supervised-forger", &["node2"]);
    need("pool-pause", &CYCLED);
    need("outage-two", &["node4", "node5"]);
    need("outage-three", &["node3"]);
    need("fresh-sync", &[SYNCED]);
    problems
}

/// Every slot without a block, between two blocks of the chain, lies inside a fault window: a
/// slot is missed only while a scripted event holds a node down.
fn missed_problems(run: &ForgingRun<'_>, data: &Data) -> Vec<String> {
    let mut by_height: BTreeMap<u32, &Recorded> = BTreeMap::new();
    for b in data.blocks.values() {
        by_height.entry(b.height).or_insert(b);
    }
    let mut problems = Vec::new();
    let mut missed = 0_usize;
    let blocks: Vec<&&Recorded> = by_height.values().collect();
    for pair in blocks.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        if b.height != a.height + 1 {
            continue;
        }
        for slot in a.slot + 1..b.slot {
            let t = data.slot_start(slot);
            if !run.windows.iter().any(|w| w.contains(t)) {
                missed += 1;
                if problems.len() < 10 {
                    problems.push(format!(
                        "slot {slot} at {} between heights {} and {} has no block and lies in no fault window",
                        run::format_time(t),
                        a.height,
                        b.height
                    ));
                }
            }
        }
    }
    if missed > problems.len() {
        problems.push(format!(
            "... and {} more missed slots",
            missed - problems.len()
        ));
    }
    problems
}

/// Every validator's relay answered once after height 10 counting every delegate whose forger
/// reaches the relays (`hasQuorum`) and none without quorum. The split host's delegates, whose
/// forger calls from an address of its own, are the only ones a relay lists as not responding.
fn counted(data: &Data) -> Vec<String> {
    let split: usize = data
        .validators
        .iter()
        .filter(|v| v.name == SPLIT)
        .map(|v| v.keys.len())
        .sum();
    let others: u64 = data
        .validators
        .iter()
        .filter(|v| v.name != SPLIT)
        .map(|v| v.keys.len() as u64)
        .sum();
    data.validators
        .iter()
        .filter(|v| {
            !data.answers.iter().any(|a| {
                a.relay == v.name
                    && a.node_height.is_some_and(|h| h > 10)
                    && a.has_quorum == others
                    && a.no_response == Some(split)
            })
        })
        .map(|v| {
            format!(
                "{}: no answer after height 10 counts the {others} delegates of nodes 1 to 4 and lists only node 5's {split} as not responding",
                v.name
            )
        })
        .collect()
}

/// The churn run's checks.
pub(super) fn check(run: &ForgingRun<'_>, cases: &mut Cases) -> Result<()> {
    let m = run.manifest;
    let steps = run::steps(run.run_dir)?;
    verdict(
        cases,
        "nodes 1 to 5 each stopped gracefully within 30 seconds, left no process and started again synced",
        cycle_problems(&steps),
    );
    verdict(
        cases,
        "each restarted relay logged that its stop completed",
        stop_log_problems(run.run_dir, &steps),
    );
    verdict(
        cases,
        "the killed relay and forger restarted under their own supervisors, their counters raised",
        crash_problems(&steps),
    );
    verdict(
        cases,
        "two nodes down: the chain forged on; three down: it halted on the forgers' own line and forged again after the start; the nodes returned",
        outage_problems(&steps),
    );
    verdict(
        cases,
        "the pool held 15000 transactions with the forgers paused, then drained",
        pool_problems(&steps),
    );
    let forger_address = run::read_json(&run.run_dir.join("run.json"))
        .ok()
        .and_then(|j| {
            j["nodes"]
                .as_array()?
                .iter()
                .find(|n| n["name"] == SPLIT)?
                .get("forgerAddress")?
                .as_str()
                .map(str::to_owned)
        });
    verdict(
        cases,
        "node 5's forger runs at an address of its own, and the vote for its delegate was refused as offline",
        split_problems(m, &steps, forger_address.as_deref()),
    );
    verdict(
        cases,
        "the empty reference relay synced from node 2's relay alone",
        sync_problems(m, &steps),
    );
    match run::read_json(&run.run_dir.join("status.json")) {
        Ok(json) => verdict(
            cases,
            "every Heartwood node's roles end online and stable, none errored",
            status_problems(m, &json),
        ),
        Err(e) => cases.fail(SECTION, "status.json", e.to_string()),
    }
    verdict(
        cases,
        "the relay-only node 6 answered every route with the reference relay's status or a documented difference",
        route_problems(run.run_dir),
    );
    verdict(
        cases,
        "every scripted event has its fault window",
        window_problems(run.windows),
    );
    match mixed_forging::load(run) {
        Ok(data) if data.problems.is_empty() => {
            verdict(
                cases,
                "every delegate's node forged in every round outside its fault windows",
                mixed_forging::production(run, &data),
            );
            verdict(
                cases,
                "no slot was missed outside a fault window",
                missed_problems(run, &data),
            );
            verdict(
                cases,
                "every block followed a quorum answer to its own node's forger in its slot",
                forging::quorum_before_blocks(
                    &data.blocks,
                    &data.answers,
                    &data.validators,
                    data.slots,
                ),
            );
            verdict(
                cases,
                "every relay counted the delegates of nodes 1 to 4 in one answer after height 10, node 5's alone not responding",
                counted(&data),
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
        "every step in its planned round and order",
        schedule::problems(&m.run, &steps, m.round_length, m.stop_height),
    );
    attestation::check(run, cases)
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
            start: Some(1_000),
            record,
        }
    }

    fn cycles() -> Vec<Step> {
        CYCLED
            .iter()
            .map(|n| {
                step(
                    &format!("cycle-{n}"),
                    json!({"stopExit": 0, "stopSeconds": 4, "remaining": "", "synced": true,
                        "recovered": "2026-01-01T00:00:10.000Z", "heightRecovered": 60}),
                )
            })
            .collect()
    }

    #[test]
    fn a_slow_or_unclean_stop_or_a_node_not_synced_again_is_a_problem() {
        assert!(cycle_problems(&cycles()).is_empty());
        let mut steps = cycles();
        steps[1].record["stopSeconds"] = json!(30);
        steps[2].record["remaining"] = json!("pids 41 42");
        steps[3].record["synced"] = json!(false);
        steps.remove(4);
        let found = cycle_problems(&steps);
        assert_eq!(found.len(), 4, "{found:?}");
    }

    #[test]
    fn a_kill_needs_the_same_supervisor_and_a_raised_counter() {
        let ok = |name: &str, role: &str| {
            step(
                name,
                json!({"role": role, "restartBefore": 0, "restartAfter": 1, "pidBefore": 5,
                    "pidAfter": 9, "supervisor": 3, "supervisorAfter": 3}),
            )
        };
        let mut steps = vec![
            ok("crash-relay-node3", "relay"),
            ok("crash-forger-node2", "forger"),
        ];
        assert!(crash_problems(&steps).is_empty());
        steps[0].record["supervisorAfter"] = json!(4);
        steps[1].record["restartAfter"] = json!(0);
        assert_eq!(crash_problems(&steps).len(), 2);
    }

    #[test]
    fn a_halt_needs_a_standstill_the_forgers_line_and_enough_slots() {
        let outage = |before: u64, after: u64, held: u64, line: &str| {
            vec![
                step(
                    "outage-two",
                    json!({"heightBefore": 170, "heightAfter": 175}),
                ),
                step(
                    "outage-three",
                    json!({"heightBefore": before, "heightAfter": after, "heldSlots": held,
                        "logLine": line}),
                ),
                step("resume", json!({"heightBefore": 180, "heightAfter": 183})),
                step(
                    "return-node4",
                    json!({"synced": true, "recovered": "2026-01-01T00:00:10.000Z"}),
                ),
                step(
                    "return-node5",
                    json!({"synced": true, "recovered": "2026-01-01T00:00:10.000Z"}),
                ),
            ]
        };
        let line = format!("[2026-01-01 00:00:00.000] INFO: {HALT_LINE} (22 of 53)");
        assert!(outage_problems(&outage(180, 180, 7, &line)).is_empty());
        assert_eq!(outage_problems(&outage(180, 181, 7, &line)).len(), 1);
        assert_eq!(outage_problems(&outage(180, 180, 5, &line)).len(), 1);
        assert_eq!(outage_problems(&outage(180, 180, 7, "")).len(), 1);
    }

    #[test]
    fn a_pool_below_its_limit_or_not_drained_is_a_problem() {
        let full = |peak: u64, accepted: u64, drained: bool| {
            vec![step(
                "full-pool",
                json!({"expected": 15000, "accepted": accepted, "peak": peak, "drained": drained,
                    "drainedAt": "2026-01-01T00:00:10.000Z"}),
            )]
        };
        let mut ok = full(15000, 15000, true);
        ok[0].start = Some(0);
        assert!(pool_problems(&ok).is_empty());
        let mut low = full(14000, 15000, false);
        low[0].start = Some(0);
        assert_eq!(pool_problems(&low).len(), 2);
        assert_eq!(pool_problems(&[]).len(), 1);
    }

    #[test]
    fn a_window_must_name_the_nodes_it_concerns() {
        let w = |fault: &str, nodes: &[&str]| Window {
            fault: fault.into(),
            nodes: nodes.iter().map(|n| (*n).to_owned()).collect(),
            start: 0,
            end: 10,
        };
        let windows = [w("outage-three", &["node1", "node2", "node3"])];
        assert!(has_window(&windows, "outage-three", &["node1", "node3"]));
        assert!(!has_window(&windows, "outage-three", &["node1", "node4"]));
        assert!(!has_window(&windows, "restart", &["node1"]));
        let problems = window_problems(&windows);
        assert!(problems.iter().any(|p| p.contains("`supervised-relay`")));
        assert!(problems.iter().any(|p| p.contains("`fresh-sync`")));
    }

    #[test]
    fn the_relay_only_node_answers_every_route_with_the_references_status() {
        let dir = std::env::temp_dir().join(format!("hw-churn-routes-{}", std::process::id()));
        let routes = dir.join(ROUTES);
        std::fs::create_dir_all(&routes).unwrap();
        std::fs::write(
            routes.join("route-record.json"),
            json!({"left": "node6", "right": "node7"}).to_string(),
        )
        .unwrap();
        let answer = |name: &str, l: u64, r: u64, documented: bool| {
            let mut v = json!({"name": name, "left": {"status": l}, "right": {"status": r}});
            if documented {
                v["documented"] = json!("heightAboveTip");
            }
            format!("{v}\n")
        };
        let mut lines: String = (0..MIN_ROUTES)
            .map(|n| answer(&format!("r{n}"), 200, 200, false))
            .collect();
        lines.push_str(&answer("above-tip", 404, 500, true));
        lines.push_str("{\"name\": \"outside\", \"skipped\": \"not sent\"}\n");
        std::fs::write(routes.join("answers.jsonl"), &lines).unwrap();
        assert!(
            route_problems(&dir).is_empty(),
            "{:?}",
            route_problems(&dir)
        );
        lines.push_str(&answer("blocks/last", 500, 200, false));
        std::fs::write(routes.join("answers.jsonl"), &lines).unwrap();
        assert_eq!(
            route_problems(&dir),
            vec!["blocks/last: node6 answered 500, the reference relay 200".to_owned()]
        );
        std::fs::write(routes.join("answers.jsonl"), answer("one", 200, 200, false)).unwrap();
        assert_eq!(route_problems(&dir).len(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(route_problems(&dir).len(), 2);
    }

    #[test]
    fn a_restart_needs_its_relays_stop_line_inside_the_step() {
        let dir = std::env::temp_dir().join(format!("hw-churn-stop-{}", std::process::id()));
        let log = dir.join("nodes/node1").join(RELAY_LOG);
        std::fs::create_dir_all(log.parent().unwrap()).unwrap();
        let mut steps = cycles();
        steps.retain(|s| s.name == "cycle-node1");
        steps[0].start = Some(1_790_000_000_000);
        let start = 1_790_000_000_000;
        let line = |t: i64| {
            let stamp = run::format_time(t);
            format!(
                "[{} {}] INFO: {STOPPED_LINE}: its pool is flushed\n",
                &stamp[..10],
                &stamp[11..23]
            )
        };
        std::fs::write(&log, line(start + 100)).unwrap();
        let ok = stop_log_problems(&dir, &steps);
        assert!(ok.is_empty(), "{ok:?}");
        std::fs::write(&log, line(start - 60_000)).unwrap();
        assert_eq!(stop_log_problems(&dir, &steps).len(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
