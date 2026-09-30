//! `difftest live check` on a recorded run of the churn network: six Heartwood nodes (nodes 1 to 5
//! each hosting delegates, the fifth with its forger at an address of its own) and an empty
//! reference relay, five rounds (run `churn`: 53 delegates, 265 blocks).
//!
//! The run restarts nodes 1 to 5 in turn, kills a relay and a forger under their supervisors,
//! fills the transaction pool to its limit with the forgers paused, stops two and then three nodes
//! (the chain halts at three), and starts an empty reference relay that may only reach node 2.
//!
//! The run's records (`tests/fixtures/live/churn/`: its steps, fault windows, status snapshot,
//! tips, block arrivals, the six nodes' digest, block, await, attestation and network-state files
//! and logs, and the resource series) are a recording of the recipe's run, with the devnet's
//! addresses replaced by documentation addresses. Files over a few tens of kilobytes are stored
//! compressed (`.gz`), and a node's block files are stored as the heights they hold (`.range`): the
//! blocks themselves are the judged corpus's, the same lines in every node's file. Its chain is
//! the judged corpus in `corpus/`, so no oracle runs. The nodes' stores are the corpus replayed
//! through Heartwood's store.
//!
//! The recorded series used more memory and wrote more to disk than the node budgets allow, so the
//! recording fails exactly those cases; the tests that need a clean run rewrite the series.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use flate2::read::GzDecoder;
use heartwood_difftest::Error;
use heartwood_difftest::corpus::Corpus;
use heartwood_difftest::live::{self, LiveOptions, resources};
use heartwood_difftest::report::{Outcome, Report};
use heartwood_difftest::store::{self, ScratchDir};
use heartwood_difftest::vectors::sha256_hex;
use serde_json::{Value, json};

const BINARY: &[u8] = b"the release binary";
const CORPUS: &str = "live-churn";
const NODES: [&str; 6] = ["node1", "node2", "node3", "node4", "node5", "node6"];

fn recording() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/live/churn")
}

fn gunzip(path: &Path) -> Vec<u8> {
    let mut out = Vec::new();
    GzDecoder::new(std::fs::File::open(path).unwrap())
        .read_to_end(&mut out)
        .unwrap();
    out
}

/// The judged chain's block lines by height: every node's block file holds a run of them.
fn chain() -> &'static Vec<(u64, String)> {
    static CHAIN: OnceLock<Vec<(u64, String)>> = OnceLock::new();
    CHAIN.get_or_init(|| {
        let text = String::from_utf8(gunzip(&recording().join("corpus/blocks.jsonl.gz"))).unwrap();
        text.lines()
            .map(|l| {
                let v: Value = serde_json::from_str(l).unwrap();
                (v["h"].as_u64().unwrap(), format!("{l}\n"))
            })
            .collect()
    })
}

/// Copy the recorded directory `from` to `to`: `.gz` files decompressed, `.range` files turned into
/// the block file they stand for.
fn restore_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if path.is_dir() {
            restore_dir(&path, &to.join(&name));
        } else if let Some(plain) = name.strip_suffix(".gz") {
            std::fs::write(to.join(plain), gunzip(&path)).unwrap();
        } else if let Some(stem) = name.strip_suffix(".range") {
            let text = std::fs::read_to_string(&path).unwrap();
            let blocks = match text.trim().split_once('-') {
                Some((lo, hi)) => {
                    let (lo, hi): (u64, u64) = (lo.parse().unwrap(), hi.parse().unwrap());
                    chain()
                        .iter()
                        .filter(|(h, _)| (lo..=hi).contains(h))
                        .map(|(_, line)| line.as_str())
                        .collect::<String>()
                }
                None => String::new(),
            };
            std::fs::write(to.join(format!("{stem}.bin")), blocks).unwrap();
        } else {
            std::fs::copy(&path, to.join(&name)).unwrap();
        }
    }
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
        let scratch = ScratchDir::new("churn-run-store").unwrap();
        let data = scratch.path().join("data");
        let corpus = scratch.path().join("corpus");
        restore_dir(&recording().join("corpus"), &corpus);
        let loaded = Corpus::load(&corpus).unwrap();
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
    let scratch = ScratchDir::new("churn-run-fixture").unwrap();
    let run = scratch.path().join("results/churn/1");
    let root = scratch.path().join("corpus");
    restore_dir(&recording(), &run);
    std::fs::remove_dir_all(run.join("corpus")).unwrap();
    restore_dir(&recording().join("corpus"), &root.join(CORPUS));
    copy_dir(
        &root.join(CORPUS).join("crypto"),
        &run.join("network/crypto"),
    );
    // The archive the chain was imported from: only the end it records is read, to see that the
    // judged corpus is the one this run made.
    let judged = Corpus::load(&root.join(CORPUS)).unwrap();
    let archive = run.join("archives/node7/20260930T223445Z");
    std::fs::create_dir_all(&archive).unwrap();
    let id = judged.block_id(judged.tip()).unwrap().to_hex();
    let epoch = judged.meta.pointer("/chain/epoch").cloned().unwrap();
    std::fs::write(
        archive.join("manifest.json"),
        json!({"range": {"to": judged.tip(), "lastId": id}, "chain": {"epoch": epoch}}).to_string(),
    )
    .unwrap();
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

/// Rewrite every series so each node stays within its memory and disk budgets: a forger at 10 MB,
/// a relay at 50 MB and a tenth of a megabyte written by each process over the run.
fn quiet_series(run: &Path) {
    for entry in std::fs::read_dir(run.join("resources")).unwrap() {
        let path = entry.unwrap().path();
        let cap: u64 = if path.to_string_lossy().contains("forger") {
            10_000_000
        } else {
            50_000_000
        };
        let mut rows = read_lines(&path);
        let last = rows.len() as u64;
        for (i, row) in rows.iter_mut().enumerate() {
            for key in ["rss", "pss"] {
                let value = row[key].as_u64().unwrap_or(0).min(cap);
                row[key] = json!(value);
            }
            row["written"] = json!(100_000 * (i as u64 + 1) / last);
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
fn a_recorded_run_passes_every_section_but_its_resources() {
    let f = fixture();
    let report = check(&f);
    let failed = failures(&report);
    assert!(!failed.is_empty());
    assert!(
        failed.iter().all(|m| m.contains("live-churn/resources")
            || m.starts_with("relay ")
            || m.contains("resident memory")
            || m.contains("disk writes in a day")),
        "{failed:#?}"
    );
    let all = labels(&report);
    for needle in [
        "nodes 1 to 5 each stopped gracefully within 30 seconds",
        "each restarted relay logged that its stop completed",
        "the killed relay and forger restarted under their own supervisors",
        "two nodes down: the chain forged on; three down: it halted",
        "the pool held 15000 transactions with the forgers paused, then drained",
        "node 5's forger runs at an address of its own",
        "the empty reference relay synced from node 2's relay alone",
        "every Heartwood node's roles end online and stable",
        "every scripted event has its fault window",
        "every delegate's node forged in every round outside its fault windows",
        "no slot was missed outside a fault window",
        "every block followed a quorum answer to its own node's forger in its slot",
        "every relay counted the delegates of nodes 1 to 4 in one answer after height 10",
        "one tip at the end",
        "every step in its planned round and order",
    ] {
        assert!(has(&all, needle), "{needle}\n{all:#?}");
    }
}

#[test]
fn over_budget_series_are_reported_with_their_numbers() {
    let f = fixture();
    let failed = failures(&check(&f));
    for needle in [
        "relay node1 resident memory outside a full pool (95th percentile)",
        "relay node1 resident memory with a full pool (highest)",
        "forger node1 resident memory (95th percentile)",
        "the five nodes' resident memory together (highest pass outside a full pool)",
        "the five nodes' disk writes in a day (extrapolated from the quiet windows)",
    ] {
        assert!(has(&failed, needle), "{needle}\n{failed:#?}");
    }
    assert!(has(&failed, "not re-baselined"), "{failed:#?}");
    // Each node's writes over the whole run, the full pool included, are recorded beside it.
    let all = labels(&check(&f));
    assert!(
        has(&all, "recorded: node6 disk writes over the whole run"),
        "{all:#?}"
    );
}

#[test]
fn a_recorded_run_with_quiet_series_passes() {
    let f = fixture();
    quiet_series(&f.run);
    let report = check(&f);
    assert!(report.is_success(), "{report}\n{:#?}", failures(&report));
}

#[test]
fn the_resource_report_reads_the_recorded_series() {
    let f = fixture();
    let report = resources::report(&f.run).expect("the report is made");
    let failed = failures(&report);
    assert!(
        has(
            &failed,
            "the five nodes' disk writes in a day (extrapolated from the quiet windows)"
        ),
        "{failed:#?}"
    );
    assert!(
        has(
            &failed,
            "relay node3 resident memory with a full pool (highest)"
        ),
        "{failed:#?}"
    );
    quiet_series(&f.run);
    let clean = resources::report(&f.run).expect("the report is made");
    assert!(clean.is_success(), "{:#?}", failures(&clean));
}

#[test]
fn a_slow_or_unclean_stop_fails() {
    let f = fixture();
    quiet_series(&f.run);
    edit_step(&f.run, "cycle-node2", |row| row["stopSeconds"] = json!(30));
    edit_step(&f.run, "cycle-node3", |row| {
        row["remaining"] = json!("pids 4242");
    });
    let failed = failures(&check(&f));
    assert!(
        has(
            &failed,
            "cycle-node2: the stop took 30 seconds, not less than 30"
        ),
        "{failed:#?}"
    );
    assert!(
        has(&failed, "cycle-node3: processes were left after the stop"),
        "{failed:#?}"
    );
}

#[test]
fn a_relay_log_without_its_stop_line_fails() {
    let f = fixture();
    quiet_series(&f.run);
    let log = f
        .run
        .join("nodes/node4/log/solar-testnet-relay-current.log");
    let text = std::fs::read_to_string(&log).unwrap();
    std::fs::write(
        &log,
        text.replace("The node has stopped", "The node was killed"),
    )
    .unwrap();
    let failed = failures(&check(&f));
    assert!(
        has(
            &failed,
            "node4: its relay's log has no `The node has stopped` line within its restart"
        ),
        "{failed:#?}"
    );
}

#[test]
fn a_node_not_synced_after_its_restart_fails() {
    let f = fixture();
    quiet_series(&f.run);
    edit_step(&f.run, "cycle-node5", |row| row["synced"] = json!(false));
    let failed = failures(&check(&f));
    assert!(
        has(
            &failed,
            "cycle-node5: the node did not answer and sync again"
        ),
        "{failed:#?}"
    );
}

#[test]
fn a_kill_without_a_restart_under_the_same_supervisor_fails() {
    let f = fixture();
    quiet_series(&f.run);
    edit_step(&f.run, "crash-relay-node3", |row| {
        row["restartAfter"] = json!(0);
    });
    edit_step(&f.run, "crash-forger-node2", |row| {
        row["supervisorAfter"] = json!(1);
    });
    let failed = failures(&check(&f));
    assert!(
        has(
            &failed,
            "crash-relay-node3: no restart of the relay under the same supervisor"
        ),
        "{failed:#?}"
    );
    assert!(
        has(
            &failed,
            "crash-forger-node2: no restart of the forger under the same supervisor"
        ),
        "{failed:#?}"
    );
}

#[test]
fn a_halt_without_evidence_fails() {
    let f = fixture();
    quiet_series(&f.run);
    edit_step(&f.run, "outage-three", |row| {
        row["heightAfter"] = json!(165);
        row["logLine"] = json!("nothing of the kind");
    });
    let failed = failures(&check(&f));
    assert!(
        has(&failed, "outage-three: no halt evidence"),
        "{failed:#?}"
    );
}

#[test]
fn a_chain_that_did_not_forge_during_two_down_fails() {
    let f = fixture();
    quiet_series(&f.run);
    edit_step(&f.run, "outage-two", |row| {
        row["heightAfter"] = row["heightBefore"].clone()
    });
    let failed = failures(&check(&f));
    assert!(
        has(&failed, "outage-two: no progress evidence"),
        "{failed:#?}"
    );
}

#[test]
fn a_pool_that_stayed_below_its_limit_or_never_drained_fails() {
    let f = fixture();
    quiet_series(&f.run);
    edit_step(&f.run, "full-pool", |row| {
        row["peak"] = json!(9000);
        row["drained"] = json!(false);
    });
    let failed = failures(&check(&f));
    assert!(
        has(
            &failed,
            "full-pool: the pool peaked at 9000, below its limit 15000"
        ),
        "{failed:#?}"
    );
    assert!(
        has(&failed, "full-pool: the pool did not drain"),
        "{failed:#?}"
    );
}

#[test]
fn a_vote_that_was_not_refused_as_offline_fails() {
    let f = fixture();
    quiet_series(&f.run);
    edit_step(&f.run, "vote-node5", |row| {
        row["detail"]["outcome"] = json!("accepted");
        row["detail"]["message"] = json!("");
    });
    let failed = failures(&check(&f));
    assert!(
        has(
            &failed,
            "vote-node5: the vote for node5's delegate was not refused"
        ),
        "{failed:#?}"
    );
}

#[test]
fn a_forger_at_its_relays_own_address_fails() {
    let f = fixture();
    quiet_series(&f.run);
    edit_json(&f.run.join("run.json"), |m| {
        let node = m["nodes"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|n| n["name"] == "node5")
            .unwrap();
        node["forgerAddress"] = node["ip"].clone();
    });
    let failed = failures(&check(&f));
    assert!(
        has(
            &failed,
            "node5: its forger does not run at an address of its own"
        ),
        "{failed:#?}"
    );
}

#[test]
fn a_reference_relay_that_saw_another_peer_fails() {
    let f = fixture();
    quiet_series(&f.run);
    edit_step(&f.run, "sync-node7", |row| {
        row["peers"] = json!(["198.51.100.12", "198.51.100.13"]);
    });
    let failed = failures(&check(&f));
    assert!(has(&failed, "sync-node7: its peers were"), "{failed:#?}");
}

#[test]
fn a_role_that_ends_offline_fails() {
    let f = fixture();
    quiet_series(&f.run);
    edit_json(&f.run.join("status.json"), |v| {
        v["nodes"][2]["processes"][1]["status"] = json!("stopped");
        v["nodes"][3]["processes"][0]["unstable"] = json!(true);
    });
    let failed = failures(&check(&f));
    assert!(has(&failed, "its forger is stopped"), "{failed:#?}");
    assert!(
        has(&failed, "its relay is online (unstable)"),
        "{failed:#?}"
    );
}

#[test]
fn a_missing_fault_window_fails() {
    let f = fixture();
    quiet_series(&f.run);
    let path = f.run.join("faults.jsonl");
    let rows: Vec<Value> = read_lines(&path)
        .into_iter()
        .filter(|r| r["fault"] != "supervised-forger" && r["fault"] != "outage-three")
        .collect();
    write_lines(&path, &rows);
    let failed = failures(&check(&f));
    assert!(
        has(&failed, "no `supervised-forger` window for node2"),
        "{failed:#?}"
    );
    assert!(
        has(&failed, "no `outage-three` window for node3"),
        "{failed:#?}"
    );
}

#[test]
fn a_relay_that_never_counted_the_delegates_fails() {
    let f = fixture();
    quiet_series(&f.run);
    // Node 2's relay never listed a count of 43 after height 10.
    let path = f.run.join("nodes/node2/difftest/network-state.jsonl");
    let rows: Vec<Value> = read_lines(&path)
        .into_iter()
        .map(|mut r| {
            r["hasQuorum"] = json!(30);
            r
        })
        .collect();
    write_lines(&path, &rows);
    let failed = failures(&check(&f));
    assert!(
        has(
            &failed,
            "node2: no answer after height 10 counts the 43 delegates"
        ),
        "{failed:#?}"
    );
}

#[test]
fn a_dry_run_of_the_recipe_is_no_evidence() {
    let f = fixture();
    quiet_series(&f.run);
    edit_json(&f.run.join("run.json"), |m| m["dryRun"] = json!(true));
    let report = check(&f);
    assert!(!report.is_success(), "{report}");
}
