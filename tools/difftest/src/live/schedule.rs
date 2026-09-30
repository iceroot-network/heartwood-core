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

/// The steps the run `run` has beyond the common ones, with their planned rounds and triggers, by
/// the names the recipe records them under. `txgen --once` starts in round 2: its votes need the
/// attestations round 2's pings give (the step `attest-wait`), and the reference refuses them
/// before. `stall-node4` is the serving relay's, which the reference baseline `w0` has no
/// counterpart of.
pub(super) fn plan(run: &str) -> Vec<Planned> {
    match run {
        "w" | "w0" => {
            let mut steps = vec![
                planned("txgen-fund", 1, 1, 0),
                planned("peers-node6", 1, 1, 0),
                planned("start-node5", 2, 2, 54),
                planned("attest-wait", 2, 2, 0),
                planned("txgen-once", 2, 2, 55),
                planned("node5-synced", 2, 2, 0),
                planned("batch-node5", 2, 2, 0),
                planned("r1", 2, 3, 0),
                planned("r2", 2, 3, 0),
                planned("r3", 2, 3, 0),
                planned("r3-vote", 2, 3, 0),
                planned("restart-node4", 3, 3, 130),
                planned("heal-node6", 3, 3, 107),
                planned("rollback-isolate", 4, 4, 160),
                planned("summary-before", 4, 4, 160),
                planned("rollback", 4, 4, 160),
                planned("summary-rolled-back", 4, 4, 160),
                planned("rollback-start", 4, 4, 160),
                planned("summary-restarted", 4, 4, 160),
                planned("rollback-open", 4, 4, 160),
                planned("p2p-probe", 4, 4, 0),
                planned("api-corpus", 4, 4, 0),
            ];
            if run == "w" {
                steps.push(planned("stall-node4", 3, 3, 110));
            }
            steps
        }
        "p" => vec![
            planned("txgen-fund", 1, 2, 0),
            planned("light-txgen", 2, 2, 55),
            planned("partition", 2, 2, 80),
            planned("conflict", 2, 3, 0),
            planned("minority-pause", 2, 3, 0),
            planned("archive-minority", 2, 3, 0),
            planned("heal", 3, 3, 133),
            planned("converged", 3, 3, 133),
            planned("conflict-outcome", 3, 3, 133),
        ],
        "s" => vec![
            planned("txgen-fund", 1, 1, 0),
            planned("start-node4", 1, 1, 20),
            planned("vote-s18a", 1, 1, 0),
            planned("attest-wait", 1, 2, 0),
            planned("txgen-once", 2, 2, 55),
            planned("open-routes-node4", 2, 2, 55),
            planned("peers-node4", 2, 2, 55),
            planned("vote-s18b", 2, 2, 55),
            planned("batch-node4", 2, 2, 55),
            planned("restart-node2", 2, 2, 75),
            planned("p2p-probe", 2, 2, 0),
            planned("api-corpus", 2, 2, 0),
        ],
        "mixed-short" | "mixed" | "mixed-split" => forging_plan(run),
        _ => Vec::new(),
    }
}

/// The steps of the mixed forging network's runs. All three fund the sender wallet and poll node
/// 5's peers in round 1; from round 2 they wait for the attestations, send the transactions, place
/// the votes of the two rounds and compare the batches. The route corpus is recorded at the pause,
/// in the last round. `mixed` (reference nodes 1 to 3 beside Heartwood nodes 4 and 5) then pauses
/// a reference forger, restarts a Heartwood and a reference node, kills a Heartwood forger and
/// partitions the network; `mixed-split` (Heartwood nodes 1 and 2 beside reference nodes 3 to 5)
/// splits the network with two conflicting transactions and heals it.
fn forging_plan(run: &str) -> Vec<Planned> {
    let last = stop_height(run).map_or(2, |h| h.div_ceil(53));
    let mut steps = vec![
        planned("txgen-fund", 1, 1, 0),
        planned("peers-node5", 1, 1, 0),
        planned("attest-wait", 1, 2, 0),
        planned("attest-evidence", 2, 2, 55),
        planned("txgen-once", 2, 2, 55),
        planned("vote-s18a", 2, 2, 55),
        planned("vote-s18b", 2, 2, 55),
        planned("batch-heartwood", 2, 2, 55),
        planned("batch-reference", 2, 2, 55),
        planned("api-corpus", last, last, 0),
    ];
    match run {
        "mixed" => steps.extend([
            planned("pause-forger-node3", 2, 2, 70),
            planned("restart-node4", 3, 3, 112),
            planned("restart-node2", 3, 3, 122),
            planned("kill-forger-node5", 3, 3, 132),
            planned("partition", 3, 4, 155),
            planned("heal", 4, 4, 160),
            planned("converged", 4, 4, 160),
        ]),
        "mixed-split" => steps.extend([
            planned("light-txgen", 2, 2, 55),
            planned("partition", 3, 3, 133),
            planned("conflict", 3, 4, 0),
            planned("minority-pause", 4, 4, 0),
            planned("archive-minority", 4, 4, 0),
            planned("heal", 4, 4, 186),
            planned("converged", 4, 4, 186),
            planned("conflict-outcome", 4, 5, 186),
        ]),
        _ => {}
    }
    steps
}

/// The steps that must run in this order (each pair: the one that starts first, the one after).
pub(super) fn order(run: &str) -> &'static [(&'static str, &'static str)] {
    match run {
        "w" | "w0" => &[
            ("attest-wait", "txgen-once"),
            ("batch-node5", "r1"),
            ("r1", "r2"),
            ("r2", "r3"),
            ("r3", "r3-vote"),
            ("r3-vote", "heal-node6"),
            ("heal-node6", "rollback-isolate"),
            ("stall-node4", "restart-node4"),
            ("restart-node4", "rollback-isolate"),
            ("rollback-isolate", "summary-before"),
            ("summary-before", "rollback"),
            ("rollback", "summary-rolled-back"),
            ("summary-rolled-back", "rollback-start"),
            ("rollback-start", "summary-restarted"),
            ("summary-restarted", "rollback-open"),
        ],
        "p" => &[
            ("light-txgen", "partition"),
            ("partition", "conflict"),
            ("conflict", "heal"),
            ("minority-pause", "heal"),
            ("archive-minority", "heal"),
            ("heal", "converged"),
        ],
        "s" => &[
            ("start-node4", "vote-s18a"),
            ("attest-wait", "txgen-once"),
            ("vote-s18a", "open-routes-node4"),
            ("open-routes-node4", "peers-node4"),
            ("open-routes-node4", "vote-s18b"),
            ("batch-node4", "restart-node2"),
        ],
        "mixed-short" | "mixed" | "mixed-split" => match run {
            "mixed" => &[
                ("attest-wait", "txgen-once"),
                ("txgen-once", "pause-forger-node3"),
                ("pause-forger-node3", "restart-node4"),
                ("restart-node4", "restart-node2"),
                ("restart-node2", "kill-forger-node5"),
                ("kill-forger-node5", "partition"),
                ("partition", "heal"),
                ("heal", "converged"),
            ],
            "mixed-split" => &[
                ("attest-wait", "txgen-once"),
                ("light-txgen", "partition"),
                ("partition", "conflict"),
                ("conflict", "heal"),
                ("minority-pause", "heal"),
                ("archive-minority", "heal"),
                ("heal", "converged"),
                ("converged", "conflict-outcome"),
            ],
            _ => &[("attest-wait", "txgen-once")],
        },
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
            ("r3-vote", "heal-node6"),
            ("heal-node6", "rollback-isolate"),
            ("restart-node4", "rollback-isolate"),
            ("stall-node4", "restart-node4"),
        ],
        "mixed" => &[("partition", "heal")],
        _ => &[],
    }
}

/// The height each run stops at (and pauses its forgers at).
pub(super) fn stop_height(run: &str) -> Option<u32> {
    match run {
        "w" | "w0" => Some(212),
        "p" => Some(159),
        "s" | "mixed-short" | "core" => Some(106),
        "mixed" | "mixed-split" | "churn" => Some(265),
        _ => None,
    }
}

/// The round of `height` in rounds of `length` heights.
fn round_of(height: u64, length: u32) -> u64 {
    height.saturating_sub(1) / u64::from(length.max(1)) + 1
}

/// The problems of the schedule of `steps` of the run `run`: a step without its start height or
/// time, outside its planned round or before its trigger height, a pair out of order, a window
/// that had not closed, the pause away from the stop height.
pub(super) fn problems(run: &str, steps: &[Step], round_length: u32, stop: u32) -> Vec<String> {
    let mut out = Vec::new();
    if stop_height(run) != Some(stop) {
        out.push(format!(
            "the run stops at height {stop}, not at the planned {}",
            stop_height(run).map_or_else(|| "?".to_owned(), |h| h.to_string())
        ));
    }
    let first = |name: &str| steps.iter().position(|s| s.name == name);
    for p in plan(run) {
        let Some(i) = first(p.name) else {
            continue;
        };
        if steps[i].start.is_none() {
            out.push(format!("step {} records no start time", p.name));
        }
        let Some(h) = steps[i].start_height() else {
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
        if let (Some(ta), Some(tb)) = (steps[a].start, steps[b].start)
            && ta > tb
        {
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
    // The pause step waits for the stop height and records the height the forgers paused at.
    match first("pause").map(|i| steps[i].number("height").or(steps[i].start_height())) {
        None => out.push("the run records no pause step".into()),
        Some(None) => out.push("the pause records no height".into()),
        Some(Some(h)) if h > u64::from(stop) || h + LAG_BLOCKS < u64::from(stop) => {
            out.push(format!(
                "the forgers paused at height {h}, not at the stop height {stop}"
            ));
        }
        Some(Some(_)) => {}
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
            ("peers-node6", 30),
            ("start-node5", 54),
            ("attest-wait", 55),
            ("txgen-once", 55),
            ("node5-synced", 60),
            ("batch-node5", 62),
            ("r1", 66),
            ("r2", 70),
            ("r3", 76),
            ("r3-vote", 78),
            ("stall-node4", 112),
            ("restart-node4", 131),
            ("heal-node6", 133),
            ("rollback-isolate", 160),
            ("summary-before", 160),
            ("rollback", 161),
            ("summary-rolled-back", 161),
            ("rollback-start", 162),
            ("summary-restarted", 162),
            ("rollback-open", 163),
            ("p2p-probe", 212),
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
        for name in ["rollback-isolate", "summary-before", "rollback"] {
            let i = steps.iter().position(|s| s.name == name).unwrap();
            steps[i].record["startHeight"] = json!(150);
        }
        let found = problems("w", &steps, 53, 212);
        assert!(
            found.iter().any(|p| p.contains(
                "step rollback started at height 150, in round 3; it is planned in round 4"
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
            found
                .iter()
                .any(|p| p.contains("step r1 started after step r2")),
            "{found:?}"
        );
    }

    #[test]
    fn a_window_that_had_not_closed_and_a_short_pause_are_problems() {
        let mut steps = kept();
        // The heal starts before r3's window ended.
        let i = steps.iter().position(|s| s.name == "r3-vote").unwrap();
        steps[i].record["end"] = json!("2100-01-01T00:00:00.000Z");
        let found = problems("w", &steps, 53, 212);
        assert!(
            found
                .iter()
                .any(|p| p.contains("heal-node6 started before the window of step r3-vote")),
            "{found:?}"
        );
        // A pause inside the last round but short of the stop height.
        let mut steps = kept();
        let i = steps.iter().position(|s| s.name == "pause").unwrap();
        steps[i].record["startHeight"] = json!(170);
        let found = problems("w", &steps, 53, 212);
        assert!(
            found
                .iter()
                .any(|p| p.contains("the forgers paused at height 170, not at the stop height 212")),
            "{found:?}"
        );
        // The pause step starts waiting before the stop height: the height it paused at counts.
        steps[i].record["height"] = json!(212);
        assert!(problems("w", &steps, 53, 212).is_empty());
        steps.remove(i);
        assert!(
            problems("w", &steps, 53, 212)
                .iter()
                .any(|p| p.contains("no pause step"))
        );
        assert!(problems("w", &kept(), 53, 265)[0].contains("planned 212"));
    }

    #[test]
    fn a_step_is_placed_by_its_own_height_and_needs_its_time() {
        // A later record of a step that started lower is still in its own round.
        let mut steps = kept();
        let i = steps.iter().position(|s| s.name == "node5-synced").unwrap();
        let synced = steps.remove(i);
        let j = steps.iter().position(|s| s.name == "stall-node4").unwrap();
        steps.insert(j + 1, synced);
        let found = problems("w", &steps, 53, 212);
        assert!(
            !found.iter().any(|p| p.contains("node5-synced")),
            "{found:?}"
        );
        // A step without its start time is named.
        let mut steps = kept();
        let i = steps.iter().position(|s| s.name == "r2").unwrap();
        steps[i].start = None;
        let found = problems("w", &steps, 53, 212);
        assert!(
            found
                .iter()
                .any(|p| p.contains("step r2 records no start time")),
            "{found:?}"
        );
    }

    #[test]
    fn the_heal_of_run_p_waits_for_the_partitions_end() {
        let steps: Vec<Step> = [
            ("txgen-fund", 5),
            ("light-txgen", 56),
            ("partition", 80),
            ("conflict", 90),
            ("minority-pause", 100),
            ("archive-minority", 101),
            ("heal", 115),
            ("converged", 136),
            ("conflict-outcome", 140),
            ("pause", 159),
        ]
        .iter()
        .enumerate()
        .map(|(k, (name, h))| {
            let at = i64::try_from(k).unwrap() * 1_000;
            step(name, *h, at, at + 500)
        })
        .collect();
        let found = problems("p", &steps, 53, 159);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].contains("step heal started at height 115, before the height 133"));
    }

    #[test]
    fn the_baseline_has_no_stall_and_a_missing_height_is_named() {
        let mut steps = kept();
        steps.retain(|s| s.name != "stall-node4");
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

    /// The recorded shape of a mixed forging run: every planned step in the plan's order, each at
    /// the first height its trigger and its round allow (the route corpus at the stop height),
    /// then the pause at the stop height.
    fn forging(run: &str) -> Vec<Step> {
        let stop = u64::from(stop_height(run).unwrap());
        let mut height = 1;
        let mut planned: Vec<(&str, u64)> = Vec::new();
        for p in plan(run) {
            if p.name == "api-corpus" {
                continue;
            }
            height = height.max(p.from).max(u64::from(p.rounds.0 - 1) * 53 + 1);
            planned.push((p.name, height));
        }
        planned.push(("api-corpus", stop));
        planned.push(("pause", stop));
        planned
            .iter()
            .enumerate()
            .map(|(k, (name, h))| {
                let at = i64::try_from(k).unwrap() * 1_000;
                step(name, *h, at, at + 500)
            })
            .collect()
    }

    #[test]
    fn the_mixed_forging_runs_have_their_plans() {
        for (run, stop) in [("mixed-short", 106), ("mixed", 265), ("mixed-split", 265)] {
            assert_eq!(stop_height(run), Some(stop));
            assert!(plan(run).iter().any(|p| p.name == "api-corpus"), "{run}");
        }
        assert!(plan("mixed").iter().any(|p| p.name == "kill-forger-node5"));
        assert!(
            plan("mixed-split")
                .iter()
                .any(|p| p.name == "conflict-outcome")
        );
        assert!(!plan("mixed-short").iter().any(|p| p.name == "partition"));
        // The route corpus is recorded in the run's last round.
        let corpus = |run: &str| {
            plan(run)
                .into_iter()
                .find(|p| p.name == "api-corpus")
                .unwrap()
        };
        assert_eq!(corpus("mixed-short").rounds, (2, 2));
        assert_eq!(corpus("mixed").rounds, (5, 5));
    }

    #[test]
    fn a_mixed_forging_run_that_keeps_to_the_plan_has_no_problem() {
        for (run, stop) in [("mixed-short", 106), ("mixed", 265), ("mixed-split", 265)] {
            let found = problems(run, &forging(run), 53, stop);
            assert_eq!(found, Vec::<String>::new(), "{run}");
        }
    }

    #[test]
    fn a_partition_before_its_height_and_a_heal_before_the_partition_are_problems() {
        let mut steps = forging("mixed");
        let i = steps.iter().position(|s| s.name == "partition").unwrap();
        steps[i].record["startHeight"] = json!(140);
        let found = problems("mixed", &steps, 53, 265);
        assert!(
            found
                .iter()
                .any(|p| p.contains("step partition started at height 140, before the height 155")),
            "{found:?}"
        );
        let mut steps = forging("mixed");
        let (a, b) = (
            steps.iter().position(|s| s.name == "partition").unwrap(),
            steps.iter().position(|s| s.name == "heal").unwrap(),
        );
        let start = steps[a].start;
        steps[a].start = steps[b].start;
        steps[b].start = start;
        let found = problems("mixed", &steps, 53, 265);
        assert!(
            found
                .iter()
                .any(|p| p.contains("step partition started after step heal")),
            "{found:?}"
        );
        // The run must stop at the planned height.
        assert!(
            problems("mixed-short", &forging("mixed-short"), 53, 265)[0].contains("planned 106")
        );
    }
}
