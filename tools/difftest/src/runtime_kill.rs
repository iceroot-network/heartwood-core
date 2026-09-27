//! The kill test through the full runtime: a child relay imports a corpus from a mock peer on
//! this host (named with the relay's seed switch of the harness) and is killed with SIGKILL at
//! random moments; each restart must restore a committed tip whose digest equals the continuous
//! line, and the import must resume to the end with equal lines. In half the kills the mock peer
//! answers its first getBlocks with nothing, so the chunk size falls to a tenth.
//!
//! Each kill starts a mock peer (`mock-peer`) serving the corpus and a relay (`heartwood relay
//! run`, both built next to this program with the harness's features) on the store of the current
//! run, waits for the relay's restart line (its start restored the store), then for a random delay
//! of the import, and kills both. The relay's digest lines must equal the corpus's continuous lines
//! (a restart line in the state sections, an apply line in every section of its step), a torn last
//! line excepted; the store must restore by both modes to one tip, no lower than the tip before the
//! kill and no higher than the last applied block, whose line equals the continuous line. A run
//! that reached the corpus's tip is followed by a new store; after the kills, a last relay imports
//! the rest of the chain without a kill.

use std::io::{BufRead, BufReader};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::corpus::{CRYPTO_FILES, Corpus};
use crate::errors::{Error, Result};
use crate::report::{CaseResult, Outcome, Report};
use crate::restart::{SAME_STATE, SAME_STEP, differing_keys};
use crate::store::{self, Checked, ScratchDir};

/// How long a relay may take to restore its store and write its restart line.
const START: Duration = Duration::from_secs(60);
/// How long the last relay may take to import the rest of the corpus.
const FINISH: Duration = Duration::from_secs(600);
/// The relay's profile in `app.json`: the reference's relay components, without the API and the
/// webhooks, which the import does not need.
const PLUGINS: [&str; 7] = [
    "@solar-network/logger",
    "@solar-network/state",
    "@solar-network/database",
    "@solar-network/transactions",
    "@solar-network/pool",
    "@solar-network/p2p",
    "@solar-network/blockchain",
];

/// `difftest store crash --runtime`: kill a child relay `kills` times after up to `max_delay`,
/// with the delays drawn from `seed`; `exe` is this program, which runs the child relay.
pub fn crash(
    corpus: &Corpus,
    exe: &Path,
    kills: u32,
    seed: u64,
    max_delay: Duration,
) -> Result<Report> {
    let bin = exe.parent().unwrap_or(Path::new("."));
    let (relay, mock) = (bin.join("heartwood"), bin.join("mock-peer"));
    for program in [&relay, &mock] {
        if !program.exists() {
            let missing = program.display();
            return Err(Error::Usage(format!(
                "{missing} is not built: run `make build` first"
            )));
        }
    }
    let forward = store::forward_lines(corpus)?;
    let scratch = ScratchDir::new("runtime-crash")?;
    let node = Node::write(scratch.path(), corpus)?;
    let mut state = seed;
    let (mut results, mut store, mut previous) = (Vec::new(), 0_u32, None);
    let max_ms = u64::try_from(max_delay.as_millis())
        .unwrap_or(u64::MAX)
        .max(1);
    for kill in 1..=kills {
        let delay = Duration::from_millis(split_mix(&mut state) % max_ms);
        let run = Run {
            relay: &relay,
            mock: &mock,
            corpus,
            node: &node,
            store,
        };
        let digest = scratch.path().join(format!("digest-{kill}.jsonl"));
        let killed = run.once(&digest, kill % 2 == 0, Stop::After(delay))?;
        let judged = judge(corpus, &node.data(store), &killed, previous, &forward)?;
        let label = format!(
            "kill {kill} {} ms into the import: applied up to {:?}, stored tip {:?}",
            delay.as_millis(),
            killed.last_applied(),
            judged.tip
        );
        results.push(case(kill, label, &judged));
        previous = judged.tip;
        if judged.tip.is_some_and(|tip| tip >= corpus.tip()) {
            (store, previous) = (store.saturating_add(1), None);
        }
    }
    let run = Run {
        relay: &relay,
        mock: &mock,
        corpus,
        node: &node,
        store,
    };
    let digest = scratch.path().join("digest-last.jsonl");
    let last = run.once(&digest, false, Stop::AtTip)?;
    let mut judged = judge(corpus, &node.data(store), &last, previous, &forward)?;
    if judged.tip != Some(corpus.tip()) {
        let tip = corpus.tip();
        judged.problems.push(format!(
            "the last relay stopped at {:?}, not the tip {tip}",
            judged.tip
        ));
    }
    let label = format!("the import resumed to the end: stored tip {:?}", judged.tip);
    results.push(case(kills.saturating_add(1), label, &judged));
    Ok(Report {
        title: "L2 kill test: the relay's store after a SIGKILL".into(),
        source: format!(
            "{kills} kills, seed {seed}, delays up to {} ms into the import, {} stores",
            max_delay.as_millis(),
            store.saturating_add(1)
        ),
        files: 1,
        results,
    })
}

/// A relay's directories and configuration, built from a corpus: `config/` with `app.json`,
/// `peers.json` (no seed: the mock peer is the seed switch's), `.env` and `crypto/`.
struct Node {
    root: PathBuf,
    network: String,
}

impl Node {
    fn write(root: &Path, corpus: &Corpus) -> Result<Node> {
        let config = root.join("config");
        let crypto = config.join("crypto");
        mkdir(&crypto)?;
        for name in CRYPTO_FILES {
            let file = format!("{name}.json");
            let from = corpus.dir.join("crypto").join(&file);
            std::fs::copy(&from, crypto.join(&file)).map_err(|e| Error::io(&from, e))?;
        }
        let network_file = corpus.dir.join("crypto/network.json");
        let text =
            std::fs::read_to_string(&network_file).map_err(|e| Error::io(&network_file, e))?;
        let network: Value = serde_json::from_str(&text)
            .map_err(|e| Error::Usage(format!("{}: {e}", network_file.display())))?;
        let network = network["name"].as_str().unwrap_or("devnet").to_owned();
        let plugins: Vec<_> = PLUGINS
            .iter()
            .map(|package| json!({ "package": package }))
            .collect();
        let files = [
            (
                "app.json",
                json!({ "relay": { "plugins": plugins } }).to_string(),
            ),
            ("peers.json", json!({ "list": [] }).to_string()),
            (
                "config.json",
                json!({ "token": "solar", "network": network }).to_string(),
            ),
            (
                ".env",
                "CORE_P2P_PORT=4000\nCORE_P2P_MIN_NETWORK_REACH=1\nCORE_API_DISABLED=true\n".into(),
            ),
        ];
        for (name, text) in files {
            let path = config.join(name);
            std::fs::write(&path, text).map_err(|e| Error::io(&path, e))?;
        }
        Ok(Node {
            root: root.to_path_buf(),
            network,
        })
    }

    /// The data directory of the `store`th store.
    fn data(&self, store: u32) -> PathBuf {
        self.root.join(format!("data-{store}"))
    }
}

fn mkdir(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path).map_err(|e| Error::io(path, e))
}

/// When a relay is killed.
#[derive(Debug, Clone, Copy)]
enum Stop {
    /// This long after its restart line, or at the corpus's tip.
    After(Duration),
    /// At the corpus's tip.
    AtTip,
}

/// One relay's run on a store.
struct Run<'a> {
    relay: &'a Path,
    mock: &'a Path,
    corpus: &'a Corpus,
    node: &'a Node,
    store: u32,
}

/// What a killed relay left: its digest lines, and whether it exited before the kill.
struct Killed {
    lines: Vec<Value>,
    problems: Vec<String>,
}

impl Killed {
    fn last_applied(&self) -> Option<u32> {
        let applied = self.lines.iter().filter(|line| line["step"] == "apply");
        applied.filter_map(height).max()
    }
}

/// A child process killed when dropped.
struct Guard(Child);

impl Drop for Guard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

impl Run<'_> {
    /// Start a mock peer (answering its first getBlocks with nothing when `empty_first`) and a
    /// relay writing its digest lines to `digest`, and kill both at `stop`.
    fn once(&self, digest: &Path, empty_first: bool, stop: Stop) -> Result<Killed> {
        let (mock, seed) = self.start_mock(empty_first)?;
        let data = self.node.data(self.store);
        for dir in ["temp", "log", "cache", "home"] {
            mkdir(&data.join(dir))?;
        }
        let mut relay = Guard(
            Command::new(self.relay)
                .args([
                    "relay",
                    "run",
                    "--token",
                    "solar",
                    "--network",
                    &self.node.network,
                ])
                .arg("--difftest-seed")
                .arg(seed.to_string())
                .arg("--difftest-digest")
                .arg(digest)
                .env("CORE_PATH_CONFIG", self.node.root.join("config"))
                .env("CORE_PATH_DATA", &data)
                .env("CORE_PATH_TEMP", data.join("temp"))
                .env("CORE_PATH_LOG", data.join("log"))
                .env("CORE_PATH_CACHE", data.join("cache"))
                .env("HOME", data.join("home"))
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .map_err(|e| Error::io(self.relay, e))?,
        );
        let tip = self.corpus.tip();
        let at_tip = |lines: &[Value]| {
            lines
                .iter()
                .any(|l| l["step"] == "apply" && height(l) == Some(tip))
        };
        let mut problems = Vec::new();
        let started = Instant::now();
        let mut restarted: Option<Instant> = None;
        loop {
            std::thread::sleep(Duration::from_millis(5));
            let lines = read_lines(digest);
            if restarted.is_none() && !lines.is_empty() {
                restarted = Some(Instant::now());
            }
            let due = match (stop, restarted) {
                (_, None) => started.elapsed() > START,
                (Stop::After(delay), Some(at)) => at.elapsed() >= delay || at_tip(&lines),
                (Stop::AtTip, Some(at)) => at_tip(&lines) || at.elapsed() > FINISH,
            };
            if let Ok(Some(status)) = relay.0.try_wait() {
                problems.push(format!("the relay exited on its own ({status})"));
                break;
            }
            if due {
                if restarted.is_none() {
                    problems.push(format!("the relay wrote no line within {START:?}"));
                }
                break;
            }
        }
        drop(relay);
        drop(mock);
        Ok(Killed {
            lines: read_lines(digest),
            problems,
        })
    }

    /// A mock peer serving the corpus on a free port of this host, and its address.
    fn start_mock(&self, empty_first: bool) -> Result<(Guard, SocketAddr)> {
        let mut command = Command::new(self.mock);
        command
            .args(["--listen", "127.0.0.1:0", "--chain"])
            .arg(&self.corpus.dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        if empty_first {
            command.arg("--empty-first-getblocks");
        }
        let mut child = Guard(command.spawn().map_err(|e| Error::io(self.mock, e))?);
        let out = child.0.stdout.take();
        let line = out.and_then(|out| BufReader::new(out).lines().next()?.ok());
        let address = line
            .as_deref()
            .and_then(|line| line.strip_prefix("mock-peer: listening on "))
            .and_then(|address| address.trim().parse().ok());
        match address {
            Some(address) => Ok((child, address)),
            None => Err(Error::Usage(format!(
                "the mock peer did not start: {line:?}"
            ))),
        }
    }
}

/// The digest lines of `path` written so far: a last line cut by the kill is left out.
fn read_lines(path: &Path) -> Vec<Value> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let complete = match text.rfind('\n') {
        Some(end) => &text[..end],
        None => "",
    };
    complete
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).unwrap_or(Value::Null))
        .collect()
}

fn height(line: &Value) -> Option<u32> {
    line["h"].as_u64().and_then(|h| u32::try_from(h).ok())
}

/// What one kill found.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Judged {
    tip: Option<u32>,
    problems: Vec<String>,
}

/// Judge a killed relay: its lines against the continuous lines, then the store it left by both
/// restore modes.
fn judge(
    corpus: &Corpus,
    data: &Path,
    killed: &Killed,
    previous: Option<u32>,
    forward: &std::collections::BTreeMap<u32, Value>,
) -> Result<Judged> {
    let mut problems = killed.problems.clone();
    problems.extend(judge_lines(&killed.lines, previous, forward));
    let checked = store::check(corpus, data)?;
    let (tip, found) = judge_store(&checked, previous, killed.last_applied(), forward);
    problems.extend(found);
    Ok(Judged { tip, problems })
}

/// The lines' problems: a line that does not read, a first line that is not the restart at the
/// tip before the kill (or the genesis block's for a new store), and a line that differs from the
/// continuous line of its height.
fn judge_lines(
    lines: &[Value],
    previous: Option<u32>,
    forward: &std::collections::BTreeMap<u32, Value>,
) -> Vec<String> {
    let mut problems = Vec::new();
    // A new store's first start may write no restart line for the genesis block.
    let first = lines.first();
    let new_store = previous.is_none() && first.is_some_and(|l| l["step"] == "apply");
    if let Some(first) = first.filter(|_| !new_store) {
        let start = previous.unwrap_or(1);
        if first["step"] != "restart" || height(first) != Some(start) {
            problems.push(format!(
                "the first line is not the restart at {start}: {first}"
            ));
        }
    }
    for (index, line) in lines.iter().enumerate() {
        let keys: &[&str] = match line["step"].as_str() {
            Some("restart") => &SAME_STATE,
            Some("apply") => &SAME_STEP,
            _ => {
                problems.push(format!(
                    "line {}: not a restart or an apply line: {line}",
                    index + 1
                ));
                continue;
            }
        };
        match height(line).and_then(|h| forward.get(&h)) {
            Some(want) => {
                let differing = differing_keys(line, want, keys);
                if !differing.is_empty() {
                    let (n, keys) = (index + 1, differing.join(", "));
                    problems.push(format!(
                        "line {n}: differs from the continuous line in {keys}"
                    ));
                }
            }
            None => problems.push(format!(
                "line {}: no continuous line at its height",
                index + 1
            )),
        }
    }
    problems
}

/// The store's tip and its problems: both modes must restore one tip, from the tip before the
/// kill (the genesis block for a new store) up to the last applied block, whose restart line
/// equals the continuous line.
fn judge_store(
    checked: &Checked,
    previous: Option<u32>,
    applied: Option<u32>,
    forward: &std::collections::BTreeMap<u32, Value>,
) -> (Option<u32>, Vec<String>) {
    let (low, mut problems, mut tips) = (previous.unwrap_or(1), Vec::new(), Vec::new());
    let high = applied.unwrap_or(low).max(low);
    for (mode, restored) in &checked.modes {
        let name = mode.name();
        let (tip, line) = match restored {
            Ok(restored) => restored,
            Err(message) => {
                problems.push(format!("{name}: {message}"));
                continue;
            }
        };
        tips.push(*tip);
        if !(low..=high).contains(tip) {
            problems.push(format!(
                "{name}: the store's tip {tip} is outside {low} to {high}"
            ));
        }
        match forward.get(tip) {
            Some(want) if differing_keys(line, want, &SAME_STATE).is_empty() => {}
            Some(_) => problems.push(format!("{name}: the restored state at {tip} differs")),
            None => problems.push(format!("{name}: no continuous line at {tip}")),
        }
    }
    if tips.windows(2).any(|pair| pair[0] != pair[1]) {
        problems.push(format!("the modes restored different tips: {tips:?}"));
    }
    (tips.first().copied(), problems)
}

fn case(line: u32, label: String, judged: &Judged) -> CaseResult {
    CaseResult {
        file: "runtime/kill".into(),
        line: usize::try_from(line).unwrap_or(0),
        op: "kill".into(),
        label,
        height: u64::from(judged.tip.unwrap_or(0)),
        outcome: match judged.problems.is_empty() {
            true => Outcome::Pass,
            false => Outcome::HarnessError(judged.problems.join("; ")),
        },
    }
}

/// SplitMix64, for the kills' delays.
fn split_mix(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::restart::HeartwoodMode;

    fn line(step: &str, h: u32, state: &str) -> Value {
        json!({"h": h, "step": step, "id": format!("id{h}"), "rw": 0, "b": state, "s": state,
               "r": state, "m": "m", "w": state, "sd": "sd", "cd": "cd"})
    }

    fn forward(tip: u32) -> BTreeMap<u32, Value> {
        (1..=tip)
            .map(|h| (h, line("apply", h, &format!("state{h}"))))
            .collect()
    }

    #[test]
    fn a_torn_last_line_is_left_out() {
        let dir = ScratchDir::new("runtime-lines").unwrap();
        let path = dir.path().join("digest.jsonl");
        std::fs::write(&path, "{\"h\":1,\"step\":\"restart\"}\n{\"h\":2,\"st").unwrap();
        let lines = read_lines(&path);
        assert_eq!(lines, [json!({"h": 1, "step": "restart"})]);
        assert!(read_lines(&dir.path().join("none")).is_empty());
    }

    #[test]
    fn a_relay_s_lines_must_start_at_the_stored_tip_and_follow_the_continuous_lines() {
        let forward = forward(10);
        let good = [line("restart", 4, "state4"), line("apply", 5, "state5")];
        assert!(judge_lines(&good, Some(4), &forward).is_empty());
        let problems = judge_lines(&good, Some(3), &forward);
        assert_eq!(problems.len(), 1, "{problems:?}");
        let wrong = [line("restart", 4, "state4"), line("apply", 5, "other")];
        assert!(judge_lines(&wrong, Some(4), &forward)[0].contains("differs"));
        let revert = [line("restart", 1, "state1"), line("revert", 1, "state1")];
        assert_eq!(judge_lines(&revert, None, &forward).len(), 1);
    }

    #[test]
    fn the_store_must_restore_one_committed_tip_up_to_the_last_applied_block() {
        let forward = forward(10);
        let restored = |tip: u32| Ok((tip, line("restart", tip, &format!("state{tip}"))));
        let both = |a: u32, b: u32| Checked {
            modes: vec![
                (HeartwoodMode::Table, restored(a)),
                (HeartwoodMode::Replay, restored(b)),
            ],
        };
        assert_eq!(
            judge_store(&both(6, 6), Some(4), Some(8), &forward),
            (Some(6), vec![])
        );
        let (_, problems) = judge_store(&both(3, 3), Some(4), Some(8), &forward);
        assert_eq!(
            problems.len(),
            2,
            "below the tip before the kill: {problems:?}"
        );
        let (_, problems) = judge_store(&both(9, 9), Some(4), Some(8), &forward);
        assert_eq!(
            problems.len(),
            2,
            "above the last applied block: {problems:?}"
        );
        let (_, problems) = judge_store(&both(6, 7), Some(4), Some(8), &forward);
        assert_eq!(problems, ["the modes restored different tips: [6, 7]"]);
        // A killed start leaves the tip before it.
        assert_eq!(
            judge_store(&both(4, 4), Some(4), None, &forward).1,
            Vec::<String>::new()
        );
    }

    /// The kill test on the committed chain, through the built relay and mock peer: a few kills.
    #[test]
    #[ignore = "integration"]
    fn the_committed_chain_survives_kills_of_the_relay() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let corpus = Corpus::load(&root.join("corpus/txgen-one-node")).unwrap();
        let exe = root.join("../../target/debug/difftest");
        let report = crash(&corpus, &exe, 6, 7, Duration::from_millis(1500)).unwrap();
        assert!(report.is_success(), "{report}");
    }
}
