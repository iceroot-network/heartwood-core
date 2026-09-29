//! The schedule of a mixed run: every step's recorded start height in its planned round, the steps
//! in their planned order, the node-6 steps each after the window of the one before, and the
//! pause at the run's stop height.
//!
//! The recipe records each step's `startHeight` (node 1's height when the step began) and its
//! `start` and `end` times. A round is `round_length` heights: round `n` holds the heights
//! `(n - 1) * round_length + 1` to `n * round_length`. A height trigger ("once node 1's height
//! reaches 130") reads the chain a moment before the step starts, so a recorded height may be a
//! block or two below the trigger: [`LAG_BLOCKS`] of slack for the lower bound of a trigger, none
//! for a round's edges.

use std::collections::BTreeMap;

use super::forging::start_heights;
use super::run::Step;
use super::{Cases, MixedRun};
use crate::errors::Result;

/// The blocks by which a step's recorded start height may lie below the height that triggered it.
const LAG_BLOCKS: u64 = 2;

/// A step of a mixed run's schedule and where it belongs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Planned {
    /// The step's name in `steps.jsonl`.
    pub name: &'static str,
    /// The first and last round it may start in.
    pub rounds: (u32, u32),
    /// The height its trigger waits for, when it has one.
    pub from: u64,
}

const fn planned(name: &'static str, first: u32, last: u32, from: u64) -> Planned {
    Planned {
        name,
        rounds: (first, last),
        from,
    }
}

/// The steps the run `run` has beyond the common ones, with their planned rounds and triggers.
/// `stall` is the serving relay's, which the reference baseline `w0` has no counterpart of.
pub(super) fn plan(run: &str) -> Vec<Planned> {
    match run {
        "w" | "w0" => {
            let mut steps = vec![
                planned("txgen-fund", 1, 1, 0),
                planned("txgen-once", 1, 1, 0),
                planned("peers-node6", 1, 1, 0),
                planned("start-node5", 2, 2, 0),
                planned("node5-synced", 2, 2, 0),
                planned("batch-node5", 2, 2, 0),
                planned("lag-node5", 2, 2, 0),
                planned("r1", 2, 3, 0),
                planned("r2", 2, 3, 0),
                planned("r3", 2, 3, 0),
                planned("vote-node6", 2, 3, 0),
                planned("restart-node4", 3, 3, 130),
                planned("heal-node6", 3, 3, 107),
                planned("summary-before-rollback", 4, 4, 160),
                planned("rollback-node6", 4, 4, 160),
                planned("summary-after-rollback", 4, 4, 160),
                planned("summary-after-start", 4, 4, 160),
                planned("resync-node6", 4, 4, 160),
                planned("p2p-corpus", 4, 4, 0),
                planned("api-corpus", 4, 4, 0),
            ];
            if run == "w" {
                steps.push(planned("stall", 3, 3, 110));
            }
            steps
        }
        "p" => vec![
            planned("txgen-fund", 1, 2, 0),
            planned("send-txs", 2, 2, 55),
            planned("partition", 2, 2, 80),
            planned("conflict", 2, 3, 0),
            planned("minority-pause", 2, 3, 0),
            planned("archive-minority", 2, 3, 0),
            planned("heal", 3, 3, 107),
            planned("converged", 3, 3, 107),
            planned("conflict-outcome", 3, 3, 107),
        ],
        "s" => vec![
            planned("txgen-fund", 1, 1, 0),
            planned("txgen-once", 1, 1, 0),
            planned("start-node4", 1, 1, 20),
            planned("vote-node1-delegate", 1, 1, 0),
            planned("open-routes-node4", 2, 2, 55),
            planned("peers-node4", 2, 2, 55),
            planned("vote-node2-delegate", 2, 2, 55),
            planned("batch-node4", 2, 2, 55),
            planned("lag-node4", 2, 2, 55),
            planned("restart-node2", 2, 2, 75),
            planned("p2p-corpus", 2, 2, 0),
            planned("api-corpus", 2, 2, 0),
        ],
        _ => Vec::new(),
    }
}

/// The steps that must run in this order (each pair: the one that starts first, the one after).
pub(super) fn order(run: &str) -> &'static [(&'static str, &'static str)] {
    match run {
        "w" | "w0" => &[
            ("batch-node5", "r1"),
            ("r1", "r2"),
            ("r2", "r3"),
            ("r3", "vote-node6"),
            ("vote-node6", "heal-node6"),
            ("heal-node6", "rollback-node6"),
            ("stall", "restart-node4"),
            ("restart-node4", "rollback-node6"),
            ("summary-before-rollback", "rollback-node6"),
            ("rollback-node6", "summary-after-rollback"),
            ("summary-after-rollback", "summary-after-start"),
            ("summary-after-start", "resync-node6"),
        ],
        "p" => &[
            ("send-txs", "partition"),
            ("partition", "conflict"),
            ("conflict", "heal"),
            ("minority-pause", "heal"),
            ("archive-minority", "heal"),
            ("heal", "converged"),
        ],
        "s" => &[
            ("vote-node1-delegate", "open-routes-node4"),
            ("open-routes-node4", "peers-node4"),
            ("open-routes-node4", "vote-node2-delegate"),
            ("batch-node4", "restart-node2"),
        ],
        _ => &[],
    }
}

/// The steps each of which must start only after the window (the `end`) of the one before it
/// closed: node 6's sequence and what its rollback waits for.
pub(super) fn waits(run: &str) -> &'static [(&'static str, &'static str)] {
    match run {
        "w" | "w0" => &[
            ("r1", "r2"),
            ("r2", "r3"),
            ("vote-node6", "heal-node6"),
            ("heal-node6", "rollback-node6"),
            ("restart-node4", "rollback-node6"),
            ("stall", "restart-node4"),
        ],
        _ => &[],
    }
}

/// The height each run stops at (and pauses its forgers at).
pub(super) fn stop_height(run: &str) -> Option<u32> {
    match run {
        "w" | "w0" => Some(212),
        "p" => Some(159),
        "s" => Some(106),
        _ => None,
    }
}

/// The round of `height` in rounds of `length` heights.
fn round_of(height: u64, length: u32) -> u64 {
    height.saturating_sub(1) / u64::from(length.max(1)) + 1
}

/// Where a step stands against the others: its start time, and else its place in the file.
fn started(steps: &[Step], index: usize) -> (i64, usize) {
    (steps[index].start.unwrap_or(i64::MIN), index)
}

/// The problems of the schedule of `steps` of the run `run`: a step outside its planned round or
/// before its trigger height, a pair out of order, a window that had not closed, the pause away
/// from the stop height.
pub(super) fn problems(run: &str, steps: &[Step], round_length: u32, stop: u32) -> Vec<String> {
    let mut out = Vec::new();
    if stop_height(run) != Some(stop) {
        out.push(format!(
            "the run stops at height {stop}, not at the planned {}",
            stop_height(run).map_or_else(|| "?".to_owned(), |h| h.to_string())
        ));
    }
    let heights: BTreeMap<usize, u64> = {
        let floors = start_heights(steps);
        // `start_heights` keeps the steps' order: index i is the i-th step.
        floors.iter().enumerate().map(|(i, (_, h))| (i, *h)).collect()
    };
    let first = |name: &str| steps.iter().position(|s| s.name == name);
    for p in plan(run) {
        let Some(i) = first(p.name) else {
            continue;
        };
        let recorded = steps[i].start_height();
        let Some(h) = recorded.map(|_| heights[&i]) else {
            out.push(format!("step {} records no start height", p.name));
            continue;
        };
        let round = round_of(h, round_length);
        if round < u64::from(p.rounds.0) || round > u64::from(p.rounds.1) {
            out.push(format!(
                "step {} started at height {h}, in round {round}; it is planned in round {}{}",
                p.name,
                p.rounds.0,
                if p.rounds.1 == p.rounds.0 {
                    String::new()
                } else {
                    format!(" or {}", p.rounds.1)
                }
            ));
        }
        if h + LAG_BLOCKS < p.from {
            out.push(format!(
                "step {} started at height {h}, before the height {} its trigger waits for",
                p.name, p.from
            ));
        }
    }
    for (before, after) in order(run) {
        let (Some(a), Some(b)) = (first(before), first(after)) else {
            continue;
        };
        if started(steps, a) > started(steps, b) {
            out.push(format!("step {before} started after step {after}"));
        }
    }
    for (earlier, later) in waits(run) {
        let (Some(a), Some(b)) = (first(earlier), first(later)) else {
            continue;
        };
        if let (Some(end), Some(start)) = (steps[a].time("end"), steps[b].start)
            && start < end
        {
            out.push(format!(
                "step {later} started before the window of step {earlier} had closed"
            ));
        }
    }
    if let Some(i) = first("pause") {
        match steps[i].start_height() {
            Some(h) if round_of(h.max(1), round_length) != round_of(u64::from(stop), round_length) => {
                out.push(format!(
                    "the pause started at height {h}, outside the last round (the stop height is {stop})"
                ));
            }
            None => out.push("the pause records no start height".into()),
            Some(_) => {}
        }
    }
    out
}

/// The schedule's checks.
pub(super) fn check(run: &MixedRun<'_>, cases: &mut Cases) -> Result<()> {
    let label = "every step in its planned round and order";
    let steps = match super::run::steps(run.run_dir) {
        Ok(steps) => steps,
        Err(e) => {
            cases.fail("schedule", label, e.to_string());
            return Ok(());
        }
    };
    let m = run.manifest;
    let found = problems(&m.run, &steps, m.round_length, m.stop_height);
    if found.is_empty() {
        cases.pass(
            "schedule",
            format!(
                "{label}: {} steps, the pause at the stop height {}'s round",
                steps.len(),
                m.stop_height
            ),
        );
    }
    for p in found.iter().take(20) {
        cases.fail("schedule", label, p.clone());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn step(name: &str, height: u64, start: i64, end: i64) -> Step {
        Step {
            name: name.into(),
            node: "node6".into(),
            ok: true,
            start: Some(start),
            record: json!({"step": name, "startHeight": height,
                "end": super::super::run::format_time(end)}),
        }
    }

    /// A schedule of run `w` that keeps to the plan.
    fn kept() -> Vec<Step> {
        let mut at = 0;
        let mut steps = Vec::new();
        for (name, h) in [
            ("txgen-fund", 4),
            ("txgen-once", 5),
            ("peers-node6", 30),
            ("start-node5", 56),
            ("node5-synced", 60),
            ("batch-node5", 62),
            ("lag-node5", 64),
            ("r1", 66),
            ("r2", 70),
            ("r3", 76),
            ("vote-node6", 78),
            ("stall", 112),
            ("restart-node4", 131),
            ("heal-node6", 133),
            ("summary-before-rollback", 160),
            ("rollback-node6", 161),
            ("summary-after-rollback", 161),
            ("summary-after-start", 162),
            ("resync-node6", 163),
            ("p2p-corpus", 212),
            ("api-corpus", 212),
            ("pause", 212),
        ] {
            steps.push(step(name, h, at, at + 500));
            at += 1_000;
        }
        steps
    }

    #[test]
    fn a_run_that_keeps_to_the_plan_has_no_problem() {
        assert_eq!(problems("w", &kept(), 53, 212), Vec::<String>::new());
    }

    #[test]
    fn a_step_out_of_its_round_or_order_is_a_problem() {
        // The rollback in round 3.
        let mut steps = kept();
        for name in ["summary-before-rollback", "rollback-node6"] {
            let i = steps.iter().position(|s| s.name == name).unwrap();
            steps[i].record["startHeight"] = json!(150);
        }
        let found = problems("w", &steps, 53, 212);
        assert!(
            found.iter().any(|p| p.contains(
                "step rollback-node6 started at height 150, in round 3; it is planned in round 4"
            )),
            "{found:?}"
        );
        // r2 before r1.
        let mut steps = kept();
        let (a, b) = (
            steps.iter().position(|s| s.name == "r1").unwrap(),
            steps.iter().position(|s| s.name == "r2").unwrap(),
        );
        steps.swap(a, b);
        let start = steps[a].start;
        steps[a].start = steps[b].start;
        steps[b].start = start;
        let found = problems("w", &steps, 53, 212);
        assert!(
            found.iter().any(|p| p.contains("step r1 started after step r2")),
            "{found:?}"
        );
    }

    #[test]
    fn a_window_that_had_not_closed_and_a_short_pause_are_problems() {
        let mut steps = kept();
        // The heal starts before r3's window ended.
        let i = steps.iter().position(|s| s.name == "vote-node6").unwrap();
        steps[i].record["end"] = json!("2100-01-01T00:00:00.000Z");
        let found = problems("w", &steps, 53, 212);
        assert!(
            found
                .iter()
                .any(|p| p.contains("heal-node6 started before the window of step vote-node6")),
            "{found:?}"
        );
        let mut steps = kept();
        let i = steps.iter().position(|s| s.name == "pause").unwrap();
        steps[i].record["startHeight"] = json!(150);
        let found = problems("w", &steps, 53, 212);
        assert!(found.iter().any(|p| p.contains("outside the last round")), "{found:?}");
        assert!(problems("w", &kept(), 53, 265)[0].contains("planned 212"));
    }

    #[test]
    fn the_baseline_has_no_stall_and_a_missing_height_is_named() {
        let mut steps = kept();
        steps.retain(|s| s.name != "stall");
        assert!(problems("w0", &steps, 53, 212).is_empty());
        let i = steps.iter().position(|s| s.name == "r1").unwrap();
        steps[i].record = json!({"step": "r1"});
        // A record without its height is reported, and the floor of the steps before it is not
        // taken for it.
        assert!(
            problems("w0", &steps, 53, 212)
                .iter()
                .any(|p| p.contains("step r1 records no start height"))
        );
    }
}
