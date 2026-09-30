//! `difftest live check` on a pair of recorded mixed runs: run `w` (reference nodes beside two
//! Heartwood relays, node 4 the one a fresh reference node syncs from and node 6 the one the
//! recovery commands are given to) and its reference baseline `w0` (the same six nodes, all
//! reference).
//!
//! Both runs are built here from the committed corpus, which the judge has judged, so no oracle
//! runs: the chain of both is that corpus (tip 265; the manifests' stop height is the planned
//! 212, which only the schedule check reads), the Heartwood relays' digest lines are the judge's,
//! the fresh node's and the re-synced node's archives are genuine archives of that chain, and the
//! rollback's predictions are written as the files the oracle's answer is cached in. The records
//! have the devnet recipe's names and shapes (its step names, `peer-polls.jsonl`, node 6's
//! `summaries/`, each container's `processes/<node>.jsonl`). Every test
//! begins from the two runs passing in every section of the mixed checks, changes one record or
//! adds a few log lines, and reads what that section then reports.
//!
//! Times are seconds after a common origin; the timeline is one block every ten seconds, so a
//! height is a tenth of its time.

use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use heartwood_crypto::Block;
use heartwood_crypto::utils::hex;
use heartwood_difftest::Error;
use heartwood_difftest::corpus::{self, Corpus};
use heartwood_difftest::live::run::{format_time, parse_time};
use heartwood_difftest::live::{self, LiveOptions};
use heartwood_difftest::report::Report;
use heartwood_difftest::store::{self, ScratchDir};
use heartwood_difftest::vectors::sha256_hex;
use serde_json::{Value, json};

const ORIGIN: &str = "2026-09-28T10:00:00.000Z";
const ROUND: u32 = 53;
const SECTIONS: [&str; 5] = ["mixed", "fresh", "recovery", "attest", "schedule"];

// ---------------------------------------------------------------------------------------------
// Times, addresses and files
// ---------------------------------------------------------------------------------------------

fn at(seconds: f64) -> i64 {
    parse_time(ORIGIN).expect("the origin parses") + (seconds * 1_000.0).round() as i64
}

fn iso(seconds: f64) -> String {
    format_time(at(seconds))
}

/// A log line's stamp.
fn stamp(seconds: f64) -> String {
    iso(seconds)
        .replace('T', " ")
        .trim_end_matches('Z')
        .to_owned()
}

fn line(seconds: f64, level: &str, text: &str) -> String {
    format!("[{}] {level}: {text}\n", stamp(seconds))
}

/// The address of node `n`.
fn ip(n: u32) -> String {
    format!("198.51.100.{}", 10 + n)
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

fn write(path: &Path, text: impl AsRef<[u8]>) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, text).unwrap();
}

fn append(path: &Path, text: &str) {
    let mut old = std::fs::read_to_string(path).unwrap_or_default();
    old.push_str(text);
    write(path, old);
}

fn jsonl(values: &[Value]) -> String {
    values.iter().map(|v| format!("{v}\n")).collect()
}

fn read_jsonl(path: &Path) -> Vec<Value> {
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

fn edit_jsonl(path: &Path, f: impl FnOnce(&mut Vec<Value>)) {
    let mut lines = read_jsonl(path);
    f(&mut lines);
    write(path, jsonl(&lines));
}

/// Change the record of the step `name` of the run in `run`.
fn edit_step(run: &Path, name: &str, f: impl FnOnce(&mut Value)) {
    edit_jsonl(&run.join("steps.jsonl"), |steps| {
        let step = steps
            .iter_mut()
            .find(|s| s["step"] == name)
            .unwrap_or_else(|| panic!("no step {name}"));
        f(step);
    });
}

/// Node 6's store summary of `phase` (`before`, `rolled-back`, `restarted`) in the recipe's file,
/// `summaries/<phase>-node6.json`.
fn summary_path(run: &Path, phase: &str) -> PathBuf {
    run.join("summaries").join(format!("{phase}-node6.json"))
}

/// Write node 6's summary of `phase` as the recipe does: the reference's from `psql`, a Heartwood
/// relay's from `difftest store summary`.
fn write_summary(run: &Path, phase: &str, sum: &Sum, heartwood: bool) {
    let (implementation, source) = if heartwood {
        ("heartwood", "difftest store summary")
    } else {
        ("reference", "psql")
    };
    write(
        &summary_path(run, phase),
        json!({"phase": phase, "node": "node6", "impl": implementation, "source": source,
               "ok": true, "tip": {"height": sum.tip, "id": "ab".repeat(32)},
               "highestRound": sum.round, "missedRows": sum.missed,
               "transactions": sum.transactions})
        .to_string(),
    );
}

/// Change node 6's summary of `phase` in `run`.
fn edit_summary(run: &Path, phase: &str, f: impl FnOnce(&mut Value)) {
    let path = summary_path(run, phase);
    let mut value: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    f(&mut value);
    write(&path, value.to_string());
}

/// A node's log of a reference run, and a Heartwood relay's file log.
fn reference_log(run: &Path, node: &str) -> PathBuf {
    run.join("logs")
        .join(node)
        .join(format!("{node}-current.log"))
}

fn relay_log(run: &Path, node: &str) -> PathBuf {
    run.join("nodes")
        .join(node)
        .join("log")
        .join("solar-testnet-relay-current.log")
}

fn ban(seconds: f64, ip: &str, reason: &str) -> String {
    line(
        seconds,
        "INFO",
        &format!("Banning {ip} for 10 seconds. Reason: {reason}"),
    )
}

const CLOSED: &str = "Did not make an endpoint request";

// ---------------------------------------------------------------------------------------------
// The chain
// ---------------------------------------------------------------------------------------------

/// The committed corpus and what the runs read of it.
struct Chain {
    corpus: Corpus,
    /// The judge's forward lines by height.
    forward: BTreeMap<u32, Value>,
    /// The heights of the reference's missed-slot rows.
    missed: Vec<u64>,
}

fn committed() -> PathBuf {
    manifest_dir().join("corpus/txgen-one-node")
}

fn chain() -> Chain {
    let corpus = Corpus::load(&committed()).unwrap();
    let forward = store::forward_lines(&corpus).unwrap();
    let missed = read_jsonl(&committed().join("reference/missed-blocks.jsonl"))
        .iter()
        .map(|r| r["height"].as_u64().unwrap())
        .collect();
    Chain {
        corpus,
        forward,
        missed,
    }
}

/// What a store holds at a height: the tip, the highest round with rows, the missed-slot rows and
/// the transactions (the genesis block's included).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Sum {
    tip: u64,
    round: u64,
    missed: u64,
    transactions: u64,
}

impl Sum {
    fn json(&self, alternative: bool) -> Value {
        if alternative {
            json!({"height": self.tip, "highestRound": self.round, "missedRows": self.missed,
                   "transactionCount": self.transactions})
        } else {
            json!({"tip": self.tip, "round": self.round, "missed": self.missed,
                   "transactions": self.transactions})
        }
    }
}

impl Chain {
    /// The store's summary at `height` (rows of the reference's `missed` heights).
    fn sum_at(&self, height: u32, round: u64, missed: &[u64]) -> Sum {
        let mut transactions = u64::from(self.corpus.genesis.header().number_of_transactions);
        for block in self.corpus.blocks.iter().filter(|b| b.height <= height) {
            let decoded = Block::decode(&block.bytes, &self.corpus.milestones).unwrap();
            transactions += u64::from(decoded.header().number_of_transactions);
        }
        Sum {
            tip: u64::from(height),
            round,
            missed: missed.iter().filter(|m| **m <= u64::from(height)).count() as u64,
            transactions,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Archives
// ---------------------------------------------------------------------------------------------

/// A full archive of the committed chain in the devnet tooling's format. With `tweak`, one round
/// row's balance differs from the chain's.
fn build_archive(chain: &Chain, dir: &Path, tweak: bool, to: u32) {
    let corpus = &chain.corpus;
    let meta: BTreeMap<u64, Value> = read_jsonl(&committed().join("reference/meta.jsonl"))
        .into_iter()
        .map(|m| (m["h"].as_u64().unwrap(), m))
        .collect();
    let mut blocks = String::new();
    let mut transactions = 0u64;
    for h in 1..=to {
        let (bytes, id) = if h == 1 {
            (corpus.genesis.encode(), corpus.genesis.id().to_hex())
        } else {
            let b = &corpus.blocks[h as usize - 2];
            (b.bytes.clone(), b.id.to_hex())
        };
        let header_length = (1..=bytes.len())
            .find(|n| sha256_hex(&bytes[..*n]) == id)
            .unwrap_or_else(|| {
                panic!(
                    "a header hashes to the block id at {h} ({} bytes)",
                    bytes.len()
                )
            });
        let mut parts = Vec::new();
        if h > 1 {
            let count = Block::decode(&bytes, &corpus.milestones)
                .unwrap()
                .header()
                .number_of_transactions as usize;
            let mut at = header_length + 4 * count;
            for i in 0..count {
                let from = header_length + 4 * i;
                let length = u32::from_le_bytes(bytes[from..from + 4].try_into().unwrap());
                parts.push(hex::encode(&bytes[at..at + length as usize]));
                at += length as usize;
            }
            transactions += count as u64;
        }
        let extra = meta
            .get(&u64::from(h))
            .cloned()
            .unwrap_or_else(|| json!({}));
        blocks.push_str(&format!(
            "{}\n",
            json!({"height": h, "id": id, "round": (h - 1) / ROUND + 1,
                   "header": hex::encode(&bytes[..header_length]), "transactions": parts,
                   "meta": {"burnedFee": extra.get("burnedFee").cloned().unwrap_or(json!("0")),
                            "donations": extra.get("donations").cloned().unwrap_or(json!({})),
                            "username": extra.get("username").cloned().unwrap_or(json!("genesis_1"))}})
        ));
    }
    let blocks_file = format!("blocks/{:09}-{:09}.jsonl", 1, to);
    write(&dir.join(&blocks_file), blocks);
    for name in ["network", "milestones", "genesisBlock", "exceptions"] {
        let rel = format!("crypto/{name}.json");
        write(
            &dir.join(&rel),
            std::fs::read(committed().join(&rel)).unwrap(),
        );
    }
    let mut rounds = read_jsonl(&committed().join("reference/rounds.jsonl"));
    if tweak {
        let row = rounds.iter_mut().find(|r| r["round"] == 3).unwrap();
        row["balance"] = json!("1");
    }
    let rounds: Vec<Value> = rounds
        .iter()
        .map(
            |r| json!({"round": r["round"], "public_key": r["publicKey"], "balance": r["balance"]}),
        )
        .collect();
    write(&dir.join("rounds.jsonl"), jsonl(&rounds));
    std::fs::copy(
        committed().join("reference/missed-blocks.jsonl"),
        dir.join("missed-blocks.jsonl"),
    )
    .unwrap();
    let mut files = serde_json::Map::new();
    for rel in [
        blocks_file.as_str(),
        "crypto/network.json",
        "crypto/milestones.json",
        "crypto/genesisBlock.json",
        "crypto/exceptions.json",
        "rounds.jsonl",
        "missed-blocks.jsonl",
    ] {
        files.insert(
            rel.to_owned(),
            json!(sha256_hex(&std::fs::read(dir.join(rel)).unwrap())),
        );
    }
    let manifest = json!({
        "format": corpus::ARCHIVE_FORMAT,
        "chain": corpus.meta["chain"],
        "range": {"from": 1, "to": to, "blocks": to,
                  "transactions": transactions,
                  "lastId": corpus.block_id(to).unwrap().to_hex()},
        "previous": null,
        "problems": [],
        "files": files,
        "solar": {"reference": corpus.meta["reference"]},
    });
    write(&dir.join("manifest.json"), manifest.to_string());
    write(&dir.join("verify.json"), "{\"ok\":true}");
}

/// The archives of the two kinds, built once: the chain's, and one whose round rows differ.
fn archive_template(tweak: bool) -> &'static Path {
    static PLAIN: OnceLock<(ScratchDir, PathBuf)> = OnceLock::new();
    static TWEAKED: OnceLock<(ScratchDir, PathBuf)> = OnceLock::new();
    let cell = if tweak { &TWEAKED } else { &PLAIN };
    let (_, dir) = cell.get_or_init(|| {
        let scratch = ScratchDir::new("live-mixed-archive").unwrap();
        let dir = scratch.path().join("archive");
        let chain = chain();
        let tip = chain.corpus.tip();
        build_archive(&chain, &dir, tweak, tip);
        (scratch, dir)
    });
    dir
}

// ---------------------------------------------------------------------------------------------
// The runs
// ---------------------------------------------------------------------------------------------

/// A run of the pair: its name, whether nodes 4 and 6 are Heartwood relays, and node 6's rollback
/// (the height of its tip when the command came, and the height the command was given).
struct Kind {
    name: &'static str,
    heartwood: bool,
    cut: u32,
    asked: u32,
}

/// Run `w`: the rollback lands on 159, which leaves one missed-slot row.
const MIXED: Kind = Kind {
    name: "w",
    heartwood: true,
    cut: 180,
    asked: 170,
};

/// Run `w0`: the rollback lands on 106, which leaves none.
const BASELINE: Kind = Kind {
    name: "w0",
    heartwood: false,
    cut: 160,
    asked: 133,
};

impl Kind {
    fn target(&self) -> u32 {
        (self.asked - 1) / ROUND * ROUND
    }

    fn is_relay(&self, node: u32) -> bool {
        self.heartwood && (node == 4 || node == 6)
    }
}

/// The two runs, the corpus root they share and the chain they hold.
struct Fixture {
    _scratch: ScratchDir,
    root: PathBuf,
    w: PathBuf,
    w0: PathBuf,
}

fn fixture() -> Fixture {
    let scratch = ScratchDir::new("live-mixed").unwrap();
    let chain = chain();
    let root = scratch.path().join("corpus");
    let results = scratch.path().join("results");
    for kind in [&BASELINE, &MIXED] {
        copy_dir(&committed(), &root.join(format!("live-{}", kind.name)));
        build_run(&chain, kind, &results.join(kind.name));
    }
    Fixture {
        w: results.join("w"),
        w0: results.join("w0"),
        _scratch: scratch,
        root,
    }
}

fn node_records(kind: &Kind) -> Vec<Value> {
    (1..=6)
        .map(|n| {
            let delegates = [18, 18, 17, 0, 0, 0][n as usize - 1];
            json!({
                "name": format!("node{n}"),
                "ip": ip(n),
                "implementation": if kind.is_relay(n) { "heartwood" } else { "reference" },
                "role": if n <= 3 { "forger" } else { "observer" },
                "delegates": delegates,
                "underTest": kind.is_relay(n),
            })
        })
        .collect()
}

/// The fault windows: (fault, node, start, recovered) in seconds.
fn faults(kind: &Kind) -> Vec<(&'static str, &'static str, f64, f64)> {
    let mut all = vec![
        ("network-start", "node6", 5.0, 8.0),
        ("not-started", "node6", 40.0, 45.0),
        ("isolation", "node2", 300.0, 320.0),
    ];
    if kind.heartwood {
        all.push(("stall", "node4", 1_120.0, 1_135.0));
    }
    all.push(("sigterm", "node4", 1_195.0, 1_210.0));
    all.push(("rollback", "node6", 1_805.0, 1_845.0));
    all.push(("pause", "node1", 2_600.0, 2_650.0));
    all
}

/// The steps of the schedule: name, node, start, end, start height and detail.
fn schedule(chain: &Chain, kind: &Kind) -> Vec<(String, &'static str, f64, f64, u64, Value)> {
    let _ = chain;
    let batch: serde_json::Map<String, Value> = [1, 2, 3, 4, 6]
        .iter()
        .map(|n| (format!("node{n}"), json!({"seenAllMs": 2_000})))
        .collect();
    let mut steps = vec![
        ("txgen-fund", "node1", 10.0, 15.0, 4, json!({})),
        ("peers-node6", "node6", 30.0, 32.0, 30, json!({})),
        ("start-node5", "node5", 440.0, 445.0, 54, json!({})),
        ("attest-wait", "node4", 442.0, 446.0, 55, json!({})),
        ("txgen-once", "node4", 446.0, 448.0, 55, json!({})),
        (
            "node5-synced",
            "node5",
            450.0,
            455.0,
            60,
            json!({"seconds": 12.5}),
        ),
        (
            "batch-node5",
            "node5",
            460.0,
            465.0,
            62,
            json!({"relays": batch}),
        ),
        (
            "r1",
            "node6",
            480.0,
            510.0,
            66,
            json!({"admitted": true, "pooled": true, "keptAcrossRestart": true}),
        ),
        (
            "r2",
            "node6",
            520.0,
            550.0,
            70,
            json!({"goneAfterClear": true}),
        ),
        ("r3", "node6", 560.0, 590.0, 76, json!({"replayPath": true})),
        (
            "r3-vote",
            "node6",
            600.0,
            610.0,
            78,
            json!({"outcome": if kind.heartwood { "accepted" } else { "rejected" },
                   "submittedAt": iso(605.0)}),
        ),
    ];
    if kind.heartwood {
        steps.push(("stall-node4", "node4", 1_120.0, 1_140.0, 112, json!({})));
    }
    steps.extend([
        ("restart-node4", "node4", 1_200.0, 1_230.0, 131, json!({})),
        ("heal-node6", "node6", 1_250.0, 1_280.0, 133, json!({})),
        (
            "rollback-isolate",
            "node6",
            1_795.0,
            1_798.0,
            180,
            json!({}),
        ),
        ("summary-before", "node6", 1_800.0, 1_802.0, 181, json!({})),
        (
            "rollback",
            "node6",
            1_810.0,
            1_840.0,
            181,
            json!({"height": kind.asked}),
        ),
        (
            "summary-rolled-back",
            "node6",
            1_850.0,
            1_852.0,
            182,
            json!({}),
        ),
        ("rollback-start", "node6", 1_860.0, 1_890.0, 183, json!({})),
        (
            "summary-restarted",
            "node6",
            1_900.0,
            1_902.0,
            190,
            json!({}),
        ),
        (
            "rollback-open",
            "node6",
            1_950.0,
            1_990.0,
            195,
            json!({"synced": "yes", "seconds": 40}),
        ),
        ("p2p-probe", "node1", 2_100.0, 2_110.0, 210, json!({})),
        ("api-corpus", "node1", 2_200.0, 2_210.0, 211, json!({})),
    ]);
    steps
        .into_iter()
        .map(|(n, node, s, e, h, d)| (n.to_owned(), node, s, e, h, d))
        .collect()
}

/// The step records: the scripted steps between the run's set-up and its close.
fn step_records(chain: &Chain, kind: &Kind) -> Vec<Value> {
    let plain = |name: &str, node: &str, start: f64, end: f64, extra: Value| {
        let mut v =
            json!({"step": name, "node": node, "start": iso(start), "end": iso(end), "ok": true});
        if let (Some(v), Some(extra)) = (v.as_object_mut(), extra.as_object()) {
            v.extend(extra.clone());
        }
        v
    };
    let mut out = vec![
        plain("generate", "all", -200.0, -190.0, json!({})),
        plain("up", "all", -190.0, -180.0, json!({})),
        plain("settle", "node1", -180.0, -100.0, json!({"height": 3})),
    ];
    for (name, node, start, end, height, detail) in schedule(chain, kind) {
        out.push(plain(
            &name,
            node,
            start,
            end,
            json!({"startHeight": height, "detail": detail}),
        ));
    }
    // The pause step waits for the stop height and records the height the forgers paused at.
    out.push(plain(
        "pause",
        "all",
        1_995.0,
        2_650.0,
        json!({"startHeight": 196, "height": 212, "stopHeight": 212}),
    ));
    out.push(plain("check-chain", "node1", 2_660.0, 2_665.0, json!({})));
    out.push(plain("tips", "all", 2_670.0, 2_672.0, json!({})));
    out.push(plain("archive", "all", 2_680.0, 2_690.0, json!({})));
    out.push(plain("collect", "all", 2_700.0, 2_710.0, json!({})));
    out
}

/// Every poll's peer list, as the recipe writes it with the machine's load: nodes 1 to 3 list both
/// relays (as text, or as an object with the reference's `ports`), and in run `w` the fresh node
/// lists node 4 alone once it has started; its polls before then failed.
fn polls(kind: &Kind) -> Vec<Value> {
    let mut out = Vec::new();
    let mut t = 100.0;
    while t <= 2_600.0 {
        for n in 1..=3u32 {
            let peers: Vec<Value> = [4, 6]
                .iter()
                .map(|p| {
                    if n == 2 {
                        json!({"ip": ip(*p), "port": 6002,
                               "ports": {"@solar-network/api": 6003, "@solar-network/webhooks": -1}})
                    } else {
                        json!(format!("{}:6002", ip(*p)))
                    }
                })
                .collect();
            out.push(
                json!({"t": iso(t), "node": format!("node{n}"), "ok": true, "load1": 1.5,
                            "status": 200, "count": peers.len(), "peers": peers}),
            );
        }
        if kind.heartwood && t >= 500.0 {
            out.push(
                json!({"t": iso(t), "node": "node5", "ok": true, "load1": 1.5,
                            "peers": [ip(4)]}),
            );
        } else if kind.heartwood {
            out.push(
                json!({"t": iso(t), "node": "node5", "ok": false, "load1": 1.5,
                            "error": "fetch failed"}),
            );
        }
        t += 50.0;
    }
    out
}

/// The process report of a relay-only Heartwood node's container, as the recipe's scan writes it:
/// the entrypoint, the relay (listening on `address` for its peers and on every address for its
/// API) and Docker's name server.
fn container(address: &str) -> Vec<Value> {
    vec![
        json!({"kind": "process", "pid": 1, "ppid": 0, "comm": "tini",
               "cmdline": "/usr/bin/tini -s -- /opt/heartwood/entrypoint.sh"}),
        json!({"kind": "process", "pid": 7, "ppid": 1, "comm": "bash",
               "cmdline": "bash /opt/heartwood/entrypoint.sh"}),
        json!({"kind": "process", "pid": 34, "ppid": 7, "comm": "heartwood",
               "cmdline": "heartwood relay run --token solar --network testnet"}),
        json!({"kind": "listener", "proto": "tcp", "family": "4", "addr": "127.0.0.11",
               "port": 42373, "pid": 0, "comm": ""}),
        json!({"kind": "listener", "proto": "tcp", "family": "4", "addr": "0.0.0.0",
               "port": 6003, "pid": 34, "comm": "heartwood"}),
        json!({"kind": "listener", "proto": "tcp", "family": "4", "addr": address,
               "port": 6002, "pid": 34, "comm": "heartwood"}),
    ]
}

fn process_report(run: &Path, node: &str) -> PathBuf {
    run.join("processes").join(format!("{node}.jsonl"))
}

/// Every block reached every node ten seconds after the one before, node 5 half a second after
/// node 1 from the block it synced to on.
fn arrivals() -> Vec<Value> {
    let mut out = Vec::new();
    for h in 1..=265u32 {
        let t = f64::from(h) * 10.0;
        out.push(json!({"t": iso(t), "node": "node1", "h": h}));
        out.push(json!({"t": iso(t + 0.3), "node": "node4", "h": h}));
        out.push(json!({"t": iso(t + 0.3), "node": "node6", "h": h}));
        if h >= 60 {
            out.push(json!({"t": iso(t + 0.5), "node": "node5", "h": h}));
        }
    }
    out
}

/// A restart line of a digest: the judge's line at `height`, as a relay's first line of a start.
fn restart_line(chain: &Chain, height: u32) -> Value {
    let mut restart = chain.forward[&height].clone();
    restart["step"] = json!("restart");
    restart.as_object_mut().unwrap().remove("rw");
    restart["m"] = json!("0000");
    restart
}

/// The digest files of a relay: the judge's lines up to `first_end`, then a start that opens with
/// a restart line at `reopen` and holds the lines after it.
fn write_digest(chain: &Chain, relay_dir: &Path, first_end: u32, reopen: u32) {
    let lines = |range: std::ops::RangeInclusive<u32>| -> Vec<Value> {
        range
            .filter_map(|h| chain.forward.get(&h).cloned())
            .collect()
    };
    let tip = *chain.forward.keys().max().unwrap();
    let dir = relay_dir.join("difftest");
    write(&dir.join("digest-0001.jsonl"), jsonl(&lines(1..=first_end)));
    let mut second = vec![restart_line(chain, reopen)];
    second.extend(lines(reopen + 1..=tip));
    write(&dir.join("digest-0002.jsonl"), jsonl(&second));
}

fn build_run(chain: &Chain, kind: &Kind, run: &Path) {
    let tip = chain.corpus.tip();
    let tip_id = chain.corpus.block_id(tip).unwrap().to_hex();
    write(
        &run.join("run.json"),
        json!({
            "format": "heartwood-live-run/1", "run": kind.name,
            "network": format!("fixture-{}", kind.name), "dryRun": false, "seed": "solar-devnet",
            "roundLength": ROUND, "blockTime": 8, "stopHeight": 212, "control": "node1",
            "gateway": "198.51.100.1", "nodes": node_records(kind),
            "corpora": {format!("live-{}", kind.name): "archives/node1"},
            "faultWindowSeconds": 30, "status": "complete",
        })
        .to_string(),
    );
    let windows: Vec<Value> = faults(kind)
        .iter()
        .map(|(fault, node, start, recovered)| {
            json!({"fault": fault, "nodes": [node], "start": iso(*start), "recovered": iso(*recovered)})
        })
        .collect();
    write(&run.join("faults.jsonl"), jsonl(&windows));
    write(&run.join("steps.jsonl"), jsonl(&step_records(chain, kind)));
    write(&run.join("peer-polls.jsonl"), jsonl(&polls(kind)));
    // Node 6's store summaries around its rollback.
    let target = kind.target();
    let l = u64::from(ROUND);
    let sum = |height: u32, round: u64| chain.sum_at(height, round, &chain.missed);
    let cut = sum(kind.cut, u64::from(kind.cut) / l + 1);
    write_summary(run, "before", &cut, kind.heartwood);
    write_summary(
        run,
        "rolled-back",
        &sum(target, u64::from(target) / l),
        kind.heartwood,
    );
    write_summary(
        run,
        "restarted",
        &sum(target, u64::from(target) / l + 1),
        kind.heartwood,
    );
    write(&run.join("propagation.jsonl"), jsonl(&arrivals()));
    let tips: serde_json::Map<String, Value> = (1..=6)
        .map(|n| (format!("node{n}"), json!({"height": tip, "id": tip_id})))
        .collect();
    write(
        &run.join("final.json"),
        json!({"ok": true, "step": "tips", "tips": tips}).to_string(),
    );
    // Node 1's archive: only the end of its range is read, to see that the corpus under the root
    // is that chain.
    write(
        &run.join("archives/node1/20260928T120000Z/manifest.json"),
        json!({"format": corpus::ARCHIVE_FORMAT, "chain": {"epoch": chain.corpus.meta["chain"]["epoch"]},
               "range": {"from": 1, "to": tip, "lastId": tip_id}})
        .to_string(),
    );
    // The reference's own logs.
    for n in 1..=6u32 {
        if kind.is_relay(n) {
            continue;
        }
        let name = format!("node{n}");
        let mut log = line(1.0, "INFO", "Blockchain 100% in sync");
        if n <= 3 {
            for relay in [4, 6] {
                log.push_str(&line(
                    50.0,
                    "INFO",
                    &format!("Accepted new peer {}:6002", ip(relay)),
                ));
            }
        }
        if n == 5 {
            // Node 5 reaches node 4 alone.
            log.push_str(&line(
                450.0,
                "INFO",
                &format!("Accepted new peer {}:6002", ip(4)),
            ));
        }
        write(&reference_log(run, &name), log);
    }
    if kind.heartwood {
        // The fresh node's archive and the Heartwood relays' records.
        copy_dir(
            archive_template(false),
            &run.join("archives/node5/20260928T100000Z"),
        );
        for n in [4u32, 6] {
            write(
                &process_report(run, &format!("node{n}")),
                jsonl(&container(&ip(n))),
            );
        }
        for node in ["node4", "node6"] {
            write(
                &relay_log(run, node),
                line(1.0, "INFO", "Blockchain 100% in sync"),
            );
        }
        write_digest(chain, &run.join("nodes/node4"), 120, 120);
        write_digest(chain, &run.join("nodes/node6"), kind.cut, kind.target());
    } else {
        copy_dir(
            archive_template(false),
            &run.join("archives/node6/20260928T100000Z"),
        );
        // Node 6 contacted node 3 after the state clear's restart, and started again at the
        // rollback's target with the reference's lines.
        let target = kind.target();
        let round = target / ROUND + 1;
        let mut log = line(640.0, "INFO", &format!("Accepted new peer {}:6002", ip(3)));
        log.push_str(&line(
            1_815.0,
            "INFO",
            &format!("Last block in database: {target}"),
        ));
        log.push_str(&line(
            1_816.0,
            "WARN",
            &format!(
                "The block from the saved state at height {} is not in the database; the state is refused",
                kind.cut
            ),
        ));
        log.push_str(&line(
            1_816.5,
            "WARN",
            &format!(
                "The block from the saved state at height {} is not in the database; the state is refused",
                target + 1
            ),
        ));
        for (t, text) in [
            (
                1_817.0,
                "The saved state is unusable, a fresh state will now be generated".to_owned(),
            ),
            (1_818.0, "Verifying database integrity".to_owned()),
            (1_819.0, "Verified database integrity".to_owned()),
            (1_820.0, "Number of registered delegates: 51".to_owned()),
            (1_821.0, "Calculating productivity data".to_owned()),
            (1_822.0, format!("Starting Round {round}")),
            (1_823.0, format!("Saving round {round}")),
        ] {
            log.push_str(&line(t, "INFO", &text));
        }
        write(&reference_log(run, "node6"), log);
    }
    // The prediction of node 6's rollback: the file the oracle's answer is cached in.
    let target = kind.target();
    write(
        &run.join(format!("predictions/rollback-{}-{}.json", kind.cut, kind.asked)),
        json!({"target": target,
               "summary": chain.sum_at(target, u64::from(target / ROUND), &chain.missed).json(false)})
        .to_string(),
    );
}

// ---------------------------------------------------------------------------------------------
// Running the check
// ---------------------------------------------------------------------------------------------

fn check_run(f: &Fixture, run: &Path) -> Report {
    check_run_with(f, run, &[])
}

/// The check of `run` with `spread`, other reference runs of the baseline's recipe.
fn check_run_with(f: &Fixture, run: &Path, spread: &[PathBuf]) -> Report {
    let no_oracle = || -> heartwood_difftest::Result<_> {
        Err(Error::Usage(
            "the fixture's chains are judged: no oracle".into(),
        ))
    };
    live::check_with(
        run,
        &LiveOptions {
            corpus_root: f.root.clone(),
            oracle: &no_oracle,
            relay_binary: f.root.join("no-such-binary"),
            compare: (run == f.w).then(|| f.w0.clone()),
            spread: spread.to_vec(),
        },
    )
    .expect("the live check runs")
}

/// The failed cases of the mixed checks' sections, as `section: label: message`.
fn failures(report: &Report, only: &[&str]) -> Vec<String> {
    report
        .results
        .iter()
        .filter(|r| r.outcome.is_failure() && only.contains(&r.op.as_str()))
        .map(|r| format!("{}: {}: {:?}", r.op, r.label, r.outcome))
        .collect()
}

fn has(list: &[String], needle: &str) -> bool {
    list.iter().any(|l| l.contains(needle))
}

#[test]
fn the_recorded_pair_passes_every_section() {
    let f = fixture();
    for run in [&f.w0, &f.w] {
        let report = check_run(&f, run);
        let failed = failures(&report, &SECTIONS);
        assert!(failed.is_empty(), "{}: {failed:#?}", run.display());
    }
}

/// The cases of `section` whose label holds `needle`: (failed, label, the outcome's text).
fn cases_of(report: &Report, section: &str, needle: &str) -> Vec<(bool, String, String)> {
    report
        .results
        .iter()
        .filter(|r| r.op == section && r.label.contains(needle))
        .map(|r| {
            (
                r.outcome.is_failure(),
                r.label.clone(),
                format!("{:?}", r.outcome),
            )
        })
        .collect()
}

/// Assert that a case of `section` holding `needle` failed, and that its text holds `message`.
#[track_caller]
fn assert_fails(report: &Report, section: &str, needle: &str, message: &str) {
    let found = cases_of(report, section, needle);
    assert!(
        found
            .iter()
            .any(|(failed, label, text)| *failed
                && (label.contains(message) || text.contains(message))),
        "no failed {section} case holding {needle:?} and {message:?}: {found:#?}"
    );
}

/// Assert that no case of `section` holding `needle` failed, and that there is one.
#[track_caller]
fn assert_passes(report: &Report, section: &str, needle: &str) {
    let found = cases_of(report, section, needle);
    assert!(!found.is_empty(), "no {section} case holding {needle:?}");
    assert!(
        found.iter().all(|(failed, ..)| !failed),
        "a {section} case holding {needle:?} failed: {found:#?}"
    );
}

/// Every section's failures stay empty except where the test looks.
#[track_caller]
fn assert_clean(report: &Report, sections: &[&str]) {
    let failed = failures(report, sections);
    assert!(failed.is_empty(), "{failed:#?}");
}

// ---------------------------------------------------------------------------------------------
// The reference nodes' logs and peer lists
// ---------------------------------------------------------------------------------------------

#[test]
fn a_reference_ban_of_a_heartwood_address_outside_a_window_fails() {
    let f = fixture();
    append(&reference_log(&f.w, "node1"), &ban(700.0, &ip(4), CLOSED));
    let report = check_run(&f, &f.w);
    assert_fails(&report, "mixed", "reference logs", "transport in outside");
    assert_passes(&report, "mixed", "node2: no ban");
}

#[test]
fn a_ban_for_another_reason_fails_wherever_it_falls() {
    let f = fixture();
    // Inside node 4's restart window, and not for a closed socket.
    append(
        &reference_log(&f.w, "node3"),
        &ban(1_200.0, &ip(4), "Invalid message"),
    );
    let report = check_run(&f, &f.w);
    assert_fails(
        &report,
        "mixed",
        "node3: the reference logs",
        "a ban of node4",
    );
}

#[test]
fn an_unexpected_reply_fails() {
    let f = fixture();
    append(
        &reference_log(&f.w, "node2"),
        &line(
            700.0,
            "WARN",
            &format!("Got unexpected reply from {}: ping", ip(6)),
        ),
    );
    let report = check_run(&f, &f.w);
    assert_fails(
        &report,
        "mixed",
        "node2: the reference logs",
        "Got unexpected reply",
    );
}

#[test]
fn a_validation_error_fails() {
    let f = fixture();
    append(
        &reference_log(&f.w, "node1"),
        &line(
            700.0,
            "WARN",
            &format!("Socket data validation error (peer {}): bad", ip(4)),
        ),
    );
    let report = check_run(&f, &f.w);
    assert_fails(
        &report,
        "mixed",
        "node1: the reference logs",
        "validation error",
    );
}

#[test]
fn the_fresh_nodes_peer_list_holds_the_serving_node_alone() {
    let f = fixture();
    edit_jsonl(&f.w.join("peer-polls.jsonl"), |polls| {
        let poll = polls
            .iter_mut()
            .filter(|p| p["node"] == "node5")
            .nth(10)
            .unwrap();
        poll["peers"] = json!([ip(4), ip(1)]);
    });
    let report = check_run(&f, &f.w);
    assert_fails(&report, "fresh", "node5", &ip(1));
}

#[test]
fn a_reference_ban_inside_a_fault_window_of_the_named_node_is_counted_in_its_class() {
    let f = fixture();
    // The restart window of node 4 holds one closed-socket ban in each run: within the baseline.
    for run in [&f.w, &f.w0] {
        append(&reference_log(run, "node1"), &ban(1_200.0, &ip(4), CLOSED));
    }
    let report = check_run(&f, &f.w);
    assert_clean(&report, &SECTIONS);
    assert_passes(
        &report,
        "mixed",
        "reference logs: 1 counts per node and class",
    );
}

#[test]
fn lines_in_the_start_windows_are_counted_in_their_class() {
    let f = fixture();
    // One transport line of node 6 in each of its start windows, in both runs.
    for run in [&f.w, &f.w0] {
        append(
            &reference_log(run, "node1"),
            &line(10.0, "WARN", &format!("Socket error (peer {})", ip(6))),
        );
        append(
            &reference_log(run, "node1"),
            &line(50.0, "WARN", &format!("Socket error (peer {})", ip(6))),
        );
    }
    let report = check_run(&f, &f.w);
    assert_clean(&report, &SECTIONS);
    assert_passes(
        &report,
        "mixed",
        "reference logs: 2 counts per node and class",
    );
    // A second line in the network start is above the baseline's.
    append(
        &reference_log(&f.w, "node2"),
        &line(11.0, "WARN", &format!("Socket error (peer {})", ip(6))),
    );
    let report = check_run(&f, &f.w);
    assert_fails(
        &report,
        "mixed",
        "reference logs",
        "transport in network-start",
    );
}

#[test]
fn socket_closes_above_the_baseline_fail_and_the_stalls_are_left_out() {
    let f = fixture();
    let closed = |t: f64| ban(t, &ip(1), CLOSED);
    append(&reference_log(&f.w0, "node4"), &closed(1_200.0));
    append(&relay_log(&f.w, "node4"), &closed(1_200.0));
    // Five closes inside the stall, which the baseline has no window for.
    let stalled: String = (0..5).map(|i| closed(1_125.0 + f64::from(i))).collect();
    append(&relay_log(&f.w, "node4"), &stalled);
    let report = check_run(&f, &f.w);
    assert_clean(&report, &SECTIONS);
    assert_passes(
        &report,
        "mixed",
        "Heartwood logs: 1 counts per node and class",
    );
    assert_passes(
        &report,
        "mixed",
        "5 Did not make an endpoint request lines of node4 in stall, left out",
    );
    // A second close in the restart window is above the baseline's one.
    append(&relay_log(&f.w, "node4"), &closed(1_201.0));
    let report = check_run(&f, &f.w);
    assert_fails(
        &report,
        "mixed",
        "Heartwood logs",
        "Did not make an endpoint request in restart",
    );
}

/// A copy of run w0 as another reference run of its recipe, in `name/w0` beside it.
fn spread_run(f: &Fixture, name: &str) -> PathBuf {
    let dir = f.w0.parent().unwrap().join(name).join("w0");
    copy_dir(&f.w0, &dir);
    dir
}

#[test]
fn counts_are_compared_with_the_most_any_reference_run_of_the_recipe_gave() {
    let f = fixture();
    let closed = |t: f64| ban(t, &ip(4), CLOSED);
    // Node 4's restart window: w0's node 1 logged one close of node 4, another reference run of
    // the recipe two, and run w's node 1 two.
    append(&reference_log(&f.w0, "node1"), &closed(1_200.0));
    let other = spread_run(&f, "other");
    append(&reference_log(&other, "node1"), &closed(1_201.0));
    for t in [1_200.0, 1_201.0] {
        append(&reference_log(&f.w, "node1"), &closed(t));
    }
    let report = check_run(&f, &f.w);
    assert_fails(&report, "mixed", "reference logs", "transport in restart");
    let report = check_run_with(&f, &f.w, std::slice::from_ref(&other));
    assert_clean(&report, &SECTIONS);
    assert_passes(&report, "mixed", "node4 restart 2 (2)");
    assert_passes(&report, "mixed", "the most any of 2 reference runs");
    // A third line is above every reference run's count.
    append(&reference_log(&f.w, "node1"), &closed(1_202.0));
    let report = check_run_with(&f, &f.w, &[other]);
    assert_fails(
        &report,
        "mixed",
        "reference logs",
        "3 transport lines of node4 in restart, more than the baseline's 2",
    );
    // A run with Heartwood relays is no reference run.
    let mixed = f.w.parent().unwrap().join("mixed").join("w");
    copy_dir(&f.w, &mixed);
    let report = check_run_with(&f, &f.w, &[mixed]);
    assert_fails(
        &report,
        "mixed",
        "reference logs",
        "no run of reference nodes only",
    );
}

#[test]
fn a_heartwood_ban_of_a_reference_address_for_another_reason_fails() {
    let f = fixture();
    append(
        &relay_log(&f.w, "node4"),
        &ban(700.0, &ip(1), "Validation failed"),
    );
    let report = check_run(&f, &f.w);
    assert_fails(
        &report,
        "mixed",
        "node4: the Heartwood logs",
        "Validation failed",
    );
}

#[test]
fn rotated_logs_are_read() {
    let f = fixture();
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder
        .write_all(ban(700.0, &ip(1), "Validation failed").as_bytes())
        .unwrap();
    let dir = relay_log(&f.w, "node4").parent().unwrap().to_owned();
    write(
        &dir.join("solar-testnet-relay-2026-09-27.log.gz"),
        encoder.finish().unwrap(),
    );
    let report = check_run(&f, &f.w);
    assert_fails(
        &report,
        "mixed",
        "node4: the Heartwood logs",
        "Validation failed",
    );
}

// ---------------------------------------------------------------------------------------------
// Acceptance
// ---------------------------------------------------------------------------------------------

/// The polls of `node` that hold the nodes in `peers` from `from` on, as the fixture writes them.
fn drop_polls(run: &Path, node: &str, from: f64) {
    edit_jsonl(&run.join("peer-polls.jsonl"), |polls| {
        polls.retain(|p| {
            p["node"] != node || parse_time(p["t"].as_str().unwrap()).unwrap() < at(from)
        });
    });
}

#[test]
fn a_poll_inside_the_polling_nodes_isolation_may_miss_the_relay() {
    let f = fixture();
    // Node 2 was cut off from the network from 300 s to 320 s: a poll at 310 s misses node 4.
    edit_jsonl(&f.w.join("peer-polls.jsonl"), |polls| {
        polls.push(json!({"t": iso(310.0), "node": "node2", "ok": true, "peers": [ip(6)]}));
    });
    // Without the reader's own line of it accepting the relay again, the poll fails.
    let report = check_run(&f, &f.w);
    assert_fails(&report, "mixed", "node2 accepts node4", "");
    // With one after the window's start, it passes.
    append(
        &reference_log(&f.w, "node2"),
        &line(345.0, "INFO", &format!("Accepted new peer {}:6002", ip(4))),
    );
    let report = check_run(&f, &f.w);
    assert_clean(&report, &SECTIONS);
    assert_passes(&report, "mixed", "node2 accepts node4");
}

#[test]
fn a_window_followed_by_no_poll_that_lists_the_relay_fails() {
    let f = fixture();
    // Node 3's polls stop before the rollback window of node 6 closes; those of node 4's restart
    // window are all before them.
    drop_polls(&f.w, "node3", 1_500.0);
    let report = check_run(&f, &f.w);
    assert_fails(&report, "mixed", "node3 accepts node6", "");
    assert_passes(&report, "mixed", "node3 accepts node4");
}

// ---------------------------------------------------------------------------------------------
// The fresh node, the propagation and the listeners
// ---------------------------------------------------------------------------------------------

#[test]
fn a_fresh_sync_archive_whose_round_rows_differ_fails() {
    let f = fixture();
    let dir = f.w.join("archives/node5/20260928T100000Z");
    std::fs::remove_dir_all(&dir).unwrap();
    copy_dir(archive_template(true), &dir);
    let report = check_run(&f, &f.w);
    assert_fails(&report, "fresh", "node5: its rows", "round");
    // Its blocks are still the chain's.
    assert_passes(&report, "fresh", "node5: its 265 block ids");
}

#[test]
fn a_batch_that_reached_a_pool_late_fails() {
    let f = fixture();
    edit_step(&f.w, "batch-node5", |step| {
        step["detail"]["relays"]["node2"]["seenAllMs"] = json!(9_000);
    });
    let report = check_run(&f, &f.w);
    assert_fails(
        &report,
        "mixed",
        "the batch through node5",
        "node2: saw the batch after 9000",
    );
}

#[test]
fn a_fresh_node_that_lags_the_control_fails_outside_its_windows() {
    let f = fixture();
    edit_jsonl(&f.w.join("propagation.jsonl"), |lines| {
        for l in lines.iter_mut() {
            if l["node"] == "node5" && l["h"] == 150 {
                l["t"] = json!(iso(1_500.0 + 12.0));
            }
        }
    });
    let report = check_run(&f, &f.w);
    assert_fails(&report, "mixed", "node5", "within 8 s");
}

#[test]
fn a_heartwood_relay_listening_on_every_address_fails() {
    let f = fixture();
    edit_jsonl(&process_report(&f.w, "node4"), |lines| {
        lines.retain(|l| l["port"] != 6002);
        lines.push(json!({"kind": "listener", "addr": "0.0.0.0", "port": 6002, "pid": 34}));
    });
    let report = check_run(&f, &f.w);
    assert_fails(
        &report,
        "mixed",
        "the processes and their listeners",
        "not on its own address",
    );
}

// ---------------------------------------------------------------------------------------------
// Node 6 after the rollback
// ---------------------------------------------------------------------------------------------

#[test]
fn node_6_in_run_w_starts_at_the_rollbacks_target() {
    let f = fixture();
    let report = check_run(&f, &f.w);
    assert_passes(
        &report,
        "mixed",
        "node6: start 2 opens at the rollback's target 159",
    );
    // Without the start's opening line the digest has a start that is not the rollback's.
    let path = f.w.join("nodes/node6/difftest/digest-0002.jsonl");
    edit_jsonl(&path, |lines| {
        lines.remove(0);
    });
    let report = check_run(&f, &f.w);
    let failed = failures(&report, &SECTIONS);
    assert!(has(&failed, "node6"), "{failed:#?}");
}

#[test]
fn run_w0s_node_6_starts_at_the_target_with_the_reference_lines() {
    let f = fixture();
    let report = check_run(&f, &f.w0);
    assert_passes(
        &report,
        "recovery",
        "the lines of the start after the rollback",
    );
    assert_passes(
        &report,
        "recovery",
        "\"Saving round 3\" in the reference's order",
    );
}

/// Rewrite the lines of run `w0`'s node 6 log.
fn edit_node6_log(f: &Fixture, change: impl FnOnce(String) -> String) {
    let path = reference_log(&f.w0, "node6");
    let text = std::fs::read_to_string(&path).unwrap();
    write(&path, change(text));
}

#[test]
fn a_start_whose_lines_are_out_of_order_or_missing_fails() {
    let f = fixture();
    edit_node6_log(&f, |t| {
        t.replace("Verifying database integrity", "TMP")
            .replace(
                "Verified database integrity",
                "Verifying database integrity",
            )
            .replace("TMP", "Verified database integrity")
    });
    let report = check_run(&f, &f.w0);
    assert_fails(
        &report,
        "recovery",
        "the lines of the start after the rollback",
        "after the previous one",
    );
}

#[test]
fn a_start_without_the_rounds_save_fails() {
    let f = fixture();
    edit_node6_log(&f, |t| t.replace("Saving round 3", "Nothing"));
    let report = check_run(&f, &f.w0);
    assert_fails(
        &report,
        "recovery",
        "the lines of the start after the rollback",
        "Saving round 3",
    );
}

#[test]
fn the_productivity_calculation_belongs_to_a_start_that_left_no_missed_row() {
    let f = fixture();
    // Node 6 reports one missed row left after the rollback, and the log still calculates.
    edit_summary(&f.w0, "rolled-back", |summary| {
        summary["missedRows"] = json!(1);
    });
    let report = check_run(&f, &f.w0);
    assert_fails(
        &report,
        "recovery",
        "the lines of the start after the rollback",
        "Calculating productivity data",
    );
}

#[test]
fn a_loader_that_refused_a_state_at_or_below_the_target_fails() {
    let f = fixture();
    edit_node6_log(&f, |t| t.replace("at height 107 ", "at height 100 "));
    let report = check_run(&f, &f.w0);
    assert_fails(
        &report,
        "recovery",
        "the lines of the start after the rollback",
        "at or below the target",
    );
}

#[test]
fn a_summary_after_the_start_without_the_next_rounds_rows_fails() {
    let f = fixture();
    edit_summary(&f.w0, "restarted", |summary| {
        summary["highestRound"] = json!(2);
    });
    let report = check_run(&f, &f.w0);
    assert_fails(
        &report,
        "recovery",
        "the summary after the start",
        "round 3",
    );
}

#[test]
fn a_prediction_that_differs_from_node_6s_summary_fails() {
    let f = fixture();
    let cache = f.w0.join("predictions/rollback-160-133.json");
    let mut prediction: Value = serde_json::from_slice(&std::fs::read(&cache).unwrap()).unwrap();
    prediction["summary"]["transactions"] =
        json!(prediction["summary"]["transactions"].as_u64().unwrap() + 1);
    write(&cache, prediction.to_string());
    let report = check_run(&f, &f.w0);
    assert_fails(
        &report,
        "recovery",
        "the summary after the rollback",
        "transactions",
    );
}

#[test]
fn a_corpus_cut_that_differs_from_node_6s_summary_before_the_command_fails() {
    let f = fixture();
    edit_summary(&f.w0, "before", |summary| {
        summary["transactions"] = json!(1);
    });
    let report = check_run(&f, &f.w0);
    assert_fails(
        &report,
        "recovery",
        "the summary before the rollback",
        "transactions",
    );
}

#[test]
fn the_two_runs_may_hold_different_transaction_counts() {
    let f = fixture();
    let report_w = check_run(&f, &f.w);
    let report_w0 = check_run(&f, &f.w0);
    // The rollback in w keeps a missed row's block, and w0's does not: 149 and 148 transactions,
    // each equal to its own prediction.
    assert_passes(&report_w, "recovery", "149 transactions");
    assert_passes(&report_w, "recovery", "148 transactions");
    assert_passes(&report_w0, "recovery", "148 transactions");
}

// ---------------------------------------------------------------------------------------------
// The backfill of the slots before block 2
// ---------------------------------------------------------------------------------------------

/// Put block 2 of run `run`'s chain into slot 3: the reference's productivity backfill then adds
/// the rows of slots 1 and 2 at height 2 at a start whose store holds no missed row, which no live
/// node writes.
fn move_block_2_to_slot_3(f: &Fixture, run: &str) {
    let dir = f.root.join(format!("live-{run}"));
    let path = dir.join("blocks.jsonl");
    let mut rows = read_jsonl(&path);
    let block = rows.iter_mut().find(|r| r["h"] == 2).unwrap();
    let hex = block["hex"].as_str().unwrap().to_owned();
    block["hex"] = json!(format!("{}{}{}", &hex[..8], "18000000", &hex[16..]));
    write(&path, jsonl(&rows));
    corpus::write_manifest(&dir).unwrap();
}

/// Run w's rollback as run w0's: given 133 from its tip 180, it lands on 106 and leaves no missed
/// row, so the reference's start after it runs the backfill.
fn roll_w_back_to_106(f: &Fixture) {
    let chain = chain();
    let sum = |round: u64| chain.sum_at(106, round, &chain.missed);
    assert_eq!(sum(2).missed, 0);
    edit_step(&f.w, "rollback", |step| {
        step["detail"] = json!({"height": 133});
    });
    write_summary(&f.w, "rolled-back", &sum(2), true);
    write_summary(&f.w, "restarted", &sum(3), true);
    write(
        &f.w.join("predictions/rollback-180-133.json"),
        json!({"target": 106, "summary": sum(2).json(false)}).to_string(),
    );
}

/// Set the missed rows of node 6's summary after the start in `run`.
fn missed_after_start(run: &Path, missed: u64) {
    edit_summary(run, "restarted", |summary| {
        summary["missedRows"] = json!(missed);
    });
}

#[test]
fn heartwoods_start_must_hold_the_backfilled_rows_when_block_2_is_not_in_slot_1() {
    let f = fixture();
    roll_w_back_to_106(&f);
    move_block_2_to_slot_3(&f, "w");
    // Node 6 of run w is Heartwood's: its start writes the two rows, as the reference's does.
    let report = check_run(&f, &f.w);
    assert_fails(
        &report,
        "recovery",
        "the summary after the start",
        "0 missed rows against the oracle's 2",
    );
    assert!(
        cases_of(&report, "recovery", "more missed rows").is_empty(),
        "no caveat is recorded"
    );
    missed_after_start(&f.w, 2);
    let report = check_run(&f, &f.w);
    assert_passes(&report, "recovery", "the summary after the start");
    // Any other difference fails.
    missed_after_start(&f.w, 1);
    let report = check_run(&f, &f.w);
    assert_fails(
        &report,
        "recovery",
        "the summary after the start",
        "1 missed rows against the oracle's 2",
    );
}

#[test]
fn the_same_difference_on_a_chain_whose_block_2_is_in_slot_1_fails() {
    let f = fixture();
    roll_w_back_to_106(&f);
    let report = check_run(&f, &f.w);
    assert_passes(&report, "recovery", "the summary after the start");
    // Two rows apart from the reference's start, which adds none on this chain.
    missed_after_start(&f.w, 2);
    let report = check_run(&f, &f.w);
    assert_fails(
        &report,
        "recovery",
        "the summary after the start",
        "2 missed rows against the oracle's 0",
    );
}

#[test]
fn the_references_start_must_hold_the_backfilled_rows() {
    let f = fixture();
    move_block_2_to_slot_3(&f, "w0");
    // Run w0's node 6 is the reference: without the two rows its start differs from the oracle's.
    let report = check_run(&f, &f.w0);
    assert_fails(
        &report,
        "recovery",
        "the summary after the start",
        "0 missed rows against the oracle's 2",
    );
    missed_after_start(&f.w0, 2);
    let report = check_run(&f, &f.w0);
    assert_passes(&report, "recovery", "the summary after the start");
    assert!(
        cases_of(&report, "recovery", "more missed rows").is_empty(),
        "the reference's own rows are no caveat"
    );
}

#[test]
fn the_backfill_excuses_no_difference_before_the_start() {
    let f = fixture();
    move_block_2_to_slot_3(&f, "w0");
    missed_after_start(&f.w0, 2);
    // The oracle's rollback counting two more rows than node 6 is a difference: nothing started.
    let cache = f.w0.join("predictions/rollback-160-133.json");
    let mut prediction: Value = serde_json::from_slice(&std::fs::read(&cache).unwrap()).unwrap();
    prediction["summary"]["missed"] = json!(2);
    write(&cache, prediction.to_string());
    let report = check_run(&f, &f.w0);
    assert_fails(
        &report,
        "recovery",
        "the summary after the rollback",
        "0 missed rows against the oracle's 2",
    );
}

/// Node 6's digest files in `run`: a first start to `first_end`, a restart there that reaches the
/// cut, and the start after the rollback, at its target.
fn restart_node6_at(chain: &Chain, run: &Path, first_end: u32) {
    let dir = run.join("nodes/node6/difftest");
    let lines = |range: std::ops::RangeInclusive<u32>| -> Vec<Value> {
        range
            .filter_map(|h| chain.forward.get(&h).cloned())
            .collect()
    };
    let tip = *chain.forward.keys().max().unwrap();
    let mut second = vec![restart_line(chain, first_end)];
    second.extend(lines(first_end + 1..=MIXED.cut));
    let mut third = vec![restart_line(chain, MIXED.target())];
    third.extend(lines(MIXED.target() + 1..=tip));
    write(&dir.join("digest-0001.jsonl"), jsonl(&lines(1..=first_end)));
    write(&dir.join("digest-0002.jsonl"), jsonl(&second));
    write(&dir.join("digest-0003.jsonl"), jsonl(&third));
}

/// Node 6's three summaries in run w with `extra` missed rows each.
fn node6_summaries_with(f: &Fixture, extra: u64) {
    let chain = chain();
    let target = MIXED.target();
    let with = |height: u32, round: u64| {
        let sum = chain.sum_at(height, round, &chain.missed);
        Sum {
            missed: sum.missed + extra,
            ..sum
        }
    };
    let l = u64::from(ROUND);
    write_summary(
        &f.w,
        "before",
        &with(MIXED.cut, u64::from(MIXED.cut) / l + 1),
        true,
    );
    write_summary(
        &f.w,
        "rolled-back",
        &with(target, u64::from(target) / l),
        true,
    );
    write_summary(
        &f.w,
        "restarted",
        &with(target, u64::from(target) / l + 1),
        true,
    );
}

#[test]
fn rows_a_start_before_the_rollback_backfilled_are_in_node_6s_every_summary() {
    let f = fixture();
    move_block_2_to_slot_3(&f, "w");
    // Node 6 also restarted at 50, below the chain's first missed row: that start found no row and
    // wrote the two rows of the slots before block 2, as the reference's start does; they stay
    // through the rollback, and the start after it, which finds them, writes none.
    restart_node6_at(&chain(), &f.w, 50);
    node6_summaries_with(&f, 2);
    let report = check_run(&f, &f.w);
    for summary in [
        "the summary before the rollback",
        "the summary after the rollback",
        "the summary after the start",
    ] {
        assert_passes(&report, "recovery", summary);
    }
    // One row fewer is a difference.
    node6_summaries_with(&f, 1);
    let report = check_run(&f, &f.w);
    assert_fails(
        &report,
        "recovery",
        "the summary before the rollback",
        "3 missed rows against the cut's 2 and the start's 2 rows before block 2",
    );
}

#[test]
fn backfilled_rows_without_a_start_that_wrote_them_fail() {
    let f = fixture();
    move_block_2_to_slot_3(&f, "w");
    // Node 6 never started with an empty table before the rollback: its first start was on an
    // empty store and the next is the one after the rollback.
    node6_summaries_with(&f, 2);
    let report = check_run(&f, &f.w);
    assert_fails(
        &report,
        "recovery",
        "the summary before the rollback",
        "4 missed rows against the cut's 2",
    );
    // A restart at a tip above the chain's first missed row (154) found rows: it wrote none.
    restart_node6_at(&chain(), &f.w, 170);
    let report = check_run(&f, &f.w);
    assert_fails(
        &report,
        "recovery",
        "the summary before the rollback",
        "4 missed rows against the cut's 2",
    );
}

#[test]
fn a_prediction_read_from_the_runs_file_says_so() {
    let f = fixture();
    let report = check_run(&f, &f.w);
    assert_passes(
        &report,
        "recovery",
        "read from predictions/rollback-180-170.json, not asked of the oracle",
    );
    assert_passes(
        &report,
        "recovery",
        "derived by the reference's rules from the rollback's prediction",
    );
}

#[test]
fn a_transport_line_outside_every_window_fails_even_when_the_baseline_has_one() {
    let f = fixture();
    for run in [&f.w, &f.w0] {
        append(&reference_log(run, "node1"), &ban(700.0, &ip(4), CLOSED));
    }
    let report = check_run(&f, &f.w);
    assert_fails(&report, "mixed", "reference logs", "transport in outside");
}

#[test]
fn the_fresh_node_accepts_the_one_relay_it_reaches() {
    let f = fixture();
    let report = check_run(&f, &f.w);
    assert_passes(&report, "mixed", "node5 accepts node4");
    // Node 5 reaches node 4 alone: nothing is asked of it about node 6.
    assert!(cases_of(&report, "mixed", "node5 accepts node6").is_empty());
    write(
        &reference_log(&f.w, "node5"),
        line(1.0, "INFO", "Blockchain 100% in sync"),
    );
    let report = check_run(&f, &f.w);
    assert_fails(&report, "mixed", "node5 accepts node4", "never logged");
}

#[test]
fn a_relay_whose_api_port_no_poll_records_fails() {
    let f = fixture();
    edit_jsonl(&f.w.join("peer-polls.jsonl"), |polls| {
        for p in polls.iter_mut() {
            if let Some(peers) = p["peers"].as_array_mut() {
                for e in peers.iter_mut() {
                    if let Some(ip) = e.get("ip").and_then(Value::as_str) {
                        *e = json!(format!("{ip}:6002"));
                    }
                }
            }
        }
    });
    let report = check_run(&f, &f.w);
    assert_fails(&report, "mixed", "node4's API port", "is not shown");
}

#[test]
fn a_relay_without_its_process_report_fails() {
    let f = fixture();
    let report = check_run(&f, &f.w);
    assert_passes(
        &report,
        "mixed",
        "a relay-only node on its own address only",
    );
    // The API listens on every address: recorded.
    assert!(
        !cases_of(&report, "mixed", "also listens on 0.0.0.0:6003").is_empty(),
        "{report:#?}"
    );
    std::fs::remove_file(process_report(&f.w, "node6")).unwrap();
    let report = check_run(&f, &f.w);
    assert_fails(
        &report,
        "mixed",
        "the processes and their listeners",
        "node6: no process in its report",
    );
}

#[test]
fn a_fresh_archive_that_ends_below_the_paused_tip_fails() {
    let f = fixture();
    let dir = f.w.join("archives/node5/20260928T100000Z");
    std::fs::remove_dir_all(&dir).unwrap();
    build_archive(&chain(), &dir, false, 200);
    let report = check_run(&f, &f.w);
    assert_fails(
        &report,
        "fresh",
        "node5: its block ids",
        "ends at height 200, below the serving chain's paused tip 265",
    );
}

// ---------------------------------------------------------------------------------------------
// The vote, the re-sync and the schedule
// ---------------------------------------------------------------------------------------------

#[test]
fn a_vote_after_the_contact_with_node_3_is_recorded_as_not_shown_live() {
    let f = fixture();
    edit_step(&f.w0, "r3-vote", |step| {
        step["detail"]["submittedAt"] = json!(iso(700.0));
    });
    let report = check_run(&f, &f.w0);
    assert_clean(&report, &SECTIONS);
    let found = cases_of(&report, "recovery", "not shown live");
    assert!(
        found
            .iter()
            .any(|(_, label, _)| label.starts_with("recorded:")),
        "{found:#?}"
    );
}

#[test]
fn a_failed_attempt_to_reach_node_3_is_no_contact() {
    let f = fixture();
    // While isolated, node 6 logs its failing attempts to reach node 3 (between r3's start and
    // end, and before the vote): the vote still came before its first contact.
    let text = std::fs::read_to_string(reference_log(&f.w0, "node6")).unwrap();
    let attempts = format!(
        "{}{}",
        line(
            570.0,
            "DEBUG",
            &format!(
                "Socket error (peer {}): connect ECONNREFUSED :warning:",
                ip(3)
            )
        ),
        line(
            600.0,
            "DEBUG",
            &format!(
                "Response error (peer {}/p2p.peer.getStatus) : timeout :warning:",
                ip(3)
            )
        ),
    );
    write(&reference_log(&f.w0, "node6"), format!("{attempts}{text}"));
    let report = check_run(&f, &f.w0);
    assert!(cases_of(&report, "recovery", "not shown live").is_empty());
    assert_passes(&report, "recovery", "was refused before node 6's contact");
    // A contact the recipe recorded before the vote makes it not shown live.
    edit_step(&f.w0, "r3-vote", |step| {
        step["detail"]["contactNode3At"] = json!(iso(565.0));
    });
    let report = check_run(&f, &f.w0);
    assert!(!cases_of(&report, "recovery", "not shown live").is_empty());
}

#[test]
fn a_vote_before_the_contact_that_was_admitted_fails() {
    let f = fixture();
    edit_step(&f.w0, "r3-vote", |step| {
        step["detail"]["admitted"] = json!(true);
    });
    let report = check_run(&f, &f.w0);
    assert_fails(
        &report,
        "recovery",
        "the vote after the state clear",
        "not refused",
    );
}

#[test]
fn a_re_sync_that_ended_on_another_block_fails() {
    let f = fixture();
    edit_jsonl_object(&f.w0.join("final.json"), |v| {
        v["tips"]["node6"]["id"] = json!("00".repeat(32));
    });
    let report = check_run(&f, &f.w0);
    assert_fails(&report, "recovery", "re-synced", "");
}

/// Change the JSON object in a file.
fn edit_jsonl_object(path: &Path, f: impl FnOnce(&mut Value)) {
    let mut value: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    f(&mut value);
    write(path, value.to_string());
}

#[test]
fn a_step_out_of_its_round_or_out_of_order_fails() {
    let f = fixture();
    edit_step(&f.w, "start-node5", |step| {
        step["startHeight"] = json!(30);
    });
    let report = check_run(&f, &f.w);
    assert_fails(&report, "schedule", "every step", "start-node5");
    let f = fixture();
    edit_step(&f.w, "r2", |step| {
        step["start"] = json!(iso(505.0));
    });
    let report = check_run(&f, &f.w);
    assert_fails(&report, "schedule", "every step", "r2");
}
