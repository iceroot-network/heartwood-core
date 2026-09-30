//! The scenario files under `tools/difftest/scenarios`: every file reads, each directory holds
//! the scenarios its suite names, the triggers stay inside the suite's planned height, and every
//! level a scenario lists has a record that proves it (the level map below). The one integration
//! test compiles every file through the real reference (ignored for the reason `integration`:
//! `DIFFTEST_TEST_SOLAR_DIR` or `DIFFTEST_TEST_IMAGE`, and a failure without either unless
//! `DIFFTEST_TEST_NO_ORACLE` says to skip).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use heartwood_difftest::scenario::compile::fire_heights;
use heartwood_difftest::scenario::format::Level;
use heartwood_difftest::scenario::{self, Scenario};

/// Delegates per round on the devnet every scenario runs on.
const ROUND_LENGTH: u32 = 53;

/// The genesis seed of the public development network.
const SEED: &str = "solar-devnet";

/// The scenario directories, each a suite, with the ids it holds and the height the suite stops
/// at (two rounds, three for the lifecycle suite, which has the failover and the restarts). The
/// lockstep directory holds the scenarios that only run through the producer.
const SUITES: &[(&str, &[&str], u32)] = &[
    (
        "transactions",
        &[
            "boot-check",
            "chained-transfers",
            "double-spend",
            "large-transfers",
            "burns-and-supply",
            "second-signature-and-ipfs",
            "insufficient-balance",
            "repeated-operations",
        ],
        106,
    ),
    (
        "lifecycle",
        &[
            "pool-survives-restart",
            "attestation",
            "missed-slots",
            "vote-in-last-block",
            "relay-forger-split",
            "restarts",
            "delegate-resignation",
        ],
        159,
    ),
    (
        "network",
        &[
            "sync-from-genesis",
            "peer-discovery",
            "propagation",
            "partition-heal",
        ],
        106,
    ),
    ("network-split", &["split-both-forge"], 106),
    (
        "lockstep",
        &[
            "burns",
            "last-block-vote",
            "milestone-activation",
            "reorg-reverts-registrations",
            "reorg-reverts-votes",
            "zero-vote-ties",
        ],
        u32::MAX,
    ),
];

/// The record that shows one level of one scenario.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Proof {
    /// The scenario's own run through the lockstep backend.
    Lockstep,
    /// The scenario's own run in the suite of its directory, on both implementations, compared.
    Suite,
    /// The boot check script's own record.
    BootCheck,
    /// The mixed forging run of reference and Heartwood nodes, at its steps for the scenario.
    Mixed,
    /// The mixed forging run that partitions the network, at its steps for the scenario.
    MixedSplit,
    /// The live record of a Heartwood node syncing from reference peers.
    SyncFromReference,
    /// The live records of a reference node syncing from a Heartwood relay.
    SyncFromHeartwood,
}

use Level::{L2, L3, L4};
use Proof::{BootCheck, Lockstep, Mixed, MixedSplit, Suite, SyncFromHeartwood, SyncFromReference};

/// Which record proves each level of each scenario, by id. A scenario's L2 level is its run
/// through the lockstep backend, its L3 level its run in its suite, its L4 level a step of the
/// mixed forging runs. Four are exceptions: the boot check's L3 level is its own script's record,
/// and the sync from genesis has no lockstep run (it needs a live reference network), so its L2
/// level is the live record of the sync from reference peers and its L4 level the live records of
/// the reference syncing from a Heartwood relay.
const LEVEL_MAP: &[(&str, Level, Proof)] = &[
    ("boot-check", L2, Lockstep),
    ("boot-check", L3, BootCheck),
    ("chained-transfers", L2, Lockstep),
    ("chained-transfers", L3, Suite),
    ("chained-transfers", L4, Mixed),
    ("double-spend", L2, Lockstep),
    ("double-spend", L3, Suite),
    ("double-spend", L4, Mixed),
    ("large-transfers", L2, Lockstep),
    ("large-transfers", L3, Suite),
    ("burns-and-supply", L3, Suite),
    ("second-signature-and-ipfs", L3, Suite),
    ("insufficient-balance", L2, Lockstep),
    ("insufficient-balance", L3, Suite),
    ("repeated-operations", L2, Lockstep),
    ("repeated-operations", L3, Suite),
    ("pool-survives-restart", L3, Suite),
    ("attestation", L3, Suite),
    ("attestation", L4, Mixed),
    ("missed-slots", L3, Suite),
    ("vote-in-last-block", L3, Suite),
    ("relay-forger-split", L3, Suite),
    ("restarts", L3, Suite),
    ("delegate-resignation", L2, Lockstep),
    ("delegate-resignation", L3, Suite),
    ("sync-from-genesis", L2, SyncFromReference),
    ("sync-from-genesis", L3, Suite),
    ("sync-from-genesis", L4, SyncFromHeartwood),
    ("peer-discovery", L3, Suite),
    ("peer-discovery", L4, Mixed),
    ("propagation", L3, Suite),
    ("propagation", L4, Mixed),
    ("partition-heal", L3, Suite),
    ("partition-heal", L4, MixedSplit),
    ("split-both-forge", L3, Suite),
    ("split-both-forge", L4, MixedSplit),
    ("burns", L2, Lockstep),
    ("last-block-vote", L2, Lockstep),
    ("milestone-activation", L2, Lockstep),
    ("reorg-reverts-registrations", L2, Lockstep),
    ("reorg-reverts-votes", L2, Lockstep),
    ("zero-vote-ties", L2, Lockstep),
];

/// The levels a file does not list that a live record proves all the same.
const LEVELS_OUTSIDE_THE_FILE: &[(&str, Level)] = &[("sync-from-genesis", L2)];

fn scenario_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("scenarios")
}

/// Every scenario file as (directory, file stem, scenario), in path order.
fn all() -> Vec<(String, String, Scenario)> {
    let mut out = Vec::new();
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(scenario_root())
        .expect("the scenario directory reads")
        .map(|entry| entry.expect("an entry").path())
        .filter(|path| path.is_dir())
        .collect();
    dirs.sort();
    for dir in dirs {
        let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
            .expect("a suite directory reads")
            .map(|entry| entry.expect("an entry").path())
            .filter(|path| path.extension().is_some_and(|e| e == "json"))
            .collect();
        files.sort();
        for file in files {
            let scenario = scenario::load(&file)
                .unwrap_or_else(|e| panic!("{} does not read: {e}", file.display()));
            let name = |p: &Path| {
                p.file_stem()
                    .or_else(|| p.file_name())
                    .and_then(|s| s.to_str())
                    .expect("a utf-8 name")
                    .to_string()
            };
            out.push((name(&dir), name(&file), scenario));
        }
    }
    out
}

/// A file's stem without the order prefix (`02-double-spend` is `double-spend`).
fn without_prefix(stem: &str) -> &str {
    match stem.split_once('-') {
        Some((number, rest)) if number.chars().all(|c| c.is_ascii_digit()) => rest,
        _ => stem,
    }
}

#[test]
fn every_suite_directory_holds_the_scenarios_it_names_and_no_other() {
    let mut found: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (dir, _, scenario) in all() {
        found.entry(dir).or_default().insert(scenario.id);
    }
    let named: BTreeMap<String, BTreeSet<String>> = SUITES
        .iter()
        .map(|(dir, ids, _)| {
            (
                (*dir).to_string(),
                ids.iter().map(|id| (*id).to_string()).collect(),
            )
        })
        .collect();
    assert_eq!(found, named);
}

#[test]
fn a_file_is_named_for_its_id_and_every_id_is_unique() {
    let mut seen = BTreeSet::new();
    for (dir, stem, scenario) in all() {
        let expected = if dir == "lockstep" || stem == "delegate-resignation" {
            stem.as_str()
        } else {
            without_prefix(&stem)
        };
        assert_eq!(scenario.id, expected, "{dir}/{stem}");
        assert!(seen.insert(scenario.id.clone()), "{} is twice", scenario.id);
    }
}

#[test]
fn the_suites_run_in_numbered_order() {
    for (dir, _, _) in all().iter().filter(|(d, _, _)| d != "lockstep") {
        let mut stems: Vec<String> = all()
            .into_iter()
            .filter(|(d, _, _)| d == dir)
            .map(|(_, stem, _)| stem)
            .filter(|stem| stem.starts_with(|c: char| c.is_ascii_digit()))
            .collect();
        let sorted = {
            let mut s = stems.clone();
            s.sort();
            s
        };
        assert_eq!(stems, sorted, "{dir}");
        stems.dedup();
        let numbers: Vec<usize> = stems
            .iter()
            .map(|s| s[..2].parse().expect("a two-digit prefix"))
            .collect();
        assert_eq!(numbers, (0..numbers.len()).collect::<Vec<_>>(), "{dir}");
    }
}

#[test]
fn every_scenario_is_on_the_public_devnet_with_its_own_actors() {
    for (dir, _, scenario) in all() {
        let id = &scenario.id;
        assert_eq!(scenario.network, "devnet", "{id}");
        assert_eq!(scenario.genesis_seed, SEED, "{id}");
        assert!(!scenario.title.is_empty(), "{id}");
        assert!(!scenario.steps.is_empty(), "{id}");
        let mut seeds = BTreeSet::new();
        for (name, actor) in &scenario.actors {
            assert!(seeds.insert(actor.seed), "{id}: {name} repeats a seed");
        }
        // Five nodes, and the lifecycle suite's two extra relays (6 and 7) for the failover.
        let nodes = match dir.as_str() {
            "lockstep" => 1,
            "lifecycle" => 7,
            _ => 5,
        };
        for step in &scenario.steps {
            assert!(
                step.node.is_none_or(|n| (1..=nodes).contains(&n)),
                "{id}: a step names a node outside 1..={nodes}"
            );
        }
        if dir != "network-split" {
            assert!(
                scenario.env.is_empty(),
                "{id}: only the split suite sets env"
            );
        }
    }
}

#[test]
fn the_split_suite_lowers_the_reach_its_minority_needs_on_every_node() {
    let (_, _, split) = all()
        .into_iter()
        .find(|(dir, _, _)| dir == "network-split")
        .expect("the split scenario exists");
    assert_eq!(
        split
            .env
            .get("CORE_P2P_MIN_NETWORK_REACH")
            .map(String::as_str),
        Some("20")
    );
    assert_eq!(split.env.len(), 1);
}

#[test]
fn every_trigger_fires_before_its_suite_stops() {
    for (dir, _, scenario) in all() {
        let planned = SUITES
            .iter()
            .find(|(d, _, _)| *d == dir)
            .map(|(_, _, planned)| *planned)
            .expect("a known suite");
        let fires = fire_heights(&scenario, ROUND_LENGTH)
            .unwrap_or_else(|e| panic!("{}: the triggers do not plan: {e}", scenario.id));
        assert_eq!(fires.len(), scenario.steps.len());
        let mut last = 0;
        for (step, fire) in scenario.steps.iter().zip(&fires) {
            let after = step
                .check
                .as_ref()
                .and_then(|c| c.within_blocks)
                .unwrap_or(0);
            last = last.max(fire + after);
        }
        assert!(
            last < planned,
            "{}: a step ends at height {last}, the suite stops at {planned}",
            scenario.id
        );
    }
}

#[test]
fn a_suite_scenario_lists_a_devnet_level_and_a_lockstep_scenario_only_the_producer_level() {
    for (dir, _, scenario) in all() {
        if dir == "lockstep" {
            assert_eq!(scenario.levels, [L2], "{}", scenario.id);
        } else {
            assert!(
                scenario.levels.contains(&L3),
                "{} lists no devnet level",
                scenario.id
            );
        }
        let unique: BTreeSet<_> = scenario.levels.iter().collect();
        assert_eq!(unique.len(), scenario.levels.len(), "{}", scenario.id);
    }
}

#[test]
fn every_level_a_scenario_lists_has_a_record_that_proves_it() {
    let mut listed: BTreeSet<(String, Level)> = BTreeSet::new();
    for (_, _, scenario) in all() {
        for level in &scenario.levels {
            listed.insert((scenario.id.clone(), *level));
        }
    }
    let mapped: BTreeSet<(String, Level)> = LEVEL_MAP
        .iter()
        .map(|(id, level, _)| ((*id).to_string(), *level))
        .collect();
    assert_eq!(mapped.len(), LEVEL_MAP.len(), "a level is mapped twice");
    for entry in &listed {
        assert!(mapped.contains(entry), "{entry:?} has no record");
    }
    for (id, level) in mapped.difference(&listed) {
        assert!(
            LEVELS_OUTSIDE_THE_FILE.contains(&(id.as_str(), *level)),
            "{id} at {level:?} is mapped but not listed"
        );
    }
}

#[test]
fn a_record_fits_the_level_and_the_suite_it_is_named_for() {
    let files = all();
    for (id, level, proof) in LEVEL_MAP {
        let (dir, _, _) = files
            .iter()
            .find(|(_, _, s)| s.id == *id)
            .unwrap_or_else(|| panic!("{id} has no file"));
        let fits = match proof {
            Proof::Lockstep => *level == L2,
            Proof::Suite => *level == L3 && dir != "lockstep",
            Proof::BootCheck => *level == L3 && dir == "transactions",
            Proof::Mixed | Proof::SyncFromHeartwood => *level == L4,
            Proof::MixedSplit => *level == L4 && dir.starts_with("network"),
            Proof::SyncFromReference => *level == L2 && dir == "network",
        };
        assert!(fits, "{id} at {level:?}: {proof:?} does not fit");
    }
}

#[test]
fn the_committed_scenarios_carry_no_internal_labels() {
    let banned: Vec<String> = [
        ("S", "-0"),
        ("S", "-1"),
        ("S", "-2"),
        ("W", "1"),
        ("W", "2"),
        ("W", "3"),
        ("B", "8"),
        ("G", "2"),
        ("L", "-2"),
        ("D", "28"),
    ]
    .iter()
    .map(|(head, tail)| format!("{head}{tail}"))
    .collect();
    let mut files = Vec::new();
    for dir in std::fs::read_dir(scenario_root()).expect("reads") {
        let dir = dir.expect("an entry").path();
        for file in std::fs::read_dir(&dir).expect("reads") {
            files.push(file.expect("an entry").path());
        }
    }
    assert!(files.len() > 20);
    for file in files {
        let text = std::fs::read_to_string(&file).expect("utf-8");
        for label in &banned {
            assert!(!text.contains(label), "{} carries {label}", file.display());
        }
    }
}

// Integration: the real reference.

fn oracle_flags() -> Option<Vec<String>> {
    if let Ok(dir) = std::env::var("DIFFTEST_TEST_SOLAR_DIR") {
        return Some(vec!["--solar-dir".into(), dir]);
    }
    std::env::var("DIFFTEST_TEST_IMAGE")
        .ok()
        .map(|image| vec!["--image".into(), image])
}

#[test]
#[ignore = "integration"]
fn every_scenario_compiles_through_the_reference() {
    let Some(oracle) = oracle_flags() else {
        assert!(
            std::env::var_os("DIFFTEST_TEST_NO_ORACLE").is_some(),
            "no test oracle configured: set DIFFTEST_TEST_IMAGE or DIFFTEST_TEST_SOLAR_DIR, or \
             DIFFTEST_TEST_NO_ORACLE to skip the test"
        );
        eprintln!(
            "every_scenario_compiles_through_the_reference: skipped (DIFFTEST_TEST_NO_ORACLE)"
        );
        return;
    };
    let scratch =
        heartwood_difftest::store::ScratchDir::new("scenario-devnet").expect("a scratch directory");
    for entry in std::fs::read_dir(scenario_root()).expect("reads") {
        let dir = entry.expect("an entry").path();
        for file in std::fs::read_dir(&dir).expect("reads") {
            let file = file.expect("an entry").path();
            let out = scratch.path().join("compiled.json");
            let result = std::process::Command::new(env!("CARGO_BIN_EXE_difftest"))
                .args(["scenario", "compile"])
                .arg(&file)
                .arg("--out")
                .arg(&out)
                .args(&oracle)
                .output()
                .expect("difftest runs");
            assert!(
                result.status.success(),
                "{} does not compile:\n{}{}",
                file.display(),
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
        }
    }
}
