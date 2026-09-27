//! `difftest live check` on recorded fixtures: a run directory with digest files (a torn last line
//! among them), store anchors, step reports, API answers and relay logs for the ban scan with its
//! fault windows.
//!
//! The run's records (`tests/fixtures/live/h/`: its steps, fault windows, bans, tips, block arrivals,
//! the rate limit's answers and txgen's summary) are recordings of a dry run of the devnet recipe,
//! with the devnet's addresses replaced by documentation addresses. Its chain is the committed
//! corpus, which the judge has judged, so no oracle runs: the fixture's node 4 is a Heartwood relay
//! whose digest lines are the judge's (in two starts, the first ending in a torn line) and whose
//! store is the corpus replayed through Heartwood's store. The dry runs recorded no ban at all, so
//! the tests add ban lines in the scan's format, inside and outside the recorded fault windows.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use heartwood_difftest::Error;
use heartwood_difftest::cli::{self, Command};
use heartwood_difftest::corpus::{self, Corpus};
use heartwood_difftest::live::{self, LiveOptions};
use heartwood_difftest::report::{Outcome, Report};
use heartwood_difftest::store::{self, ScratchDir};
use serde_json::{Value, json};

#[test]
fn a_live_check_parses() {
    let args: Vec<String> = ["live", "check", "fixtures/run"]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
    assert_eq!(
        cli::parse(&args).unwrap(),
        Command::LiveCheck {
            dir: "fixtures/run".into()
        }
    );
}

fn manifest_dir() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
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

/// The committed corpus's store, replayed once for every test.
fn replayed_store() -> &'static Path {
    static STORE: OnceLock<(ScratchDir, PathBuf)> = OnceLock::new();
    let (_, data) = STORE.get_or_init(|| {
        let scratch = ScratchDir::new("live-fixture-store").unwrap();
        let data = scratch.path().join("data");
        let loaded = Corpus::load(&manifest_dir().join("corpus/txgen-one-node")).unwrap();
        store::replay(&loaded, &data, None)
            .expect("the committed corpus replays through the store");
        (scratch, data)
    });
    data
}

/// A run directory `results/h` and a corpus root holding the judged chain `live-h`.
struct Fixture {
    _scratch: ScratchDir,
    run: PathBuf,
    root: PathBuf,
    /// The judge's forward lines of the chain, by height.
    forward: Vec<Value>,
}

/// The height at which the fixture's relay was killed and started again.
const RESTART_AT: usize = 120;

fn fixture() -> Fixture {
    let scratch = ScratchDir::new("live-fixture").unwrap();
    let run = scratch.path().join("results/h");
    let root = scratch.path().join("corpus");
    copy_dir(&manifest_dir().join("tests/fixtures/live/h"), &run);
    let committed = manifest_dir().join("corpus/txgen-one-node");
    copy_dir(&committed, &root.join("live-h"));
    let loaded = Corpus::load(&root.join("live-h")).unwrap();
    // The archive node 1's chain was imported from: only its manifest's end is read, to see that
    // the corpus under the root is that chain.
    let archive = run.join("archives/node1/20260927T120000Z");
    std::fs::create_dir_all(&archive).unwrap();
    let epoch = loaded.meta.pointer("/chain/epoch").cloned().unwrap();
    std::fs::write(
        archive.join("manifest.json"),
        json!({"format": corpus::ARCHIVE_FORMAT, "chain": {"epoch": epoch},
               "range": {"from": 1, "to": loaded.tip(), "lastId": loaded.block_id(loaded.tip()).unwrap().to_hex()}})
        .to_string(),
    )
    .unwrap();
    let forward: Vec<Value> = store::forward_lines(&loaded)
        .unwrap()
        .into_values()
        .collect();
    // Node 4's digest files: the judge's lines up to the restart, a torn line, then a restart line
    // and the rest of the chain.
    let difftest = run.join("nodes/node4/difftest");
    std::fs::create_dir_all(&difftest).unwrap();
    let text = |lines: &[Value]| lines.iter().map(|l| format!("{l}\n")).collect::<String>();
    std::fs::write(
        difftest.join("digest-0001.jsonl"),
        format!(
            "{}{{\"h\":{},\"step\":\"app",
            text(&forward[..RESTART_AT]),
            RESTART_AT + 1
        ),
    )
    .unwrap();
    let mut restart = forward[RESTART_AT - 1].clone();
    restart["step"] = json!("restart");
    restart.as_object_mut().unwrap().remove("rw");
    restart["m"] = json!("0000");
    std::fs::write(
        difftest.join("digest-0002.jsonl"),
        format!("{restart}\n{}", text(&forward[RESTART_AT..])),
    )
    .unwrap();
    copy_dir(replayed_store(), &run.join("nodes/node4/data"));
    let log = run.join("nodes/node4/log");
    std::fs::create_dir_all(&log).unwrap();
    std::fs::write(
        log.join("solar-testnet-relay-current.log"),
        "[2026-09-27 12:20:01.000] INFO: Blockchain 100% in sync\n\
         [2026-09-27 12:21:07.000] DEBUG: Protocol violation by peer 203.0.113.9: ReplySchema :warning:\n",
    )
    .unwrap();
    Fixture {
        _scratch: scratch,
        run,
        root,
        forward,
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

#[test]
fn a_recorded_run_passes() {
    let f = fixture();
    let report = check(&f);
    assert!(report.is_success(), "{report}\n{:#?}", failures(&report));
    let all = labels(&report);
    let has = |needle: &str| all.iter().any(|l| l.contains(needle));
    assert!(
        has("node4: a torn last line of start 1 dropped"),
        "{all:#?}"
    );
    assert!(
        has("digest lines in 2 starts equal the judge's"),
        "{all:#?}"
    );
    assert!(has("1 restart lines"), "{all:#?}");
    assert!(
        has("node4: the stored tip is the final tip 265"),
        "{all:#?}"
    );
    assert!(has("store: round rows of rounds 1 to 6"), "{all:#?}");
    assert!(
        has("no reply refusal and no violation against a reference peer"),
        "{all:#?}"
    );
    assert!(has("the judge's anchors hold (judged before"), "{all:#?}");
    assert!(has("one tip at the pause"), "{all:#?}");
    assert!(has("node4: no ban of"), "{all:#?}");
    assert!(has("every block within 8 s"), "{all:#?}");
    // Without the run `r` beside it, the pair comparison is skipped.
    assert!(
        report
            .results
            .iter()
            .any(|r| matches!(&r.outcome, Outcome::Skipped(m) if m.contains("no run r beside"))),
        "{all:#?}"
    );
    assert!(f.run.join("live-check.txt").exists());
    assert!(f.run.join("corpus/live-h/MANIFEST.sha256").exists());
}

#[test]
fn a_changed_line_a_refusal_ban_and_a_violation_fail() {
    let f = fixture();
    // A digest line whose wallets differ from the judge's.
    let path = f.run.join("nodes/node4/difftest/digest-0002.jsonl");
    let text = std::fs::read_to_string(&path).unwrap();
    let mut lines: Vec<Value> = text
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    lines[10]["w"] = json!("00");
    std::fs::write(
        &path,
        lines.iter().map(|l| format!("{l}\n")).collect::<String>(),
    )
    .unwrap();
    // A ban of node 4 for a refused request, and one for a closed socket outside every window.
    let bans = f.run.join("bans.jsonl");
    let mut text = std::fs::read_to_string(&bans).unwrap();
    text.push_str(
        "{\"node\":\"node2\",\"ts\":\"2026-09-27T12:30:00.000Z\",\"ip\":\"198.51.100.14\",\"duration\":\"10 seconds\",\"reason\":\"Validation failed (invalid version)\",\"line\":\"x\"}\n\
         {\"node\":\"node3\",\"ts\":\"2026-09-27T12:31:00.000Z\",\"ip\":\"198.51.100.14\",\"duration\":\"10 seconds\",\"reason\":\"Did not make an endpoint request\",\"line\":\"x\"}\n",
    );
    std::fs::write(&bans, text).unwrap();
    // A violation recorded against a reference node.
    let log = f
        .run
        .join("nodes/node4/log/solar-testnet-relay-current.log");
    let mut text = std::fs::read_to_string(&log).unwrap();
    text.push_str("[2026-09-27 12:32:00.000] DEBUG: Protocol violation by peer 198.51.100.11: Protocol :warning:\n");
    std::fs::write(&log, text).unwrap();
    let report = check(&f);
    let failed = failures(&report);
    let has = |needle: &str| failed.iter().any(|l| l.contains(needle));
    assert!(
        has(&format!(
            "the apply line of height {} differs from live-h's in w",
            RESTART_AT + 10
        )),
        "{failed:#?}"
    );
    assert!(
        has("no request-refusal or nes-termination ban"),
        "{failed:#?}"
    );
    assert!(
        has("no socket-close ban outside a fault window"),
        "{failed:#?}"
    );
    assert!(has("protocol violation of 198.51.100.11"), "{failed:#?}");
    assert_eq!(failed.len(), 4, "{failed:#?}");
    let _ = &f.forward;
}

#[test]
fn a_socket_close_ban_inside_a_fault_window_is_recorded() {
    let f = fixture();
    // The fixture's restart of node 4 (its window) is in faults.jsonl; a socket-close ban inside it.
    let faults = std::fs::read_to_string(f.run.join("faults.jsonl")).unwrap();
    let restart: Value = faults
        .lines()
        .map(|l| serde_json::from_str::<Value>(l).unwrap())
        .find(|v| v["fault"] == "restart")
        .expect("the fixture restarts node 4");
    let bans = f.run.join("bans.jsonl");
    let mut text = std::fs::read_to_string(&bans).unwrap();
    text.push_str(&format!(
        "{}\n",
        json!({"node": "node1", "ts": restart["start"], "ip": "198.51.100.14", "duration": "10 seconds",
               "reason": "Failed to complete websocket handshake", "line": "x"})
    ));
    std::fs::write(&bans, text).unwrap();
    let report = check(&f);
    assert!(report.is_success(), "{:#?}", failures(&report));
    assert!(
        labels(&report).iter().any(|l| l.contains(
            "1 bans inside fault windows: Failed to complete websocket handshake (restart)"
        )),
        "{:#?}",
        labels(&report)
    );
}

#[test]
fn a_node_that_stopped_short_of_the_tip_fails() {
    let f = fixture();
    let path = f.run.join("nodes/node4/difftest/digest-0002.jsonl");
    let text = std::fs::read_to_string(&path).unwrap();
    let kept: String = text.lines().take(50).map(|l| format!("{l}\n")).collect();
    std::fs::write(&path, kept).unwrap();
    let report = check(&f);
    let failed = failures(&report);
    assert!(
        failed
            .iter()
            .any(|l| l.contains("not at the final tip 265")),
        "{failed:#?}"
    );
    assert!(
        failed
            .iter()
            .any(|l| l.contains("heights without an apply line of the final chain's block")),
        "{failed:#?}"
    );
}
