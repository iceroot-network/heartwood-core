//! `difftest live check` on recorded fixtures: a run directory with digest files (a torn last line
//! among them), store anchors, step reports, API answers and relay logs for the ban scan with its
//! fault windows.
//!
//! The run's records (`tests/fixtures/live/h/`: its steps, fault windows, bans, tips, block arrivals,
//! the rate limit's answers and txgen's summary) are recordings of a dry run of the devnet recipe,
//! with the devnet's addresses replaced by documentation addresses, and the step of the pool
//! transactions the recipe gained later. Its chain is the committed
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
use heartwood_difftest::synthetic::DEVNET_SEED;
use heartwood_difftest::vectors::sha256_hex;
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
            dir: "fixtures/run".into(),
            compare: None,
            spread: vec![],
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

/// A run directory `results/h`, a corpus root holding the judged chain `live-h`, and the release
/// binary of the tree (a stand-in file whose SHA-256 the run records).
struct Fixture {
    _scratch: ScratchDir,
    run: PathBuf,
    root: PathBuf,
    relay: PathBuf,
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
    // The relay the run tested: the tree's release binary, as run.json records it.
    let relay = scratch.path().join("heartwood");
    std::fs::write(&relay, b"the release binary").unwrap();
    let mut manifest: Value =
        serde_json::from_str(&std::fs::read_to_string(run.join("run.json")).unwrap()).unwrap();
    manifest["relayImage"] = json!("heartwood-relay:fixture");
    manifest["relayBinarySha256"] = json!(sha256_hex(b"the release binary"));
    manifest["worktreeCommit"] = json!("0123456789abcdef0123456789abcdef01234567");
    manifest["worktreeClean"] = json!(true);
    std::fs::write(run.join("run.json"), manifest.to_string()).unwrap();
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
        relay,
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
    assert!(has("is this tree's release binary"), "{all:#?}");
    assert!(
        has("node4: 2 starts: the first and one after each of its 1 scripted restarts"),
        "{all:#?}"
    );
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
fn a_run_of_another_relay_binary_fails() {
    let f = fixture();
    // The tree was built again after the run: its release binary is not the one the run tested.
    std::fs::write(&f.relay, b"a later build").unwrap();
    let report = check(&f);
    let failed = failures(&report);
    assert!(
        failed
            .iter()
            .any(|l| l.contains("the relay the run tested is this tree's")
                && l.contains("repeat the run")),
        "{failed:#?}"
    );
    // A run that does not record its relay binary cannot count either.
    let path = f.run.join("run.json");
    let mut manifest: Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    manifest
        .as_object_mut()
        .unwrap()
        .remove("relayBinarySha256");
    std::fs::write(&path, manifest.to_string()).unwrap();
    let failed = failures(&check(&f));
    assert!(
        failed
            .iter()
            .any(|l| l.contains("records no relayBinarySha256")),
        "{failed:#?}"
    );
}

#[test]
fn a_start_nobody_scripted_fails() {
    let f = fixture();
    // The relay stopped by itself once more: a third start, which no fault of the run explains.
    let difftest = f.run.join("nodes/node4/difftest");
    let second = std::fs::read_to_string(difftest.join("digest-0002.jsonl")).unwrap();
    let first_line = second.lines().next().unwrap();
    std::fs::write(
        difftest.join("digest-0003.jsonl"),
        format!("{first_line}\n"),
    )
    .unwrap();
    let report = check(&f);
    let failed = failures(&report);
    assert!(
        failed
            .iter()
            .any(|l| l.contains("3 starts (digest files), but its scripted restarts make 2")),
        "{failed:#?}"
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

fn write_lines(path: &Path, rows: &[Value]) {
    std::fs::write(
        path,
        rows.iter().map(|v| format!("{v}\n")).collect::<String>(),
    )
    .unwrap();
}

/// The node of the devnet that holds the delegate `username` (`genesis_N`): 11, 11, 11, 10 and 10.
fn owner_of(username: &str) -> usize {
    let n: usize = username.trim_start_matches("genesis_").parse().unwrap();
    match n {
        1..=11 => 1,
        12..=22 => 2,
        23..=33 => 3,
        34..=43 => 4,
        _ => 5,
    }
}

/// A run `m` of five Heartwood validators whose chain is the committed corpus: every relay's block
/// file and digest lines are the corpus's, and every relay answered its forger, in the slot of
/// each of its delegates' blocks, at the height below it with all 53 delegates counted.
struct Devnet {
    _scratch: ScratchDir,
    run: PathBuf,
    manifest: live::run::Manifest,
    corpus: Corpus,
}

fn devnet() -> Devnet {
    let scratch = ScratchDir::new("live-devnet").unwrap();
    let run = scratch.path().join("m");
    let source = manifest_dir().join("corpus/txgen-one-node");
    let corpus = Corpus::load(&source).unwrap();
    let meta = live::run::read_lines(&source.join("reference/meta.jsonl")).unwrap();
    let names: std::collections::BTreeMap<u64, String> = meta
        .iter()
        .map(|m| {
            (
                m["h"].as_u64().unwrap(),
                m["username"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    let mut keys: Vec<std::collections::BTreeMap<String, String>> = vec![Default::default(); 5];
    for b in &corpus.blocks {
        let block = heartwood_crypto::Block::decode(&b.bytes, &corpus.milestones).unwrap();
        let name = &names[&u64::from(b.height)];
        keys[owner_of(name) - 1].insert(block.header().generator_public_key.to_hex(), name.clone());
    }
    let ip = |n: usize| format!("198.51.100.{}", 10 + n);
    let nodes: Vec<Value> = (1..=5)
        .map(|n| {
            let delegates: Vec<Value> = keys[n - 1]
                .iter()
                .map(|(k, u)| json!({"username": u, "publicKey": k}))
                .collect();
            json!({"name": format!("node{n}"), "ip": ip(n), "role": "forger", "implementation": "heartwood",
                   "underTest": true, "forgerAddress": ip(n), "delegates": delegates})
        })
        .collect();
    std::fs::create_dir_all(&run).unwrap();
    std::fs::write(
        run.join("run.json"),
        json!({"format": live::run::FORMAT, "run": "m", "network": "devnet", "seed": DEVNET_SEED,
               "roundLength": 53, "blockTime": 8, "stopHeight": 265, "control": "node1",
               "nodes": nodes, "corpora": {"live-m": "nodes/node1"}, "status": "complete"})
        .to_string(),
    )
    .unwrap();
    let forward: Vec<Value> = store::forward_lines(&corpus)
        .unwrap()
        .into_values()
        .collect();
    let epoch =
        heartwood_crypto::crypto::slots::epoch_unix_ms(corpus.milestones.at(1).epoch()).unwrap();
    for n in 1..=5 {
        let dir = run.join(format!("nodes/node{n}"));
        std::fs::create_dir_all(dir.join("difftest")).unwrap();
        copy_dir(&source.join("crypto"), &dir.join("crypto"));
        std::fs::copy(
            source.join("blocks.jsonl"),
            dir.join("difftest/blocks-0001.bin"),
        )
        .unwrap();
        write_lines(&dir.join("difftest/digest-0001.jsonl"), &forward);
        let mut answers = Vec::new();
        for b in &corpus.blocks {
            let block = heartwood_crypto::Block::decode(&b.bytes, &corpus.milestones).unwrap();
            if owner_of(&names[&u64::from(b.height)]) != n {
                continue;
            }
            let at = epoch + i64::from(block.header().timestamp) * 1000 + 300;
            answers.push(
                json!({"t": live::run::format_time(at), "caller": ip(n), "status": 0,
                "nodeHeight": b.height - 1, "hasQuorum": 53, "noQuorum": 0,
                "delegates": {"hasQuorum": [], "noQuorum": [], "noResponse": []}}),
            );
        }
        write_lines(&dir.join("difftest/network-state.jsonl"), &answers);
    }
    let manifest = live::run::Manifest::load(&run).unwrap();
    Devnet {
        _scratch: scratch,
        run,
        manifest,
        corpus,
    }
}

fn slots(d: &Devnet) -> live::forging::Slots {
    live::forging::Slots {
        epoch_ms: heartwood_crypto::crypto::slots::epoch_unix_ms(d.corpus.milestones.at(1).epoch())
            .unwrap(),
        slot_ms: 8_000,
    }
}

#[test]
fn every_block_needs_a_quorum_answer_to_its_forger_in_its_slot() {
    use live::forging::{answers, full_counts, quorum_before_blocks, recorded_blocks, validators};
    let d = devnet();
    let v = validators(&d.run).unwrap();
    assert_eq!(v.iter().map(|v| v.keys.len()).sum::<usize>(), 53);
    let (blocks, problems) = recorded_blocks(&d.run, &d.manifest, &d.corpus.milestones).unwrap();
    assert!(problems.is_empty(), "{problems:#?}");
    assert_eq!(blocks.len(), 264);
    let (all, problems) = answers(&d.run, &d.manifest).unwrap();
    assert!(problems.is_empty(), "{problems:#?}");
    assert!(quorum_before_blocks(&blocks, &all, &v, slots(&d)).is_empty());
    assert!(full_counts(&all, &v, i64::MAX, None, 53).is_empty());
    // An answer below the quorum, to another caller, at another height or in another slot is no
    // evidence for its block.
    let first = all
        .iter()
        .find(|a| a.node_height == Some(100))
        .unwrap()
        .clone();
    for changed in [
        live::forging::Answer {
            has_quorum: 26,
            ..first.clone()
        },
        live::forging::Answer {
            caller: "198.51.100.99".into(),
            ..first.clone()
        },
        live::forging::Answer {
            node_height: Some(99),
            ..first.clone()
        },
        live::forging::Answer {
            at: first.at + 8_000,
            ..first.clone()
        },
    ] {
        let others: Vec<_> = all
            .iter()
            .filter(|a| **a != first)
            .cloned()
            .chain([changed])
            .collect();
        let problems = quorum_before_blocks(&blocks, &others, &v, slots(&d));
        assert_eq!(problems.len(), 1, "{problems:#?}");
        assert!(problems[0].contains("at height 101"), "{problems:#?}");
    }
    // A relay whose answers never counted all 53 delegates, or that never answered after height 10.
    let short: Vec<_> = all
        .iter()
        .map(|a| live::forging::Answer {
            no_response: Some(usize::from(a.relay == "node3")),
            ..a.clone()
        })
        .collect();
    let problems = full_counts(&short, &v, i64::MAX, None, 53);
    assert_eq!(
        problems,
        ["node3: no answer after height 10 counts all 53 delegates"]
    );
    let problems = full_counts(&all, &v, i64::MAX, Some(10), 53);
    assert_eq!(problems.len(), 5, "{problems:#?}");
}

#[test]
fn the_attestation_report_gives_the_latency_and_the_answers_without_a_quorum() {
    use live::forging::{Answer, answers, attestation_report, recorded_blocks, validators};
    let d = devnet();
    let v = validators(&d.run).unwrap();
    let (blocks, _) = recorded_blocks(&d.run, &d.manifest, &d.corpus.milestones).unwrap();
    let (all, _) = answers(&d.run, &d.manifest).unwrap();
    // The fixture answers each block's forger 300 ms into the slot.
    let report = attestation_report(&blocks, &all, &v, slots(&d));
    assert!(
        report.contains("300 / 300 / 300 ms into the slot")
            && report.contains("over 264 blocks, 0 without one"),
        "{report}"
    );
    assert!(report.contains("0 of "), "{report}");
    // An answer below the quorum is counted, and its block has no quorum answer.
    let first = all
        .iter()
        .find(|a| a.node_height == Some(100))
        .unwrap()
        .clone();
    let others: Vec<_> = all
        .iter()
        .filter(|a| **a != first)
        .cloned()
        .chain([Answer {
            has_quorum: 26,
            ..first.clone()
        }])
        .collect();
    let report = attestation_report(&blocks, &others, &v, slots(&d));
    assert!(
        report.contains("over 263 blocks, 1 without one"),
        "{report}"
    );
    assert!(report.contains("1 of "), "{report}");
}

#[test]
fn a_step_starts_no_lower_than_the_steps_before_it() {
    use live::forging::start_heights;
    use live::run::Step;
    let step = |name: &str, height: u64| Step {
        name: name.into(),
        node: "all".into(),
        ok: true,
        start: None,
        record: json!({"step": name, "startHeight": height}),
    };
    // The runner read 53 when the failover began, just after its wait for 54 had returned.
    let steps = [
        step("up", 0),
        step("processes-round2", 54),
        step("failover-second", 53),
        step("failover-third", 62),
    ];
    assert_eq!(
        start_heights(&steps),
        [
            ("up", 0),
            ("processes-round2", 54),
            ("failover-second", 54),
            ("failover-third", 62)
        ]
    );
}

#[test]
fn a_failover_needs_the_relays_own_receipt_of_the_block_in_time() {
    use live::forging::{Receipt, failover_received};
    let receipt = |relay: &str, from: &str, id: &str, at: i64| Receipt {
        relay: relay.into(),
        at,
        username: "genesis_5".into(),
        height: 60,
        id: id.into(),
        from: from.into(),
    };
    let receipts = [receipt("node6", "127.2.0.111", "aa", 100_000)];
    let ok = |relay, from, id, start| failover_received(&receipts, relay, from, id, start, 8_000);
    assert!(ok("node6", "127.2.0.111", "aa", Some(90_000)));
    assert!(
        !ok("node7", "127.2.0.111", "aa", Some(90_000)),
        "another relay"
    );
    assert!(
        !ok("node6", "127.2.0.112", "aa", Some(90_000)),
        "another source"
    );
    assert!(
        !ok("node6", "127.2.0.111", "bb", Some(90_000)),
        "another block"
    );
    assert!(
        !ok("node6", "127.2.0.111", "aa", Some(100_001)),
        "before the step"
    );
    assert!(
        !ok("node6", "127.2.0.111", "aa", Some(-100_001)),
        "too long after it"
    );
    assert!(
        !ok("node6", "127.2.0.111", "aa", None),
        "a step without a start"
    );
}

#[test]
fn network_state_lines_are_read_as_the_relay_writes_them() {
    let d = devnet();
    let path = d.run.join("nodes/node2/difftest/network-state.jsonl");
    let mut text = std::fs::read_to_string(&path).unwrap();
    // A relay's answer with no height (its chain not loaded yet) and a line without its time.
    text.push_str("{\"t\":\"2026-09-26T17:30:00.000Z\",\"caller\":\"198.51.100.12\",\"status\":2,\"nodeHeight\":null,\"hasQuorum\":0,\"noQuorum\":0,\"delegates\":{\"hasQuorum\":[],\"noQuorum\":[],\"noResponse\":[\"genesis_1\"]}}\n");
    text.push_str("{\"caller\":\"198.51.100.12\",\"hasQuorum\":53}\n");
    std::fs::write(&path, text).unwrap();
    let (all, problems) = live::forging::answers(&d.run, &d.manifest).unwrap();
    assert_eq!(problems.len(), 1, "{problems:#?}");
    assert!(problems[0].contains("no time"), "{problems:#?}");
    let last = all.iter().rfind(|a| a.relay == "node2").unwrap();
    assert_eq!(
        (last.node_height, last.has_quorum, last.no_response),
        (None, 0, Some(1))
    );
    std::fs::remove_file(d.run.join("nodes/node4/difftest/network-state.jsonl")).unwrap();
    let (_, problems) = live::forging::answers(&d.run, &d.manifest).unwrap();
    assert!(
        problems
            .iter()
            .any(|p| p == "node4: no network-state record"),
        "{problems:#?}"
    );
}

#[test]
fn two_blocks_of_one_key_in_one_slot_fail_from_the_block_files_or_the_logs() {
    use heartwood_crypto::blocks::factory::{self, BlockDraft};
    use live::forging::{Receipt, receipts_of, recorded_blocks, signing_slots, validators};
    let d = devnet();
    let v = validators(&d.run).unwrap();
    let (blocks, _) = recorded_blocks(&d.run, &d.manifest, &d.corpus.milestones).unwrap();
    assert_eq!(signing_slots(&blocks, &[], &v, slots(&d)), (vec![], 0));
    // Another block of the same delegate for the same slot: an empty block rebuilt with another
    // reward, signed with the delegate's key the seed derives.
    let stored = d
        .corpus
        .blocks
        .iter()
        .find(|b| {
            b.height > 20
                && heartwood_crypto::Block::decode(&b.bytes, &d.corpus.milestones)
                    .unwrap()
                    .transactions()
                    .is_empty()
        })
        .unwrap();
    let block = heartwood_crypto::Block::decode(&stored.bytes, &d.corpus.milestones).unwrap();
    let h = block.header();
    let key = (1..=53)
        .map(|k| {
            heartwood_crypto::genesis::derive::role_key(
                heartwood_difftest::synthetic::DERIVATION_TAG,
                &format!("delegate/{k}"),
                DEVNET_SEED,
                d.corpus.network.pub_key_hash,
            )
            .unwrap()
            .keys
        })
        .find(|k| k.public_key().to_compressed() == h.generator_public_key)
        .unwrap();
    let draft = BlockDraft {
        version: h.version,
        timestamp: h.timestamp,
        height: h.height,
        previous_block: h.previous_block,
        number_of_transactions: 0,
        total_amount: 0,
        total_fee: 0,
        reward: h.reward + 1,
        payload_length: 0,
        payload_hash: h.payload_hash,
    };
    let other = factory::make(
        draft,
        Vec::new(),
        &key,
        heartwood_crypto::Aux::fixed([7; 32]),
        &d.corpus.milestones,
    )
    .unwrap();
    assert_ne!(other.id(), block.id());
    let line = json!({"h": other.height(), "id": other.id().to_hex(), "hex": heartwood_crypto::utils::hex::encode(&other.encode())});
    write_lines(&d.run.join("nodes/node3/difftest/blocks-0002.bin"), &[line]);
    let (with_other, problems) =
        recorded_blocks(&d.run, &d.manifest, &d.corpus.milestones).unwrap();
    assert!(problems.is_empty(), "{problems:#?}");
    let (problems, _) = signing_slots(&with_other, &[], &v, slots(&d));
    assert_eq!(problems.len(), 1, "{problems:#?}");
    assert!(
        problems[0].contains("2 different blocks in slot"),
        "{problems:#?}"
    );
    // The same, seen only in a relay's log: a block received from the delegate's forger in that
    // slot that no relay applied.
    let holder = v
        .iter()
        .find(|v| v.keys.contains_key(&h.generator_public_key.to_hex()))
        .unwrap();
    let username = &holder.keys[&h.generator_public_key.to_hex()];
    let at = slots(&d).epoch_ms + i64::from(h.timestamp) * 1000 + 1_500;
    let stamp = live::run::format_time(at)
        .replace('T', " ")
        .replace('Z', "");
    let log = format!(
        "[{stamp}] INFO: Received new block forged by delegate {username} (#7) at height {} with 2.0 reward :package:\n\
         [{stamp}] DEBUG: The id of the new block is {}\n\
         [{stamp}] DEBUG: It contains 0 transactions and was received from {}\n",
        h.height,
        other.id().to_hex(),
        holder.forger
    );
    let receipts = receipts_of(&holder.name, &log);
    assert_eq!(
        receipts,
        [Receipt {
            relay: holder.name.clone(),
            at: at / 1000 * 1000 + 500,
            username: username.clone(),
            height: u64::from(h.height),
            id: other.id().to_hex(),
            from: holder.forger.clone(),
        }]
    );
    let (problems, unapplied) = signing_slots(&blocks, &receipts, &v, slots(&d));
    assert_eq!((problems.len(), unapplied), (1, 1), "{problems:#?}");
    // A block a peer broadcast is not bound to a slot by its receipt.
    let broadcast = log.replace(&holder.forger, "198.51.100.99");
    let (problems, unapplied) = signing_slots(
        &blocks,
        &receipts_of(&holder.name, &broadcast),
        &v,
        slots(&d),
    );
    assert_eq!((problems.len(), unapplied), (0, 0), "{problems:#?}");
}

fn process_reports(manifest: &live::run::Manifest) -> Vec<Value> {
    [2, 5]
        .into_iter()
        .map(|round| {
            let processes: Vec<_> = manifest
                .nodes
                .iter()
                .enumerate()
                .flat_map(|(i, n)| {
                    ["relay", "forger"]
                        .into_iter()
                        .enumerate()
                        .map(move |(j, role)| {
                            let pid = 100 + 4 * i + 2 * j;
                            json!({"node": n.name, "role": role, "pid": pid, "parent": pid + 1,
                                   "supervisor": pid + 1,
                                   "log": format!("{}-{}-{role}-current.log", n.name, manifest.network),
                                   "logExists": true,
                                   "listeners": if role == "relay" { json!([{"ip": n.ip, "port": 6002}]) } else { json!([]) }})
                        })
                })
                .collect();
            json!({"round": round, "ts": "2026-09-27T12:00:00.000Z", "processes": processes})
        })
        .collect()
}

#[test]
fn processes_one_chain_and_status_polls() {
    let d = devnet();
    assert!(
        live::forging::one_chain(&d.run, &d.manifest)
            .unwrap()
            .is_empty()
    );
    let reports = process_reports(&d.manifest);
    assert!(live::forging::processes(&d.manifest, &reports).is_empty());
    let mut listening = reports.clone();
    listening[0]["processes"][1]["listeners"] = json!([{"ip": "0.0.0.0", "port": 6002}]);
    let mut wide = reports.clone();
    wide[1]["processes"][0]["listeners"] = json!([{"ip": "0.0.0.0", "port": 6002}]);
    let mut shared = reports.clone();
    shared[0]["processes"][3]["supervisor"] = shared[0]["processes"][1]["supervisor"].clone();
    let mut unnamed = reports.clone();
    unnamed[0]["processes"][0]["log"] = json!("relay.out.log");
    for (broken, needle) in [
        (listening, "forger has a listening socket"),
        (wide, "relay listeners are absent or not bound to"),
        (shared, "no distinct parent supervisor"),
        (unnamed, "no log file named node1-devnet-relay-"),
        (reports[..1].to_vec(), "no process report in round 5"),
    ] {
        let problems = live::forging::processes(&d.manifest, &broken);
        assert!(problems.iter().any(|p| p.contains(needle)), "{problems:#?}");
    }
    // A reapply line of another state on one relay: its applied chain is not the others'.
    let path = d.run.join("nodes/node5/difftest/digest-0001.jsonl");
    let mut lines = live::run::read_lines(&path).unwrap();
    let mut reapplied = lines[10].clone();
    reapplied["step"] = json!("reapply");
    reapplied["w"] = json!("different");
    lines.push(reapplied);
    write_lines(&path, &lines);
    let problems = live::forging::one_chain(&d.run, &d.manifest).unwrap();
    assert!(
        problems
            .iter()
            .any(|s| s.contains("node5: final applied chain diverges at height 11")),
        "{problems:#?}"
    );
    let tips = json!({"node1": {"height": 265, "id": "a"}, "node2": {"height": 265, "id": "a"}});
    let status = |ts, consistent| json!({"ts": ts, "consistent": consistent, "tips": tips});
    let mut records = vec![
        status("2026-09-27T12:00:00Z", true),
        status("2026-09-27T12:00:30Z", false),
        status("2026-09-27T12:01:00Z", true),
    ];
    let windows = vec![live::run::Window {
        fault: "restart".into(),
        nodes: vec!["node2".into()],
        start: live::run::parse_time("2026-09-27T12:00:20Z").unwrap(),
        end: live::run::parse_time("2026-09-27T12:00:50Z").unwrap(),
    }];
    assert!(live::forging::statuses(&records, &windows, 2).is_empty());
    assert!(!live::forging::statuses(&records, &[], 2).is_empty());
    records[2]["consistent"] = json!(false);
    assert!(!live::forging::statuses(&records, &windows, 2).is_empty());
    records[2] = status("2026-09-27T12:01:40Z", true);
    let problems = live::forging::statuses(&records, &windows, 2);
    assert!(
        problems
            .iter()
            .any(|p| p.contains("at most 35 seconds apart")),
        "{problems:#?}"
    );
}

#[test]
fn forging_comparisons_require_real_scenarios_and_keep_wallet_balances() {
    use live::steps::{compare_scenario_wallets, problems, required};
    for run in ["t", "m", "f"] {
        assert!(!problems(run, &[]).is_empty());
        assert!(required(run).contains(&"fresh-node5".to_owned()));
    }
    assert!(problems("x", &[]).iter().any(|p| p == "unknown live run x"));
    let a = json!({"wallets": [{"address": "a", "balance": "12", "nonce": "2",
        "attributes": {"delegate": {"rank": 1}, "voteBalance": "1", "vote": {"x": 100}}}]});
    let mut b = a.clone();
    b["wallets"][0]["attributes"]["delegate"]["rank"] = json!(2);
    b["wallets"][0]["attributes"]["voteBalance"] = json!("100");
    assert!(compare_scenario_wallets(&a, &b).is_empty());
    b["wallets"][0]["balance"] = json!("13");
    assert!(!compare_scenario_wallets(&a, &b).is_empty());
    let mut c = a.clone();
    c["wallets"][0]["attributes"]["vote"] = json!({"y": 100});
    assert!(!compare_scenario_wallets(&a, &c).is_empty());
    assert!(!compare_scenario_wallets(&json!({}), &json!({})).is_empty());
}

#[test]
#[ignore = "integration"]
fn recorded_block_files_import_with_restarts_and_refuse_gaps() {
    let scratch = ScratchDir::new("block-files").unwrap();
    let source = manifest_dir().join("corpus/txgen-one-node");
    let rows = live::run::read_lines(&source.join("blocks.jsonl")).unwrap();
    let first = scratch.path().join("blocks-0001.bin");
    let second = scratch.path().join("blocks-0002.bin");
    // The first start ends in a torn line; the second applies height 100 again (a switch to the
    // same branch) and goes on.
    write_lines(&first, &rows[..100]);
    use std::io::Write;
    std::fs::OpenOptions::new()
        .append(true)
        .open(&first)
        .unwrap()
        .write_all(b"{\"h\":")
        .unwrap();
    write_lines(&second, &rows[98..]);
    let out = scratch.path().join("imported");
    let import = |files: &[PathBuf], seed: &str, out: &Path| {
        heartwood_difftest::corpus_from_blocks::from_blocks(
            files,
            &source.join("crypto"),
            seed,
            out,
        )
    };
    import(&[first.clone(), second.clone()], DEVNET_SEED, &out).unwrap();
    let c = Corpus::load(&out).unwrap();
    assert_eq!(c.tip(), 265);
    assert_eq!(c.meta["tornRecords"], 1);
    assert_eq!(c.meta["anchorSource"], "heartwood-replay");
    assert_eq!(
        std::fs::read(out.join("blocks.jsonl")).unwrap(),
        std::fs::read(source.join("blocks.jsonl")).unwrap()
    );
    // The replay's rows are the rows the reference's node wrote for the same chain.
    for name in ["rounds.jsonl", "missed-blocks.jsonl", "meta.jsonl"] {
        let ours = live::run::read_lines(&out.join("reference").join(name)).unwrap();
        let theirs = live::run::read_lines(&source.join("reference").join(name)).unwrap();
        assert_eq!(ours.len(), theirs.len(), "{name}");
    }
    // The output must be empty; a gap, a line that is not its bytes and another seed are refused.
    assert!(import(std::slice::from_ref(&first), DEVNET_SEED, &out).is_err());
    assert!(
        import(
            std::slice::from_ref(&second),
            DEVNET_SEED,
            &scratch.path().join("gap")
        )
        .is_err()
    );
    let mut malformed = rows[0].clone();
    malformed["id"] = json!("00");
    let bad = scratch.path().join("bad.bin");
    write_lines(&bad, &[malformed]);
    assert!(import(&[bad], DEVNET_SEED, &scratch.path().join("bad-id")).is_err());
    assert!(import(&[first], "wrong-seed", &scratch.path().join("bad-seed")).is_err());
    assert!(
        !scratch
            .path()
            .join("bad-seed")
            .join("blocks.jsonl")
            .exists()
    );
}

#[test]
fn a_devnet_run_with_a_listening_forger_a_divergence_and_no_answers_fails() {
    let f = fixture();
    let d = devnet();
    // The devnet's records as run m's, beside the fixture's steps and chain.
    let path = f.run.join("run.json");
    let mut v = live::run::read_json(&path).unwrap();
    let devnet_run = live::run::read_json(&d.run.join("run.json")).unwrap();
    v["run"] = json!("m");
    v["network"] = json!("devnet");
    v["nodes"] = devnet_run["nodes"].clone();
    v["corpora"] = json!({"live-h": "archives/node1"});
    v.as_object_mut().unwrap().remove("pair");
    std::fs::write(&path, v.to_string()).unwrap();
    for n in 1..=5 {
        let from = d.run.join(format!("nodes/node{n}"));
        copy_dir(&from, &f.run.join(format!("nodes/node{n}")));
    }
    let manifest = live::run::Manifest::load(&f.run).unwrap();
    let mut reports = process_reports(&manifest);
    reports[0]["processes"][1]["listeners"] = json!([{"ip": "0.0.0.0", "port": 6002}]);
    write_lines(&f.run.join("processes.jsonl"), &reports);
    let digest = f.run.join("nodes/node5/difftest/digest-0001.jsonl");
    let mut rows = live::run::read_lines(&digest).unwrap();
    rows[10]["step"] = json!("reapply");
    rows[10]["w"] = json!("different");
    write_lines(&digest, &rows);
    std::fs::remove_file(f.run.join("nodes/node2/difftest/network-state.jsonl")).unwrap();
    let report = check(&f);
    assert!(!report.is_success());
    let failed = failures(&report);
    for needle in [
        "forger has a listening socket",
        "node5: final applied chain diverges",
        "node2: no network-state record",
        "no answer to node2's forger",
        "no relay's debug log shows a block received from a forger",
    ] {
        assert!(
            failed.iter().any(|s| s.contains(needle)),
            "{needle}: {failed:#?}"
        );
    }
}
