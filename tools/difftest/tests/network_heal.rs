//! The network suite's block-lag rule after a heal: the minority's window runs until the network
//! converged on one tip (the window the mixed run's check gives an undeclared minority), within a
//! bound, and everything outside it is judged as before.
//!
//! A saved run of the reference's network suite, whose minority node got a block 1.3 s after the
//! partition's window closed and before the network converged, passes; fixtures built here fail
//! where they leave the chosen bound: a convergence later than the bound, a network that never
//! converges, a minority block late after the convergence's window, a majority block late after
//! the heal, a partition without a heal and a heal without a partition. `scenario compare` reads
//! the step through the same rule.

use std::path::{Path, PathBuf};

use heartwood_difftest::live::{self, HEAL_CONVERGENCE_SECONDS, SUITE_WINDOW_SECONDS};
use heartwood_difftest::report::Outcome;
use heartwood_difftest::scenario;
use heartwood_difftest::store::ScratchDir;
use serde_json::{Value, json};

fn saved_run() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/network-heal/reference-run")
}

/// A time `ms` milliseconds after a fixed start, as the recipes write it.
fn at(ms: i64) -> String {
    let base = live::run::parse_time("2026-10-02T00:00:00.000Z").expect("a time");
    live::run::format_time(base + ms)
}

/// A run directory: `propagation.jsonl` from `rows` (milliseconds, node, height, block), its
/// actions and the recipe's report of the step, judged or not.
fn write_run(dir: &Path, rows: &[(i64, u32, u64, String)], actions: &[Value], judged: bool) {
    std::fs::create_dir_all(dir).expect("the run directory");
    let mut text = String::new();
    for (t, node, h, id) in rows {
        let row = json!({"t": at(*t), "node": format!("node{node}"), "h": h, "id": id});
        text.push_str(&format!("{row}\n"));
    }
    std::fs::write(dir.join("propagation.jsonl"), text).expect("propagation.jsonl");
    let actions: String = actions.iter().map(|a| format!("{a}\n")).collect();
    std::fs::write(dir.join("actions.jsonl"), actions).expect("actions.jsonl");
    let report = json!({"limitSeconds": 8.0, "judged": judged, "ok": false, "over": []});
    std::fs::write(dir.join("propagation-lag.json"), report.to_string()).expect("the report");
}

fn action(verb: &str, args: &[&str], start: i64, end: i64) -> Value {
    json!({"verb": verb, "args": args, "start": at(start), "end": at(end), "rc": 0})
}

/// A five-node network split 3 and 2 at 100 s and healed from 200 s to 201 s: a block every 8 s,
/// each node holding it within 0.1 s, the minority its own branch during the split; after the heal
/// the minority syncs every majority block it missed at once, `catch_up` ms after the heal's end,
/// and holds each later block with everyone else.
fn split(catch_up: i64) -> Vec<(i64, u32, u64, String)> {
    let mut rows = Vec::new();
    let synced = 201_000 + catch_up;
    for h in 1..=40u64 {
        let t = i64::try_from(h).expect("small") * 8_000;
        for node in 1..=5u32 {
            let at = t + i64::from(node) * 20;
            if node >= 4 && t > 100_000 && at < synced {
                if t < 200_000 {
                    rows.push((at, node, h, format!("branch-{h}")));
                } else {
                    rows.push((synced, node, h, format!("main-{h}")));
                }
                continue;
            }
            rows.push((at, node, h, format!("main-{h}")));
        }
    }
    // The heights the minority held on its branch, replaced in its sync too.
    for node in 4..=5u32 {
        for h in 13..=24u64 {
            rows.push((synced, node, h, format!("main-{h}")));
        }
    }
    rows.sort();
    rows
}

fn heal_actions() -> Vec<Value> {
    vec![
        action("partition", &["1,2,3", "4,5"], 100_000, 101_000),
        action("heal", &[], 200_000, 201_000),
    ]
}

fn judged(dir: &Path) -> Vec<String> {
    live::suite_propagation(dir)
        .expect("readable")
        .expect("judged")
}

#[test]
fn the_saved_minority_block_after_the_heal_falls_in_its_convergence_window() {
    // The recipe failed the step: node 4 got block 71 1.3 s after the partition's window.
    let steps = std::fs::read_to_string(saved_run().join("steps.jsonl")).expect("steps");
    assert!(
        steps.contains(r#""step":"propagation-lag","node":"all""#)
            && steps.contains(r#""ok":false"#)
    );
    assert_eq!(judged(&saved_run()), Vec::<String>::new());
}

#[test]
fn a_valid_recovery_passes_and_each_late_or_unbounded_heal_fails() {
    let scratch = ScratchDir::new("network-heal").expect("scratch");
    let dir = |name: &str| scratch.path().join(name);
    // The minority catches up 35 s after the heal's end: after the partition's window, inside the
    // convergence's.
    let valid = dir("valid");
    write_run(&valid, &split(35_000), &heal_actions(), true);
    assert_eq!(judged(&valid), Vec::<String>::new());
    // Not judged by the recipe (a suite other than the network suite): the recipe's verdict stays.
    let unjudged = dir("unjudged");
    write_run(&unjudged, &split(35_000), &heal_actions(), false);
    assert_eq!(live::suite_propagation(&unjudged).expect("readable"), None);
    // A convergence later than the bound.
    let slow = dir("slow");
    let late = (HEAL_CONVERGENCE_SECONDS + 10) * 1_000;
    write_run(&slow, &split(late), &heal_actions(), true);
    let problems = judged(&slow);
    assert!(
        problems.iter().any(|p| p.contains("beyond")),
        "{problems:?}"
    );
    // A network that never converges: the minority stays on its branch.
    let stuck = dir("stuck");
    let rows: Vec<_> = split(35_000)
        .into_iter()
        .filter(|(t, node, _, _)| *node < 4 || *t < 200_000)
        .collect();
    write_run(&stuck, &rows, &heal_actions(), true);
    let problems = judged(&stuck);
    assert!(
        problems.iter().any(|p| p.contains("did not converge")),
        "{problems:?}"
    );
    // A minority block late after the convergence's window: the lag rule holds again there.
    let after = dir("after");
    let mut rows = split(35_000);
    let window_end = 201_000 + 35_000 + SUITE_WINDOW_SECONDS * 1_000;
    rows.retain(|(t, node, h, _)| !(*node == 5 && *h == 31 && *t > 200_000));
    rows.push((window_end + 9_500, 5, 31, "main-31".to_owned()));
    rows.sort();
    write_run(&after, &rows, &heal_actions(), true);
    let problems = judged(&after);
    assert!(
        problems.iter().any(|p| p.contains("node5 got block 31")),
        "{problems:?}"
    );
    // A majority block late after the partition's window: the minority's window is not its own.
    let majority = dir("majority");
    let mut rows = split(35_000);
    rows.retain(|(_, node, h, _)| !(*node == 2 && *h == 29));
    rows.push((29 * 8_000 + 9_000, 2, 29, "main-29".to_owned()));
    rows.sort();
    write_run(&majority, &rows, &heal_actions(), true);
    let problems = judged(&majority);
    assert!(
        problems.iter().any(|p| p.contains("node2 got block 29")),
        "{problems:?}"
    );
    // No evidence of the heal: a partition that stands to the end excuses nothing.
    let unhealed = dir("unhealed");
    write_run(&unhealed, &split(35_000), &heal_actions()[..1], true);
    let problems = judged(&unhealed);
    assert!(
        problems.iter().any(|p| p.contains("has no heal")),
        "{problems:?}"
    );
    // A heal without a partition.
    let lone = dir("lone");
    write_run(&lone, &split(35_000), &heal_actions()[1..], true);
    let problems = judged(&lone);
    assert!(
        problems.iter().any(|p| p.contains("without a partition")),
        "{problems:?}"
    );
}

#[test]
fn a_judged_run_without_its_block_evidence_fails() {
    let scratch = ScratchDir::new("network-heal-evidence").expect("scratch");
    // No block arrival at all: nothing was judged, so nothing passes.
    let empty = scratch.path().join("empty");
    write_run(&empty, &[], &[], true);
    let problems = judged(&empty);
    assert!(
        problems.iter().any(|p| p.contains("no block arrival")),
        "{problems:?}"
    );
    // A partitioned node without a block arrival: its catch-up is not shown, so the network's
    // convergence is not either.
    let silent = scratch.path().join("silent");
    let rows: Vec<_> = split(35_000)
        .into_iter()
        .filter(|(_, node, _, _)| *node != 5)
        .collect();
    write_run(&silent, &rows, &heal_actions(), true);
    let problems = judged(&silent);
    assert!(
        problems
            .iter()
            .any(|p| p.contains("node5 of the partition holds no block arrival")),
        "{problems:?}"
    );
    // Arrivals without the blocks' ids cannot show one tip: no convergence after the heal.
    let anonymous = scratch.path().join("anonymous");
    let rows: Vec<_> = split(35_000)
        .into_iter()
        .map(|(t, node, h, _)| (t, node, h, String::new()))
        .collect();
    write_run(&anonymous, &rows, &heal_actions(), true);
    let problems = judged(&anonymous);
    assert!(
        problems.iter().any(|p| p.contains("did not converge")),
        "{problems:?}"
    );
    // The recipe's other form of a time (milliseconds since the epoch) is read as the recipe
    // reads it: a late block in that form still fails.
    let numeric = scratch.path().join("numeric");
    write_run(&numeric, &split(35_000), &heal_actions(), true);
    let path = numeric.join("propagation.jsonl");
    let mut text = std::fs::read_to_string(&path).expect("rows");
    let base = live::run::parse_time("2026-10-02T00:00:00.000Z").expect("a time");
    let late = base + 29 * 8_000 + 9_000;
    text.push_str(&format!(
        "{{\"t\":{late},\"node\":\"node2\",\"h\":29,\"id\":\"main-29\"}}\n"
    ));
    std::fs::write(&path, text).expect("rows");
    let problems = judged(&numeric);
    assert!(
        problems.iter().any(|p| p.contains("node2 got block 29")),
        "{problems:?}"
    );
    // A time outside any calendar is not a time: it is left out, as the recipe leaves out a row
    // it cannot read, and judges nothing.
    let mut text = std::fs::read_to_string(&path).expect("rows");
    text.push_str(&format!(
        "{{\"t\":{},\"node\":\"node2\",\"h\":30,\"id\":\"main-30\"}}\n",
        i64::MIN
    ));
    std::fs::write(&path, text).expect("rows");
    let problems = judged(&numeric);
    assert_eq!(problems.len(), 1, "{problems:?}");
}

/// A suite directory with the network suite's two runs, the reference's from `reference`.
fn suite_dir(root: &Path, reference: &Path) -> PathBuf {
    let dir = root.join("suites");
    for side in ["network-reference", "network-heartwood"] {
        let to = dir.join(side);
        std::fs::create_dir_all(&to).expect("a run");
        for file in [
            "propagation.jsonl",
            "actions.jsonl",
            "propagation-lag.json",
            "steps.jsonl",
        ] {
            std::fs::copy(reference.join(file), to.join(file)).expect("a file");
        }
    }
    dir
}

fn steps_case(dir: &Path) -> Outcome {
    let report = scenario::record::compare(dir, Some("network"), None).expect("the comparison");
    report
        .results
        .into_iter()
        .find(|r| r.label.contains("the reference run's") && r.label.contains("scripted steps"))
        .map(|r| r.outcome)
        .expect("the reference run's steps case")
}

#[test]
fn scenario_compare_judges_the_lag_step_by_the_same_rule() {
    let scratch = ScratchDir::new("network-heal-compare").expect("scratch");
    let dir = suite_dir(scratch.path(), &saved_run());
    assert_eq!(steps_case(&dir), Outcome::Pass);
    // The same run with a block late outside every window fails the step, and says why.
    let late = dir.join("network-reference").join("propagation.jsonl");
    let mut text = std::fs::read_to_string(&late).expect("rows");
    text.push_str(r#"{"t":"2026-10-02T01:00:20.000Z","node":"node2","h":88,"id":"x"}"#);
    text.push('\n');
    std::fs::write(&late, text).expect("rows");
    match steps_case(&dir) {
        Outcome::HarnessError(message) => {
            assert!(
                message.contains("propagation-lag (all): 1 blocks later"),
                "{message}"
            );
        }
        other => panic!("the late block passed: {other:?}"),
    }
}
