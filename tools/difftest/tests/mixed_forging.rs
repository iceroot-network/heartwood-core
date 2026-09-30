//! `difftest live check` on a recorded run of the mixed forging network: reference nodes and
//! Heartwood nodes forging one chain (run `mixed-short`: nodes 1 to 3 reference, nodes 4 and 5
//! Heartwood, two rounds).
//!
//! The run's records (`tests/fixtures/live/mixed-short/`: its steps, fault window, tips, block
//! arrivals, both Heartwood relays' digest, block, await, attestation and network-state files, the
//! votes' and batches' wallets and txgen's summary) are a recording of a dry run of the devnet
//! recipe, with the devnet's addresses replaced by documentation addresses. Its chain is the
//! judged corpus in `corpus/`, so no oracle runs. The route corpus's answers are left out. The relays' stores are the corpus replayed
//! through Heartwood's store, as in `live.rs`. The dry run recorded no ban, so the tests add ban
//! lines in the scan's format, inside and outside the recorded fault window, and rewrite the run
//! into a split run (`mixed-split`) with a minority to see how its archive is handled.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use heartwood_difftest::Error;
use heartwood_difftest::corpus::{self, Corpus};
use heartwood_difftest::live::{self, LiveOptions};
use heartwood_difftest::report::{Outcome, Report};
use heartwood_difftest::store::{self, ScratchDir};
use heartwood_difftest::vectors::sha256_hex;
use serde_json::{Value, json};

fn manifest_dir() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn recording() -> PathBuf {
    manifest_dir().join("tests/fixtures/live/mixed-short")
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
        let scratch = ScratchDir::new("mixed-forging-store").unwrap();
        let data = scratch.path().join("data");
        let loaded = Corpus::load(&recording().join("corpus")).unwrap();
        store::replay(&loaded, &data, None).expect("the recorded chain replays through the store");
        (scratch, data)
    });
    data
}

const BINARY: &[u8] = b"the release binary";
const CORPUS: &str = "live-mixed-short";

/// A run directory, a corpus root holding the judged chain and the tree's release binary (a
/// stand-in file whose SHA-256 the run records).
struct Fixture {
    _scratch: ScratchDir,
    run: PathBuf,
    root: PathBuf,
    relay: PathBuf,
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

fn fixture() -> Fixture {
    let scratch = ScratchDir::new("mixed-forging-fixture").unwrap();
    let run = scratch.path().join("results/mixed-short/2");
    let root = scratch.path().join("corpus");
    copy_dir(&recording(), &run);
    std::fs::remove_dir_all(run.join("corpus")).unwrap();
    copy_dir(&recording().join("corpus"), &root.join(CORPUS));
    let relay = scratch.path().join("heartwood");
    std::fs::write(&relay, BINARY).unwrap();
    let mut manifest = read_json(&run.join("run.json"));
    manifest["relayImage"] = json!("heartwood-relay:fixture");
    manifest["relayBinarySha256"] = json!(sha256_hex(BINARY));
    // The route corpus's answers are tens of megabytes: the api tests judge them, not this one.
    manifest.as_object_mut().unwrap().remove("api");
    copy_dir(
        &recording().join("corpus/crypto"),
        &run.join("network/crypto"),
    );
    std::fs::write(run.join("run.json"), manifest.to_string()).unwrap();
    // The archive node 1's chain was imported from: only its manifest's end is read, to see that
    // the corpus under the root is that chain.
    let loaded = Corpus::load(&root.join(CORPUS)).unwrap();
    let archive = run.join("archives/node1/20260930T163435Z");
    std::fs::create_dir_all(&archive).unwrap();
    std::fs::write(
        archive.join("manifest.json"),
        archive_manifest(&loaded, loaded.tip()),
    )
    .unwrap();
    for node in ["node4", "node5"] {
        copy_dir(replayed_store(), &run.join("nodes").join(node).join("data"));
        let log = run.join("nodes").join(node).join("log");
        std::fs::create_dir_all(&log).unwrap();
        std::fs::write(
            log.join("solar-testnet-relay-current.log"),
            "[2026-09-30 15:59:01.000] INFO: Blockchain 100% in sync\n",
        )
        .unwrap();
    }
    Fixture {
        _scratch: scratch,
        run,
        root,
        relay,
    }
}

/// An archive manifest that ends at `to` of the chain `loaded`.
fn archive_manifest(loaded: &Corpus, to: u32) -> String {
    let epoch = loaded.meta.pointer("/chain/epoch").cloned().unwrap();
    json!({"format": corpus::ARCHIVE_FORMAT, "chain": {"epoch": epoch},
           "range": {"from": 1, "to": to, "lastId": loaded.block_id(to).unwrap().to_hex()}})
    .to_string()
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

/// The recorded fault window of the pause, and a time inside it.
fn pause_window(run: &Path) -> (String, String) {
    let faults = read_lines(&run.join("faults.jsonl"));
    let pause = faults
        .iter()
        .find(|v| v["fault"] == "pause")
        .expect("the run paused the network");
    (
        pause["start"].as_str().unwrap().to_owned(),
        pause["recovered"].as_str().unwrap().to_owned(),
    )
}

fn add_ban(run: &Path, node: &str, ts: &str, ip: &str, reason: &str) {
    let path = run.join("bans.jsonl");
    let mut text = std::fs::read_to_string(&path).unwrap();
    text.push_str(&format!(
        "{}\n",
        json!({"node": node, "ts": ts, "ip": ip, "duration": "10 seconds", "reason": reason, "line": "x"})
    ));
    std::fs::write(&path, text).unwrap();
}

const CLOSED: &str = "Did not make an endpoint request";

#[test]
fn a_recorded_run_passes_every_section() {
    let f = fixture();
    let report = check(&f);
    assert!(report.is_success(), "{report}\n{:#?}", failures(&report));
    let all = labels(&report);
    for needle in [
        "every forger produced in every round outside its fault windows",
        "every Heartwood-hosted block had a quorum answer in its slot",
        "every Heartwood relay counted all 53 delegates before the partition",
        "the votes were admitted, and none was placed for a restarted relay's own delegates",
        "every reference relay lists a version for every Heartwood-hosted delegate",
        "the partition's sides forged as their delegates' reach allows",
        "one tip at the end",
        "every step in its planned round and order",
        "node4: 384 offered",
        "node5: 384 offered",
        "node4: no ban of 198.51.100.14 but inside a fault window",
        "node5: no ban of 198.51.100.15 but inside a fault window",
    ] {
        assert!(has(&all, needle), "{needle}\n{all:#?}");
    }
}

#[test]
fn a_reference_ban_of_a_heartwood_address_outside_a_window_fails() {
    let f = fixture();
    // The window of the recorded pause opens at height 60; a ban before it is outside every window.
    add_ban(
        &f.run,
        "node2",
        "2026-09-30T15:55:00.000Z",
        "198.51.100.14",
        CLOSED,
    );
    let failed = failures(&check(&f));
    assert!(
        has(
            &failed,
            "node4: no socket-close ban outside a fault window: node2 banned it at 2026-09-30T15:55:00.000Z"
        ),
        "{failed:#?}"
    );
    assert_eq!(failed.len(), 1, "{failed:#?}");
}

#[test]
fn the_same_ban_inside_a_fault_window_is_recorded() {
    let f = fixture();
    let (start, end) = pause_window(&f.run);
    assert!(start < end);
    add_ban(&f.run, "node2", &start, "198.51.100.14", CLOSED);
    let report = check(&f);
    assert!(report.is_success(), "{:#?}", failures(&report));
    assert!(
        has(
            &labels(&report),
            "1 bans inside fault windows: Did not make an endpoint request (pause)"
        ),
        "{:#?}",
        labels(&report)
    );
}

#[test]
fn a_ban_for_another_reason_fails_inside_a_window_too() {
    let f = fixture();
    let (start, _) = pause_window(&f.run);
    add_ban(
        &f.run,
        "node2",
        &start,
        "198.51.100.15",
        "Validation failed (invalid version)",
    );
    let failed = failures(&check(&f));
    assert!(
        has(&failed, "no request-refusal or nes-termination ban"),
        "{failed:#?}"
    );
}

/// The recorded run as a split run: the recording's chain stands in for the majority's, and node 5
/// is the minority, whose archive from before the heal is the same chain (its manifest ends where
/// the majority's does).
fn split(f: &Fixture) {
    let mut manifest = read_json(&f.run.join("run.json"));
    manifest["run"] = json!("mixed-split");
    manifest["corpora"] = json!({
        "live-mixed-split": "archives/node1",
        "live-mixed-split-minority": "archives/node5-preheal",
    });
    manifest["minority"] = json!({"nodes": ["node5"], "corpus": "live-mixed-split-minority"});
    std::fs::write(f.run.join("run.json"), manifest.to_string()).unwrap();
    let loaded = Corpus::load(&f.root.join(CORPUS)).unwrap();
    for name in ["live-mixed-split", "live-mixed-split-minority"] {
        copy_dir(&f.root.join(CORPUS), &f.root.join(name));
    }
    std::fs::remove_dir_all(f.root.join(CORPUS)).unwrap();
    let archive = f.run.join("archives/node5-preheal/20260930T170000Z");
    std::fs::create_dir_all(&archive).unwrap();
    std::fs::write(
        archive.join("manifest.json"),
        archive_manifest(&loaded, loaded.tip()),
    )
    .unwrap();
}

#[test]
fn a_minority_archive_is_judged_as_a_chain_of_its_own() {
    let f = fixture();
    split(&f);
    let report = check(&f);
    let all = labels(&report);
    let failed = failures(&report);
    // The minority's archive is a chain of its own beside the run's: it is judged, and the check
    // that both sides forged names it, here failing because the recorded chain has no fork.
    assert!(has(&all, "live-mixed-split-minority"), "{all:#?}");
    assert!(
        has(
            &failed,
            "live-mixed-split-minority holds no block of its own"
        ),
        "{failed:#?}"
    );
}

/// Rewrite the fixture's `run.json` with `change`.
fn edit_manifest(f: &Fixture, change: impl FnOnce(&mut Value)) {
    let mut manifest = read_json(&f.run.join("run.json"));
    change(&mut manifest);
    std::fs::write(f.run.join("run.json"), manifest.to_string()).unwrap();
}

#[test]
fn a_mixed_run_without_a_heartwood_node_is_no_evidence() {
    let f = fixture();
    edit_manifest(&f, |m| {
        for node in m["nodes"].as_array_mut().unwrap() {
            node["implementation"] = json!("reference");
            node["underTest"] = json!(false);
        }
    });
    let failed = failures(&check(&f));
    assert!(
        has(&failed, "a mixed forging run with Heartwood nodes"),
        "{failed:#?}"
    );
    assert!(
        has(
            &failed,
            "the attestation record's waits and unsigned statuses"
        ),
        "{failed:#?}"
    );
}

#[test]
fn a_dry_run_of_the_recipe_is_no_evidence() {
    let f = fixture();
    edit_manifest(&f, |m| m["dryRun"] = json!(true));
    let failed = failures(&check(&f));
    assert!(
        has(&failed, "a mixed forging run with Heartwood nodes"),
        "{failed:#?}"
    );
}
