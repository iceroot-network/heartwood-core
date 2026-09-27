//! A corpus replayed through Heartwood's store.
//!
//! The L2 replay commits every block with the store's chunk writer in a temporary database, and
//! checks the store anchors: the stored round rows, missed rows and block columns against the
//! reference node's own rows in the corpus. The kill test replays a corpus in a child process
//! that is killed at random moments, and checks that each restore lands on a committed tip whose
//! state digest equals the continuous replay's.
//!
//! The store anchors (the rows of the reference node's PostgreSQL database, which the corpus
//! holds in `reference/`):
//!
//! - rounds: each round's rows equal the node's, in the order of the reference's `getRound`
//!   (vote balance from the highest, then public key in byte order);
//! - missed rows: equal to the node's as multisets of (timestamp, height, username), the node's
//!   rows at the heights of blocks it refused or reverted left out (they are written before the
//!   block is judged, `reference/rejections.jsonl`);
//! - block columns: every block's `username`, `burned_fee` and `donations` equal the node's.
//!
//! The new crates may still be stubs in a tree where they are being written: [`guarded`] turns a
//! stub's `todo!()` into [`Unbuilt`], which a report lists as skipped rather than failed.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use heartwood_blockchain::{ChainStore, RestoreMode, RestorePath, restore};
use heartwood_crypto::Block;
use heartwood_database::{BlockRow, Database, MissedRow, RoundRow, Settings};
use heartwood_kernel::config::crypto::{self, CryptoFiles};
use heartwood_state::State;
use heartwood_state::digest::{self, Step};
use heartwood_transactions::Handlers;
use serde_json::{Value, json};

use crate::corpus::{Corpus, TS_DIGESTS};
use crate::digests;
use crate::errors::{Error, Result};
use crate::report::{CaseResult, Outcome, Report};
use crate::restart::{HeartwoodMode, SAME_STATE, differing_keys};

/// A directory under the system's temporary directory, removed when dropped.
#[derive(Debug)]
pub struct ScratchDir {
    path: PathBuf,
}

impl ScratchDir {
    /// A new empty directory whose name starts with `prefix`.
    pub fn new(prefix: &str) -> Result<ScratchDir> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("heartwood-{prefix}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).map_err(|e| Error::io(&path, e))?;
        Ok(ScratchDir { path })
    }

    /// The directory.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// A component that is still a stub: the text of its `todo!()`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unbuilt(pub String);

/// The start of the message of a `todo!()` panic.
const UNBUILT: &str = "not yet implemented";

/// Run `f`, a call into a component that may still be a stub: its `todo!()` becomes [`Unbuilt`];
/// any other panic goes on unwinding.
pub fn guarded<T>(f: impl FnOnce() -> T) -> std::result::Result<T, Unbuilt> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(value) => Ok(value),
        Err(payload) => {
            let message = payload
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| payload.downcast_ref::<&str>().map(|s| (*s).to_owned()))
                .unwrap_or_default();
            if message.starts_with(UNBUILT) {
                Err(Unbuilt(message))
            } else {
                std::panic::resume_unwind(payload)
            }
        }
    }
}

/// Whether the store can be opened in this tree (it is not a stub).
pub fn store_is_built() -> bool {
    let Ok(dir) = ScratchDir::new("store-probe") else {
        return false;
    };
    guarded(|| ChainStore::open(&Settings::in_data_dir(dir.path()))).is_ok()
}

/// The crypto files of `corpus`, read and checked by the kernel's loader, as a node reads them.
pub fn crypto_files(corpus: &Corpus) -> std::result::Result<CryptoFiles, StartProblem> {
    let dir = corpus.dir.join("crypto");
    guarded(|| crypto::load(&dir))?
        .map_err(|e| StartProblem::Failed(format!("the corpus's crypto files are refused: {e}")))
}

/// How a node's start went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartProblem {
    /// A component is a stub.
    Unbuilt(String),
    /// The start failed.
    Failed(String),
}

impl StartProblem {
    /// The outcome a report gives this problem.
    pub fn outcome(&self) -> Outcome {
        match self {
            StartProblem::Unbuilt(message) => Outcome::Skipped(format!("not built yet: {message}")),
            StartProblem::Failed(message) => Outcome::HarnessError(message.clone()),
        }
    }
}

impl StartProblem {
    /// The problem as the error of a command.
    pub fn into_error(self) -> Error {
        match self {
            StartProblem::Unbuilt(m) | StartProblem::Failed(m) => Error::Usage(m),
        }
    }
}

impl From<Unbuilt> for StartProblem {
    fn from(Unbuilt(message): Unbuilt) -> StartProblem {
        StartProblem::Unbuilt(message)
    }
}

/// A node's store under a data directory, with the state it restored.
#[derive(Debug)]
pub struct StoreNode {
    data: PathBuf,
    files: CryptoFiles,
    /// The store.
    pub store: ChainStore,
    /// The restored state.
    pub state: State,
    /// How the state was restored.
    pub path: RestorePath,
}

impl StoreNode {
    /// Open the store under `data` and restore the state by `mode`, as a node starts.
    pub fn start(
        data: &Path,
        files: &CryptoFiles,
        mode: RestoreMode,
    ) -> std::result::Result<StoreNode, StartProblem> {
        let settings = Settings::in_data_dir(data);
        let mut store = guarded(|| ChainStore::open(&settings))?
            .map_err(|e| StartProblem::Failed(format!("the store does not open: {e}")))?;
        let restored = guarded(|| restore(&mut store, files, &Handlers, mode))?
            .map_err(|e| StartProblem::Failed(format!("the start was refused: {e}")))?;
        Ok(StoreNode {
            data: data.to_path_buf(),
            files: files.clone(),
            store,
            state: restored.state,
            path: restored.path,
        })
    }

    /// Stop the node and start it again by `mode`.
    pub fn restart(self, mode: RestoreMode) -> std::result::Result<StoreNode, StartProblem> {
        let StoreNode {
            data, files, store, ..
        } = self;
        drop(store);
        StoreNode::start(&data, &files, mode)
    }

    /// The configuration.
    pub fn files(&self) -> &CryptoFiles {
        &self.files
    }

    /// The store, the state and how it was restored.
    pub fn into_parts(self) -> (ChainStore, State, RestorePath) {
        (self.store, self.state, self.path)
    }
}

/// The restore mode of a Heartwood restart mode, and whether `path` is the path it must take.
pub fn restore_mode(mode: HeartwoodMode) -> RestoreMode {
    match mode {
        HeartwoodMode::Table => RestoreMode::Auto,
        HeartwoodMode::Replay => RestoreMode::Replay,
    }
}

/// Whether a restart by `mode` took the path it must: the state tables for the table mode, the
/// requested replay for the replay mode.
pub fn took_its_path(mode: HeartwoodMode, path: &RestorePath) -> bool {
    use heartwood_blockchain::ReplayReason;
    match mode {
        HeartwoodMode::Table => matches!(path, RestorePath::Table),
        HeartwoodMode::Replay => matches!(path, RestorePath::Replay(ReplayReason::Requested)),
    }
}

/// The rows of a store that the anchors compare.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StoreRows {
    /// The round rows, by round, each in the order the store gives them.
    pub rounds: BTreeMap<u64, Vec<RoundRow>>,
    /// The missed rows.
    pub missed: Vec<MissedRow>,
    /// The block rows from height 2 to the tip.
    pub blocks: Vec<BlockRow>,
}

/// Read the rows of `database` for a chain at `tip` whose rows go up to round `last_round`.
pub fn read_rows(
    database: &Database,
    tip: u32,
    last_round: u64,
) -> std::result::Result<StoreRows, String> {
    let mut rows = StoreRows::default();
    for round in 1..=last_round {
        let r = database
            .round(round)
            .map_err(|e| format!("round {round}: {e}"))?;
        rows.rounds.insert(round, r);
    }
    rows.missed = database
        .missed_blocks()
        .map_err(|e| format!("missed rows: {e}"))?;
    if tip >= 2 {
        rows.blocks = database
            .blocks(2, tip)
            .map_err(|e| format!("blocks: {e}"))?
            .into_iter()
            .map(|stored| stored.row)
            .collect();
    }
    Ok(rows)
}

/// The last round a chain at `tip` has rows of: the tip's round, or the next one when the tip
/// ends a round.
pub fn last_round(tip: u32, round_length: u32) -> u64 {
    u64::from(tip) / u64::from(round_length.max(1)) + 1
}

fn read_reference(dir: &Path, name: &str) -> std::result::Result<Vec<Value>, String> {
    let path = dir.join("reference").join(name);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).map_err(|e| format!("{}: {e}", path.display())))
        .collect()
}

fn amount_text(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// The node's round rows of the corpus in `dir` up to `last_round`, per round in `getRound` order.
fn node_rounds(
    dir: &Path,
    last_round: u64,
) -> std::result::Result<BTreeMap<u64, Vec<(String, String)>>, String> {
    let mut out: BTreeMap<u64, Vec<(String, String)>> = BTreeMap::new();
    for row in read_reference(dir, "rounds.jsonl")? {
        let round = row.get("round").and_then(Value::as_u64).unwrap_or(0);
        if round == 0 || round > last_round {
            continue;
        }
        let key = row
            .get("publicKey")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        let balance = amount_text(row.get("balance")).unwrap_or_default();
        out.entry(round).or_default().push((key, balance));
    }
    for rows in out.values_mut() {
        rows.sort_by(|(ka, ba), (kb, bb)| {
            let a = ba.parse::<u128>().unwrap_or(0);
            let b = bb.parse::<u128>().unwrap_or(0);
            b.cmp(&a).then_with(|| ka.as_bytes().cmp(kb.as_bytes()))
        });
    }
    Ok(out)
}

/// The store anchors of the chain in `dir` cut at `tip`, with rows up to `last_round`: one
/// result per anchor, labelled.
pub fn anchors(dir: &Path, rows: &StoreRows, tip: u32, last_round: u64) -> Vec<(String, Outcome)> {
    let mut out = Vec::new();
    let failed = |message: String| Outcome::HarnessError(message);
    // Rounds, in getRound order.
    match node_rounds(dir, last_round) {
        Err(e) => out.push(("store: round rows".into(), failed(e))),
        Ok(node) => {
            let mut problems = Vec::new();
            let rounds: std::collections::BTreeSet<u64> =
                node.keys().chain(rows.rounds.keys()).copied().collect();
            for round in rounds {
                let theirs = node.get(&round).cloned().unwrap_or_default();
                let ours: Vec<(String, String)> = rows
                    .rounds
                    .get(&round)
                    .map(|r| {
                        r.iter()
                            .map(|row| (row.public_key.clone(), row.balance.to_string()))
                            .collect()
                    })
                    .unwrap_or_default();
                if theirs != ours {
                    let first = theirs
                        .iter()
                        .zip(&ours)
                        .position(|(a, b)| a != b)
                        .unwrap_or(theirs.len().min(ours.len()));
                    problems.push(format!(
                        "round {round}: the node has {} rows, the store {}; first difference at row {first}: node {:?}, store {:?}",
                        theirs.len(),
                        ours.len(),
                        theirs.get(first),
                        ours.get(first)
                    ));
                }
            }
            out.push((
                format!("store: round rows of rounds 1 to {last_round}"),
                if problems.is_empty() {
                    Outcome::Pass
                } else {
                    failed(problems.join("; "))
                },
            ));
        }
    }
    // Missed rows, as multisets, without the node's rows at heights it refused or reverted.
    match (
        read_reference(dir, "missed-blocks.jsonl"),
        read_reference(dir, "rejections.jsonl"),
    ) {
        (Ok(node), Ok(rejections)) => {
            let refused: std::collections::BTreeSet<u64> = rejections
                .iter()
                .filter_map(|r| r.get("height").and_then(Value::as_u64))
                .collect();
            let key = |t: i64, h: u64, u: &str| format!("{t}:{h}:{u}");
            let mut theirs: Vec<String> = Vec::new();
            let mut excluded = 0;
            for row in &node {
                let h = row.get("height").and_then(Value::as_u64).unwrap_or(0);
                if h > u64::from(tip) {
                    continue;
                }
                let k = key(
                    row.get("timestamp").and_then(Value::as_i64).unwrap_or(-1),
                    h,
                    row.get("username").and_then(Value::as_str).unwrap_or(""),
                );
                if refused.contains(&h) {
                    excluded += 1;
                    continue;
                }
                theirs.push(k);
            }
            let mut ours: Vec<String> = rows
                .missed
                .iter()
                .filter(|m| !refused.contains(&u64::from(m.height)))
                .map(|m| key(m.timestamp, u64::from(m.height), &m.username))
                .collect();
            theirs.sort();
            ours.sort();
            out.push((
                format!(
                    "store: missed rows ({} of the node's, {excluded} left out)",
                    theirs.len()
                ),
                if theirs == ours {
                    Outcome::Pass
                } else {
                    failed(format!(
                        "only the node: {:?}; only the store: {:?}",
                        theirs
                            .iter()
                            .filter(|k| !ours.contains(k))
                            .collect::<Vec<_>>(),
                        ours.iter()
                            .filter(|k| !theirs.contains(k))
                            .collect::<Vec<_>>()
                    ))
                },
            ));
        }
        (Err(e), _) | (_, Err(e)) => out.push(("store: missed rows".into(), failed(e))),
    }
    // Block columns.
    match read_reference(dir, "meta.jsonl") {
        Err(e) => out.push(("store: block columns".into(), failed(e))),
        Ok(meta) => {
            let by_height: BTreeMap<u64, &Value> = meta
                .iter()
                .filter_map(|m| m.get("h").and_then(Value::as_u64).map(|h| (h, m)))
                .collect();
            let mut problems = Vec::new();
            for row in &rows.blocks {
                let Some(node) = by_height.get(&u64::from(row.height)) else {
                    problems.push(format!("{}: the node has no columns", row.height));
                    continue;
                };
                let username = node.get("username").and_then(Value::as_str);
                let burned = amount_text(node.get("burnedFee"));
                let donations: BTreeMap<String, String> = node
                    .get("donations")
                    .and_then(Value::as_object)
                    .map(|o| {
                        o.iter()
                            .map(|(a, v)| (a.clone(), amount_text(Some(v)).unwrap_or_default()))
                            .collect()
                    })
                    .unwrap_or_default();
                let stored: BTreeMap<String, String> =
                    serde_json::from_str::<Value>(&row.donations)
                        .ok()
                        .and_then(|v| v.as_object().cloned())
                        .map(|o| {
                            o.iter()
                                .map(|(a, v)| (a.clone(), amount_text(Some(v)).unwrap_or_default()))
                                .collect()
                        })
                        .unwrap_or_default();
                if username != row.username.as_deref()
                    || burned.as_deref() != Some(row.burned_fee.to_string().as_str())
                    || donations != stored
                {
                    problems.push(format!(
                        "{}: node ({username:?}, {burned:?}, {donations:?}), store ({:?}, {}, {stored:?})",
                        row.height, row.username, row.burned_fee
                    ));
                }
            }
            if rows.blocks.len() != usize::try_from(tip.saturating_sub(1)).unwrap_or(0) {
                problems.push(format!(
                    "the store holds {} blocks above the genesis block, the chain {}",
                    rows.blocks.len(),
                    tip.saturating_sub(1)
                ));
            }
            out.push((
                format!("store: block columns of {} blocks", rows.blocks.len()),
                if problems.is_empty() {
                    Outcome::Pass
                } else {
                    failed(problems.into_iter().take(20).collect::<Vec<_>>().join("; "))
                },
            ));
        }
    }
    out
}

/// The line the replay of the kill test prints after each commit.
const COMMITTED: &str = "committed ";

/// `difftest store replay`: continue the replay of `corpus` through the store under `data`, one
/// block per chunk, up to `to` (the corpus's tip by default), printing `committed H` after each
/// commit. The store is restored first, as a node starts.
pub fn replay(corpus: &Corpus, data: &Path, to: Option<u32>) -> Result<u32> {
    let files = crypto_files(corpus).map_err(StartProblem::into_error)?;
    let mut node =
        StoreNode::start(data, &files, RestoreMode::Auto).map_err(StartProblem::into_error)?;
    let to = to.unwrap_or_else(|| corpus.tip()).min(corpus.tip());
    let milestones = node.files.milestones.clone();
    let mut stdout = std::io::stdout();
    let _ = writeln!(stdout, "{COMMITTED}{}", node.state.height());
    let _ = stdout.flush();
    while node.state.height() < to {
        let height = node.state.height() + 1;
        let block = corpus
            .block(height)
            .ok_or_else(|| Error::Usage(format!("the corpus has no block {height}")))?;
        let block = Block::receive(&block.bytes, &milestones)
            .map_err(|e| Error::Usage(format!("block {height}: {e}")))?;
        let applied = node
            .state
            .apply_block(&block, &milestones, &Handlers)
            .map_err(|e| Error::Usage(format!("block {height} was refused: {e}")))?;
        let mut writer = node
            .store
            .chunk()
            .map_err(|e| Error::Usage(format!("store: {e}")))?;
        writer
            .applied(&node.state, &block, &applied, &milestones)
            .map_err(|e| Error::Usage(format!("store: {e}")))?;
        writer
            .commit()
            .map_err(|e| Error::Usage(format!("store: {e}")))?;
        let _ = writeln!(stdout, "{COMMITTED}{height}");
        let _ = stdout.flush();
    }
    Ok(node.state.height())
}

/// The tip a restart restored and its restart line, or why it failed.
pub type RestoredTip = std::result::Result<(u32, Value), String>;

/// What a check of a store found: the stored tip, and the state each restart mode restored.
#[derive(Debug, Clone, PartialEq)]
pub struct Checked {
    /// Each mode, and what it restored.
    pub modes: Vec<(HeartwoodMode, RestoredTip)>,
}

/// `difftest store check`: restore the store under `data` by each mode, as a node starts, and
/// give each mode's tip and restart line.
pub fn check(corpus: &Corpus, data: &Path) -> Result<Checked> {
    let files = crypto_files(corpus).map_err(StartProblem::into_error)?;
    let mut modes = Vec::new();
    for mode in HeartwoodMode::ALL {
        let outcome = match StoreNode::start(data, &files, restore_mode(mode)) {
            Ok(node) => {
                let line = digest::height_digest(
                    &node.state,
                    &files.milestones,
                    Step::Restart,
                    &[],
                    None,
                    &[0; 32],
                );
                Ok((node.state.height(), digests::line_value(&line)))
            }
            Err(StartProblem::Unbuilt(m) | StartProblem::Failed(m)) => Err(m),
        };
        modes.push((mode, outcome));
    }
    Ok(Checked { modes })
}

/// SplitMix64, for the kill test's delays.
fn next(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// What one kill of the kill test found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KillResult {
    /// The last height the child reported as committed before the kill.
    pub last_reported: Option<u32>,
    /// The problems: a tip that is neither the last nor the next reported one, a state that
    /// differs from the continuous line of its tip, a mode that failed.
    pub problems: Vec<String>,
    /// The tip the store held.
    pub tip: Option<u32>,
}

/// Judge a check after a kill: each mode's tip must be `last_reported` or the next height, the
/// two modes must agree, and each restart line must equal the continuous line of its tip in `b`,
/// `s`, `r` and `w` (`forward` gives the continuous forward line of a height).
pub fn judge_kill(
    checked: &Checked,
    last_reported: Option<u32>,
    forward: &dyn Fn(u32) -> Option<Value>,
) -> KillResult {
    let mut problems = Vec::new();
    let mut tips = Vec::new();
    for (mode, outcome) in &checked.modes {
        match outcome {
            Err(message) => problems.push(format!("{}: {message}", mode.name())),
            Ok((tip, line)) => {
                tips.push(*tip);
                let allowed = match last_reported {
                    Some(last) => *tip == last || *tip == last + 1,
                    None => *tip <= 1,
                };
                if !allowed {
                    problems.push(format!(
                        "{}: the store's tip is {tip}, the child last reported {last_reported:?}",
                        mode.name()
                    ));
                }
                match forward(*tip) {
                    Some(want) => {
                        let differing = differing_keys(line, &want, &SAME_STATE);
                        if !differing.is_empty() {
                            problems.push(format!(
                                "{}: the restored state at {tip} differs from the continuous line in {}",
                                mode.name(),
                                differing.join(", ")
                            ));
                        }
                    }
                    None => problems.push(format!("{}: no continuous line at {tip}", mode.name())),
                }
            }
        }
    }
    if tips.windows(2).any(|w| w[0] != w[1]) {
        problems.push(format!("the modes restored different tips: {tips:?}"));
    }
    KillResult {
        last_reported,
        problems,
        tip: tips.first().copied(),
    }
}

/// The continuous forward lines of a corpus by height: the genesis line and each apply line of
/// the oracle's digest lines.
pub fn forward_lines(corpus: &Corpus) -> Result<BTreeMap<u32, Value>> {
    let lines = digests::read(&corpus.dir.join(TS_DIGESTS))?;
    let mut out = BTreeMap::new();
    for line in lines {
        let step = line.get("step").and_then(Value::as_str);
        if matches!(step, Some("genesis" | "apply")) {
            let h = line
                .get("h")
                .and_then(Value::as_u64)
                .and_then(|h| u32::try_from(h).ok())
                .unwrap_or(0);
            out.insert(h, line);
        }
    }
    Ok(out)
}

/// `difftest store crash`: replay `corpus` through a store in a child process (`difftest store
/// replay`, the binary `exe`), kill it with `SIGKILL` after a random delay, check the store by
/// both restore modes, and go on from the stored tip; a new store once the chain is replayed.
/// `kills` kills; the delays are drawn from `seed`, up to `max_delay`.
pub fn crash(
    corpus: &Corpus,
    exe: &Path,
    kills: u32,
    seed: u64,
    max_delay: Duration,
) -> Result<Report> {
    let forward = forward_lines(corpus)?;
    let title = "L2 kill test: the store after a SIGKILL".to_owned();
    if let Err(problem) = crypto_files(corpus).and_then(|files| {
        let dir =
            ScratchDir::new("crash-probe").map_err(|e| StartProblem::Failed(e.to_string()))?;
        StoreNode::start(dir.path(), &files, RestoreMode::Auto).map(|_| ())
    }) {
        return Ok(Report {
            title,
            source: format!("{kills} kills asked"),
            files: 1,
            results: vec![CaseResult {
                file: format!("{}/kill", corpus.name()),
                line: 0,
                op: "kill".into(),
                label: "the store's first start".into(),
                height: 0,
                outcome: problem.outcome(),
            }],
        });
    }
    let mut state = seed;
    let mut results = Vec::new();
    let mut scratch = ScratchDir::new("crash")?;
    let mut tips = BTreeMap::<u32, u32>::new();
    for kill in 1..=kills {
        let data = scratch.path().to_path_buf();
        let mut child = Command::new(exe)
            .args(["store", "replay"])
            .arg(&corpus.dir)
            .arg("--db")
            .arg(&data)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| Error::Usage(format!("cannot start {}: {e}", exe.display())))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| Error::Usage("the child has no output".into()))?;
        let (sender, receiver) = mpsc::channel::<u32>();
        let reader = std::thread::spawn(move || {
            for line in BufReader::new(stdout)
                .lines()
                .map_while(std::result::Result::ok)
            {
                if let Some(h) = line.strip_prefix(COMMITTED).and_then(|h| h.parse().ok()) {
                    let _ = sender.send(h);
                }
            }
        });
        let millis = u64::try_from(max_delay.as_millis())
            .unwrap_or(u64::MAX)
            .max(1);
        let delay = Duration::from_millis(next(&mut state) % millis);
        let started = Instant::now();
        let mut finished = false;
        while started.elapsed() < delay {
            if let Ok(Some(_)) = child.try_wait() {
                finished = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        if !finished {
            let _ = child.kill();
        }
        let _ = child.wait();
        let _ = reader.join();
        let last_reported = receiver.try_iter().last();
        let checked = check(corpus, &data)?;
        let judged = judge_kill(&checked, last_reported, &|h| forward.get(&h).cloned());
        if let Some(tip) = judged.tip {
            *tips.entry(tip).or_insert(0) += 1;
        }
        results.push(CaseResult {
            file: format!("{}/kill", corpus.name()),
            line: usize::try_from(kill).unwrap_or(0),
            op: "kill".into(),
            label: format!(
                "kill {kill} after {} ms: reported {last_reported:?}, stored tip {:?}",
                delay.as_millis(),
                judged.tip
            ),
            height: u64::from(judged.tip.unwrap_or(0)),
            outcome: if judged.problems.is_empty() {
                Outcome::Pass
            } else if judged.problems.iter().all(|p| p.contains(UNBUILT)) {
                Outcome::Skipped(format!("not built yet: {}", judged.problems.join("; ")))
            } else {
                Outcome::HarnessError(judged.problems.join("; "))
            },
        });
        if judged.tip.is_some_and(|t| t >= corpus.tip()) || finished {
            scratch = ScratchDir::new("crash")?;
        }
    }
    Ok(Report {
        title,
        source: format!(
            "{kills} kills, seed {seed}, delays up to {} ms; stored tips {}",
            max_delay.as_millis(),
            json!(tips)
        ),
        files: 1,
        results,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stub_is_reported_and_other_panics_unwind() {
        let stub: std::result::Result<u8, Unbuilt> = guarded(|| todo!("the store"));
        assert!(matches!(stub, Err(Unbuilt(m)) if m.contains("the store")));
        assert_eq!(guarded(|| 5), Ok(5));
        let other = std::panic::catch_unwind(|| guarded(|| -> u8 { panic!("boom") }));
        assert!(other.is_err());
    }

    #[test]
    fn scratch_directories_are_removed() {
        let path = {
            let dir = ScratchDir::new("unit").unwrap();
            std::fs::write(dir.path().join("x"), "1").unwrap();
            dir.path().to_path_buf()
        };
        assert!(!path.exists());
    }

    fn line(b: &str) -> Value {
        json!({"h": 5, "step": "restart", "b": b, "s": "s", "r": "r", "w": "w", "m": "x"})
    }

    #[test]
    fn a_kill_lands_on_the_last_or_the_next_committed_block() {
        let forward = |h: u32| (h == 5 || h == 6).then(|| line("b"));
        let both = |tip: u32, b: &str| Checked {
            modes: vec![
                (HeartwoodMode::Table, Ok((tip, line(b)))),
                (HeartwoodMode::Replay, Ok((tip, line(b)))),
            ],
        };
        assert!(
            judge_kill(&both(5, "b"), Some(5), &forward)
                .problems
                .is_empty()
        );
        assert!(
            judge_kill(&both(6, "b"), Some(5), &forward)
                .problems
                .is_empty()
        );
        let behind = judge_kill(&both(5, "b"), Some(6), &forward);
        assert_eq!(behind.problems.len(), 2, "{behind:?}");
        let wrong = judge_kill(&both(5, "other"), Some(5), &forward);
        assert!(
            wrong.problems.iter().all(|p| p.contains("differs")),
            "{wrong:?}"
        );
        let split = Checked {
            modes: vec![
                (HeartwoodMode::Table, Ok((5, line("b")))),
                (HeartwoodMode::Replay, Ok((6, line("b")))),
            ],
        };
        assert!(
            judge_kill(&split, Some(5), &forward)
                .problems
                .iter()
                .any(|p| p.contains("different tips"))
        );
        let failed = Checked {
            modes: vec![(HeartwoodMode::Table, Err("refused".into()))],
        };
        assert_eq!(judge_kill(&failed, Some(5), &forward).problems.len(), 1);
        // Killed before the first commit: an empty store restores the genesis block.
        let genesis = |h: u32| (h == 1).then(|| line("b"));
        assert!(
            judge_kill(&both(1, "b"), None, &genesis)
                .problems
                .is_empty()
        );
    }

    fn write(dir: &Path, name: &str, lines: &[Value]) {
        std::fs::create_dir_all(dir.join("reference")).unwrap();
        let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
        std::fs::write(dir.join("reference").join(name), text).unwrap();
    }

    fn block_row(height: u32, username: &str, burned: u64, donations: &str) -> BlockRow {
        BlockRow {
            id: heartwood_crypto::BlockId::from_array([u8::try_from(height).unwrap(); 32]),
            height,
            version: 0,
            timestamp: height * 8,
            previous_block: None,
            number_of_transactions: 0,
            total_amount: 0,
            total_fee: 0,
            burned_fee: burned,
            reward: 0,
            donations: donations.into(),
            payload_length: 0,
            payload_hash: String::new(),
            generator_public_key: String::new(),
            username: Some(username.into()),
            block_signature: String::new(),
        }
    }

    #[test]
    fn the_store_anchors_compare_rows_in_the_reference_order() {
        let dir = crate::vectors::testutil::TestDir::new("store-anchors");
        write(
            dir.path(),
            "rounds.jsonl",
            &[
                json!({"round": 1, "publicKey": "03aa", "balance": "5"}),
                json!({"round": 1, "publicKey": "02bb", "balance": "5"}),
                json!({"round": 1, "publicKey": "02cc", "balance": "9"}),
                json!({"round": 2, "publicKey": "02cc", "balance": "9"}),
            ],
        );
        write(
            dir.path(),
            "missed-blocks.jsonl",
            &[
                json!({"timestamp": 16, "height": 3, "username": "d1"}),
                json!({"timestamp": 12, "height": 2, "username": "d9"}),
                json!({"timestamp": 24, "height": 4, "username": "d2"}),
            ],
        );
        write(dir.path(), "rejections.jsonl", &[json!({"height": 2})]);
        write(
            dir.path(),
            "meta.jsonl",
            &[
                json!({"h": 2, "username": "d1", "burnedFee": "1", "donations": {"dA": "2"}}),
                json!({"h": 3, "username": "d2", "burnedFee": "0", "donations": {}}),
            ],
        );
        let row = |round: u64, key: &str, balance: u64| RoundRow {
            round,
            public_key: key.into(),
            balance,
        };
        let good = StoreRows {
            rounds: BTreeMap::from([
                (
                    1,
                    vec![row(1, "02cc", 9), row(1, "02bb", 5), row(1, "03aa", 5)],
                ),
                (2, vec![row(2, "02cc", 9)]),
            ]),
            missed: vec![MissedRow {
                timestamp: 16,
                height: 3,
                username: "d1".into(),
            }],
            blocks: vec![
                block_row(2, "d1", 1, "{\"dA\":\"2\"}"),
                block_row(3, "d2", 0, "{}"),
            ],
        };
        let results = anchors(dir.path(), &good, 3, 2);
        assert_eq!(results.len(), 3);
        assert!(
            results.iter().all(|(_, o)| *o == Outcome::Pass),
            "{results:?}"
        );
        assert!(results[1].0.contains("1 left out"), "{results:?}");

        let mut swapped = good.clone();
        swapped.rounds.get_mut(&1).unwrap().swap(1, 2);
        assert!(anchors(dir.path(), &swapped, 3, 2)[0].1.is_failure());
        let mut missing = good.clone();
        missing.missed.clear();
        assert!(anchors(dir.path(), &missing, 3, 2)[1].1.is_failure());
        let mut burned = good.clone();
        burned.blocks[1].burned_fee = 7;
        assert!(anchors(dir.path(), &burned, 3, 2)[2].1.is_failure());
        let mut short = good;
        short.blocks.pop();
        assert!(anchors(dir.path(), &short, 3, 2)[2].1.is_failure());
    }

    #[test]
    fn the_last_round_of_a_tip() {
        assert_eq!(last_round(1, 53), 1);
        assert_eq!(last_round(52, 53), 1);
        assert_eq!(last_round(53, 53), 2);
        assert_eq!(last_round(54, 53), 2);
        assert_eq!(last_round(265, 53), 6);
    }
}
