//! `difftest live check` on a recorded run of the core network: three nodes, each one `core`
//! process running the relay and the forger together (run `core`: two rounds, 18, 18 and 17
//! delegates).
//!
//! The run's records (`tests/fixtures/live/core/`: its steps, lock holders, status snapshot, tips,
//! the three nodes' digest, block, await, attestation and network-state files and logs, and the
//! resource series) are a recording of the recipe's run, with the devnet's addresses replaced by
//! documentation addresses. Its chain is the judged corpus in `corpus/`, so no oracle runs. The
//! nodes' stores are the corpus replayed through Heartwood's store.
//!
//! The recorded series wrote more to disk than the node budget allows, so the recording fails
//! exactly that case; the tests that need a clean run rewrite the written bytes.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use heartwood_difftest::Error;
use heartwood_difftest::corpus::Corpus;
use heartwood_difftest::live::{self, LiveOptions, resources};
use heartwood_difftest::report::{Outcome, Report};
use heartwood_difftest::store::{self, ScratchDir};
use heartwood_difftest::vectors::sha256_hex;
use serde_json::{Value, json};

const BINARY: &[u8] = b"the release binary";
const CORPUS: &str = "live-core";
const NODES: [&str; 3] = ["node1", "node2", "node3"];

fn recording() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/live/core")
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.path().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).unwrap();
        }
    }
}

/// The recorded chain's store, replayed once for every test.
fn replayed_store() -> &'static Path {
    static STORE: OnceLock<(ScratchDir, PathBuf)> = OnceLock::new();
    let (_, data) = STORE.get_or_init(|| {
        let scratch = ScratchDir::new("core-run-store").unwrap();
        let data = scratch.path().join("data");
        let loaded = Corpus::load(&recording().join("corpus")).unwrap();
        store::replay(&loaded, &data, None).expect("the recorded chain replays through the store");
        (scratch, data)
    });
    data
}

fn read_json(path: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn read_lines(path: &Path) -> Vec<Value> {
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

fn write_lines(path: &Path, lines: &[Value]) {
    let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
    std::fs::write(path, text).unwrap();
}

/// A run directory, a corpus root holding the judged chain and the tree's release binary (a
/// stand-in file whose SHA-256 the run records).
struct Fixture {
    _scratch: ScratchDir,
    run: PathBuf,
    root: PathBuf,
    relay: PathBuf,
}

fn fixture() -> Fixture {
    let scratch = ScratchDir::new("core-run-fixture").unwrap();
    let run = scratch.path().join("results/core/1");
    let root = scratch.path().join("corpus");
    copy_dir(&recording(), &run);
    std::fs::remove_dir_all(run.join("corpus")).unwrap();
    copy_dir(&recording().join("corpus"), &root.join(CORPUS));
    copy_dir(
        &recording().join("corpus/crypto"),
        &run.join("network/crypto"),
    );
    let relay = scratch.path().join("heartwood");
    std::fs::write(&relay, BINARY).unwrap();
    let mut manifest = read_json(&run.join("run.json"));
    manifest["relayImage"] = json!("heartwood-relay:fixture");
    manifest["relayBinarySha256"] = json!(sha256_hex(BINARY));
    std::fs::write(run.join("run.json"), manifest.to_string()).unwrap();
    for node in NODES {
        copy_dir(replayed_store(), &run.join("nodes").join(node).join("data"));
    }
    Fixture {
        _scratch: scratch,
        run,
        root,
        relay,
    }
}

fn check(f: &Fixture) -> Report {
    let no_oracle = || -> heartwood_difftest::Result<_> {
        Err(Error::Usage(
            "the fixture's chains are judged: no oracle".into(),
        ))
    };
    live::check_with(
        &f.run,
        &LiveOptions {
            corpus_root: f.root.clone(),
            oracle: &no_oracle,
            relay_binary: f.relay.clone(),
            compare: None,
            spread: Vec::new(),
        },
    )
    .expect("the live check runs")
}

fn labels(report: &Report) -> Vec<String> {
    report.results.iter().map(|r| r.label.clone()).collect()
}

fn failures(report: &Report) -> Vec<String> {
    report
        .results
        .iter()
        .filter(|r| r.outcome.is_failure())
        .map(|r| match &r.outcome {
            Outcome::HarnessError(m) => format!("{}: {m}", r.label),
            other => format!("{}: {other:?}", r.label),
        })
        .collect()
}

fn has(list: &[String], needle: &str) -> bool {
    list.iter().any(|l| l.contains(needle))
}

/// Rewrite every series so each node wrote half a megabyte over the run: within the disk budget.
fn quiet_disks(run: &Path) {
    for node in NODES {
        let path = run.join("resources").join(format!("{node}-core.jsonl"));
        let mut rows = read_lines(&path);
        let last = rows.len() as u64;
        for (i, row) in rows.iter_mut().enumerate() {
            row["written"] = json!(500_000 * (i as u64 + 1) / last);
        }
        write_lines(&path, &rows);
    }
}

/// Replace the recorded step `name` with `change` applied to it.
fn edit_step(run: &Path, name: &str, change: impl FnOnce(&mut Value)) {
    let path = run.join("steps.jsonl");
    let mut rows = read_lines(&path);
    let row = rows
        .iter_mut()
        .find(|r| r["step"] == name)
        .expect("the recording has the step");
    change(row);
    write_lines(&path, &rows);
}

fn edit_json(path: &Path, change: impl FnOnce(&mut Value)) {
    let mut value = read_json(path);
    change(&mut value);
    std::fs::write(path, value.to_string()).unwrap();
}

#[test]
fn a_recorded_run_passes_every_section_but_its_disk_writes() {
    let f = fixture();
    let report = check(&f);
    let failed = failures(&report);
    // The recorded nodes wrote about fourteen megabytes in fourteen minutes: over the day's budget.
    assert_eq!(failed.len(), 3, "{failed:#?}");
    for node in NODES {
        assert!(
            has(
                &failed,
                &format!("{node} disk writes in a day (extrapolated)")
            ),
            "{failed:#?}"
        );
    }
    let all = labels(&report);
    for needle in [
        "each node runs one process holding the core, relay and forger locks",
        "the status lists one online core process per node, none restarted",
        "`relay start` was refused while core runs, and changed no process",
        "every node's delegates forged in every round",
        "every block followed a quorum answer to its own node's forger in its slot",
        "every node counted all 53 delegates in one answer after height 10",
        "one tip at the end",
        "`devnet stop` stopped every node within 30 seconds",
        "every node's log ends with the clean stop",
        "every step in its planned round and order",
        "core node1 resident memory outside a full pool (95th percentile)",
        "the five nodes' CPU time in quiet windows",
    ] {
        assert!(has(&all, needle), "{needle}\n{all:#?}");
    }
}

#[test]
fn a_recorded_run_with_quiet_disks_passes() {
    let f = fixture();
    quiet_disks(&f.run);
    let report = check(&f);
    assert!(report.is_success(), "{report}\n{:#?}", failures(&report));
}

#[test]
fn the_resource_report_reads_the_recorded_series() {
    let f = fixture();
    let report = resources::report(&f.run).expect("the report is made");
    let failed = failures(&report);
    assert_eq!(failed.len(), 3, "{failed:#?}");
    assert!(
        failed.iter().all(|m| m.contains("disk writes in a day")),
        "{failed:#?}"
    );
    let all = labels(&report);
    assert!(
        has(&all, "core node3 proportional memory (highest, recorded)"),
        "{all:#?}"
    );
    quiet_disks(&f.run);
    let clean = resources::report(&f.run).expect("the report is made");
    assert!(clean.is_success(), "{:#?}", failures(&clean));
}

#[test]
fn a_lock_held_by_another_process_fails() {
    let f = fixture();
    quiet_disks(&f.run);
    edit_json(&f.run.join("locks.json"), |v| {
        v["nodes"]["node2"]["holders"]["forger.lock"] = json!([4242]);
        v["nodes"]["node3"]["children"] = json!(1);
    });
    let failed = failures(&check(&f));
    assert!(
        has(&failed, "node2: forger.lock is held by [4242]"),
        "{failed:#?}"
    );
    assert!(
        has(&failed, "node3: the core process has children"),
        "{failed:#?}"
    );
}

#[test]
fn a_node_with_a_separate_relay_process_fails() {
    let f = fixture();
    quiet_disks(&f.run);
    edit_json(&f.run.join("status.json"), |v| {
        let first = &mut v["nodes"][0]["processes"];
        let relay = json!({"role": "relay", "status": "online", "restarts": 0});
        first.as_array_mut().unwrap().push(relay);
    });
    let failed = failures(&check(&f));
    assert!(has(&failed, "not one core process"), "{failed:#?}");
}

#[test]
fn a_relay_start_that_was_not_refused_fails() {
    let f = fixture();
    quiet_disks(&f.run);
    edit_step(&f.run, "relay-start-refused", |row| {
        row["exit"] = json!(0);
        row["pidsAfter"] = json!("1512312 1512354 1512396 77 ");
    });
    let failed = failures(&check(&f));
    assert!(
        has(&failed, "`relay start` exited with success while core runs"),
        "{failed:#?}"
    );
    assert!(has(&failed, "the nodes' processes changed"), "{failed:#?}");
}

#[test]
fn a_slow_stop_fails() {
    let f = fixture();
    quiet_disks(&f.run);
    edit_step(&f.run, "stop", |row| row["seconds"] = json!(31.5));
    let failed = failures(&check(&f));
    assert!(
        has(&failed, "`devnet stop` took 31.5 seconds, more than 30"),
        "{failed:#?}"
    );
}

#[test]
fn a_log_without_the_clean_stop_fails() {
    let f = fixture();
    quiet_disks(&f.run);
    let log = f.run.join("nodes/node2/log/node2-devnet-core-current.log");
    let text = std::fs::read_to_string(&log).unwrap();
    std::fs::write(
        &log,
        text.replace("The node has stopped", "The node was killed"),
    )
    .unwrap();
    let failed = failures(&check(&f));
    assert!(
        has(&failed, "node2: its log has no `The node has stopped` line"),
        "{failed:#?}"
    );
}

#[test]
fn a_node_whose_blocks_are_missing_fails() {
    let f = fixture();
    quiet_disks(&f.run);
    // Node 3's block file ends at height 50: its delegates' later blocks are nowhere to be read.
    let path = f.run.join("nodes/node3/difftest/digest-0001.jsonl");
    let rows: Vec<Value> = read_lines(&path)
        .into_iter()
        .filter(|r| r["h"].as_u64().is_none_or(|h| h <= 50))
        .collect();
    write_lines(&path, &rows);
    let report = check(&f);
    assert!(!report.is_success(), "{report}");
}

#[test]
fn a_dry_run_of_the_recipe_is_no_evidence() {
    let f = fixture();
    quiet_disks(&f.run);
    edit_json(&f.run.join("run.json"), |m| m["dryRun"] = json!(true));
    let report = check(&f);
    assert!(!report.is_success(), "{report}");
}
