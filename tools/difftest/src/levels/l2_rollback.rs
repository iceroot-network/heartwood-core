//! The rollback replay (`difftest run l2 DIR --rollback`): per corpus, the store at the tip built
//! once by the processor replay; for every rollback target of the class's list that the corpus
//! reaches, a copy of that store rolled back through `heartwood_blockchain::rollback`, its lines,
//! target and deleted rows compared with the oracle's `chain.rollback` on the same corpus; the
//! next start, which must take the replay path for want of a state and write the rows of the round
//! after the target as the oracle's restart saves them; then the blocks above the target imported
//! again, every line equal to the continuous line and the store's anchors holding. When the
//! rollback left no missed-slot row, the start writes the rows of the slots before block 2 as the
//! reference's start does (its summary gives their number, compared with the oracle's); they must
//! still be there after the re-sync, and the anchors, whose node never started with an empty
//! table, are read without them. A refusal must leave the store as it was, byte for byte.
//!
//! The rollbacks are those of the vector class (`tools/oracle/lib/vectors/rollback.js`): every
//! refusal, `--height` at the first, a middle and the last block of rounds 2 to 5, `--number` 1 and
//! 60, and both flags, each that the corpus's tip reaches. The oracle's side is a chain session on
//! the corpus's blocks (`chain.open` on its crypto files, then each block applied), one per
//! rollback, as a rollback changes its store; the refusals share one.
//!
//! The pieces the class's subject (`subjects::rollback`) and the synthetic rollback cases share are
//! here too: [`TipStore`] (a chain's store at its tip, copied for each rollback), [`roll_back`] and
//! [`start_after`] (Heartwood's side in the oracle's shape), [`oracle_open`] and
//! [`oracle_rollback`] (the oracle's side), and [`resync`] (the blocks imported again through the
//! download path and the job).

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use heartwood_blockchain::chain_state::ChainState;
use heartwood_blockchain::events::NoObserver;
use heartwood_blockchain::rollback;
use heartwood_blockchain::{
    BlockProcessorResult, ChainStore, Job, JobContext, JobEnd, PoolTask, ReplayReason, RestoreMode,
    RestorePath, run_job,
};
use heartwood_crypto::Milestones;
use heartwood_crypto::crypto::slots;
use heartwood_crypto::utils::hex;
use heartwood_database::{Database, Settings};
use heartwood_kernel::config::crypto::CryptoFiles;
use heartwood_kernel::{ManualClock, calculate_round};
use heartwood_pool::{DelegateVersions, Options as PoolOptions, Pool};
use heartwood_state::digest::{self, DigestLine, Step};
use heartwood_transactions::Handlers;
use serde_json::{Value, json};

use super::l2::{call, corpus_crypto};
use super::l2_processor::{Chunks, JobBlock, JobRig, Recorder, download, replay_through_jobs};
use crate::compare;
use crate::corpus::Corpus;
use crate::digests;
use crate::errors::{Error, Result};
use crate::oracle::{Oracle, OracleServer};
use crate::report::{CaseResult, Outcome, Report};
use crate::restart::SAME_STATE;
use crate::store::{self, ScratchDir, StartProblem, StoreNode, guarded};
use crate::store_summary;

/// The title of the level's reports.
const TITLE: &str = "L2 rollback replay: snapshot rollback, the start after it and the re-sync";

/// The keys a line of the re-sync shares with the continuous line of its height (the step and the
/// running digest aside).
const SAME_STEP: [&str; 9] = ["h", "id", "rw", "b", "s", "r", "m", "w", "sd"];

/// What a rollback replay covers.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RollbackOptions {
    /// The corpus's chain cut at this height, as the judge cut it.
    pub to: Option<u32>,
}

/// The flags of one rollback: `--height` and `--number`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Flags {
    /// `--height`.
    pub height: Option<i64>,
    /// `--number`.
    pub number: Option<i64>,
}

impl Flags {
    /// `--height h`.
    pub const fn height(height: i64) -> Flags {
        Flags {
            height: Some(height),
            number: None,
        }
    }

    /// `--number n`.
    pub const fn number(number: i64) -> Flags {
        Flags {
            height: None,
            number: Some(number),
        }
    }

    /// The flags as the command line gives them, for reports.
    pub fn name(&self) -> String {
        let mut words = Vec::new();
        if let Some(h) = self.height {
            words.push(format!("--height {h}"));
        }
        if let Some(n) = self.number {
            words.push(format!("--number {n}"));
        }
        if words.is_empty() {
            "no flag".into()
        } else {
            words.join(" ")
        }
    }

    /// The flags as `chain.rollback` takes them, beside `extra`.
    pub fn with(&self, mut extra: Value) -> Value {
        if let Some(h) = self.height {
            extra["height"] = json!(h);
        }
        if let Some(n) = self.number {
            extra["number"] = json!(n);
        }
        extra
    }
}

/// The class's refusals on a chain at `tip`, which change nothing.
pub fn refusals(tip: u32) -> Vec<Flags> {
    let tip = i64::from(tip);
    let both = |height, number| Flags {
        height: Some(height),
        number: Some(number),
    };
    let mut out = vec![
        Flags::default(),
        Flags::height(0),
        Flags::number(0),
        both(0, 0),
        Flags::height(-5),
        Flags::number(tip),
        Flags::number(-5),
        Flags::height(tip),
        Flags::height(tip + 35),
    ];
    out.extend([1, 40, 53].map(Flags::height));
    out
}

/// The class's rollbacks that a chain at `tip` reaches (the requested height in round 2 or above,
/// below the tip).
pub fn rollbacks(tip: u32) -> Vec<Flags> {
    let tip = i64::from(tip);
    let heights = [54, 60, 80, 106, 107, 133, 159, 160, 180, 212, 213, 239, 264];
    let mut out: Vec<Flags> = (heights.iter().filter(|&&h| h < tip))
        .map(|&h| Flags::height(h))
        .collect();
    let reaches = |number: i64| (54..tip).contains(&(tip - number));
    out.extend(
        [1, 60]
            .into_iter()
            .filter(|&n| reaches(n))
            .map(Flags::number),
    );
    if 133 < tip {
        out.push(Flags {
            height: Some(133),
            number: Some(60),
        });
    }
    if reaches(60) {
        out.push(Flags {
            height: Some(0),
            number: Some(60),
        });
    }
    out
}

/// A chain's store at its tip, closed, from which each rollback starts on a copy.
#[derive(Debug)]
pub struct TipStore {
    scratch: ScratchDir,
    /// The configuration.
    pub files: CryptoFiles,
    /// The tip's height.
    pub tip: u32,
    /// The running digest of the tip's line in the replay that built the store.
    pub chain_digest: [u8; 32],
}

impl TipStore {
    /// The chain of `corpus` applied once, block by block, from a first start (the job rig's
    /// [`JobRig::extend`]), as the oracle's `chain.open` applies it.
    pub fn applied(corpus: &Corpus) -> std::result::Result<TipStore, StartProblem> {
        let files = store::crypto_files(corpus)?;
        let mut rig = JobRig::start(&files)?;
        let blocks = (corpus.blocks.iter())
            .map(|b| heartwood_crypto::Block::receive(&b.bytes, &files.milestones))
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| StartProblem::Failed(format!("a block of the corpus: {e}")))?;
        rig.extend(&blocks)?;
        Ok(TipStore::of(rig, files))
    }

    /// The chain of `corpus` imported through the processor replay, its downloads, jobs and
    /// reverts, as a follower imports it; the replay's problems with their heights.
    pub fn processed(
        corpus: &Corpus,
    ) -> std::result::Result<(TipStore, Vec<(u32, String)>), StartProblem> {
        let files = store::crypto_files(corpus)?;
        let replay = replay_through_jobs(corpus, Chunks::Reference).map_err(|problem| {
            StartProblem::Failed(format!("the processor replay could not run: {problem:?}"))
        })?;
        Ok((TipStore::of(replay.rig, files), replay.problems))
    }

    fn of(rig: JobRig, files: CryptoFiles) -> TipStore {
        let parts = rig.into_parts();
        let tip = parts.state.height();
        // The store is closed here, so its file holds every commit.
        drop(parts.store);
        TipStore {
            scratch: parts.scratch,
            files,
            tip,
            chain_digest: parts.chain_digest,
        }
    }

    /// The store's database file.
    pub fn database_file(&self) -> PathBuf {
        Settings::in_data_dir(self.scratch.path()).path
    }

    /// A copy of the store's files in a new scratch directory.
    pub fn copy(&self) -> Result<ScratchDir> {
        let copy = ScratchDir::new("rollback")?;
        let (from, to) = (
            self.scratch.path().join("database"),
            copy.path().join("database"),
        );
        std::fs::create_dir_all(&to).map_err(|e| Error::io(&to, e))?;
        for entry in std::fs::read_dir(&from).map_err(|e| Error::io(&from, e))? {
            let entry = entry.map_err(|e| Error::io(&from, e))?;
            let target = to.join(entry.file_name());
            std::fs::copy(entry.path(), &target).map_err(|e| Error::io(&target, e))?;
        }
        Ok(copy)
    }

    /// The store's files, in memory, from which [`TipImage::store`] makes the store again.
    pub fn image(&self) -> Result<TipImage> {
        let dir = self.scratch.path().join("database");
        let mut files = Vec::new();
        for entry in std::fs::read_dir(&dir).map_err(|e| Error::io(&dir, e))? {
            let path = entry.map_err(|e| Error::io(&dir, e))?.path();
            let bytes = std::fs::read(&path).map_err(|e| Error::io(&path, e))?;
            files.push((
                path.file_name().map(ToOwned::to_owned).unwrap_or_default(),
                bytes,
            ));
        }
        Ok(TipImage {
            database: Arc::new(files),
            files: self.files.clone(),
            tip: self.tip,
            chain_digest: self.chain_digest,
        })
    }
}

/// A [`TipStore`]'s files kept in memory, so that a store built once serves many cases without a
/// directory that outlives them.
#[derive(Debug, Clone)]
pub struct TipImage {
    database: Arc<Vec<(std::ffi::OsString, Vec<u8>)>>,
    files: CryptoFiles,
    tip: u32,
    chain_digest: [u8; 32],
}

impl TipImage {
    /// The store again, in a new scratch directory.
    pub fn store(&self) -> Result<TipStore> {
        let scratch = ScratchDir::new("rollback-tip")?;
        let dir = scratch.path().join("database");
        std::fs::create_dir_all(&dir).map_err(|e| Error::io(&dir, e))?;
        for (name, bytes) in self.database.iter() {
            let path = dir.join(name);
            std::fs::write(&path, bytes).map_err(|e| Error::io(&path, e))?;
        }
        Ok(TipStore {
            scratch,
            files: self.files.clone(),
            tip: self.tip,
            chain_digest: self.chain_digest,
        })
    }
}

/// Heartwood's rollback of a copy of a store: the copy, the rollback in the shape of the oracle's
/// `chain.rollback` answer, and whether it rolled back (else it was refused).
#[derive(Debug)]
pub struct HeartwoodRollback {
    /// The rolled-back copy.
    pub scratch: ScratchDir,
    /// `{refused: {lines}}` or `{target, round, lines, deleted, summary}`.
    pub answer: Value,
    /// Whether the store was rolled back.
    pub rolled: bool,
}

/// `lines` as the oracle writes them: `[[level, text], ...]`.
fn lines_json<'l>(lines: impl IntoIterator<Item = (&'l str, &'l str)>) -> Value {
    Value::Array(lines.into_iter().map(|(l, t)| json!([l, t])).collect())
}

/// Heartwood's rollback of a copy of `tip` with `flags`, as `snapshot rollback` makes it.
pub fn roll_back(tip: &TipStore, flags: Flags) -> Result<HeartwoodRollback> {
    let scratch = tip.copy()?;
    let settings = Settings::in_data_dir(scratch.path());
    let failed = |e: &dyn std::fmt::Display| Error::Oracle(format!("Heartwood's store: {e}"));
    let answer = guarded(|| -> Result<(Value, bool)> {
        let mut store = ChainStore::open(&settings).map_err(|e| failed(&e))?;
        let last = store.database().last_block().map_err(|e| failed(&e))?;
        let current = last.map_or(0, |block| block.row.height);
        let milestones = &tip.files.milestones;
        let plan = match rollback::plan(flags.height, flags.number, current, milestones) {
            Ok(plan) => plan,
            Err(refusal) => {
                let lines = refusal.lines();
                let lines = lines
                    .iter()
                    .map(|(level, text)| (level.name(), text.as_str()));
                return Ok((json!({"refused": {"lines": lines_json(lines)}}), false));
            }
        };
        let rolled = rollback::rollback(&mut store, &plan, milestones).map_err(|e| failed(&e))?;
        drop(store);
        let lines = rollback::lines(&plan, &rolled);
        let answer = json!({
            "target": rolled.target,
            "round": rolled.round,
            "lines": lines_json(lines.iter().map(|text| ("info", text.as_str()))),
            "deleted": {
                "blocks": rolled.blocks,
                "transactions": rolled.transactions,
                "missed": rolled.missed,
                "rounds": rolled.round_rows,
            },
            "summary": store_summary::summary(scratch.path())?,
        });
        Ok((answer, true))
    })
    .map_err(|store::Unbuilt(m)| Error::Oracle(format!("not built yet: {m}")))??;
    Ok(HeartwoodRollback {
        scratch,
        answer: answer.0,
        rolled: answer.1,
    })
}

/// Heartwood's start after a rollback: the node, and its start in the shape of the oracle's
/// `chain.restart` answer (`{line, summary}`) and of the `active` rows of `chain.dump`'s `#R`.
#[derive(Debug)]
pub struct Started {
    /// The started node.
    pub node: StoreNode,
    /// How it restored its state.
    pub path: RestorePath,
    /// The restart's line, following the running digest the start was given.
    pub line: DigestLine,
    /// `{line, summary}`.
    pub restart: Value,
    /// The store's rows of the round after the tip, `[[key, balance], ...]` sorted by key.
    pub active: Value,
}

/// The store's rows of the round after `tip`, sorted by public key: `[[key, balance], ...]`.
pub fn active_rows(database: &Database, tip: u32, milestones: &Milestones) -> Result<Value> {
    let round = calculate_round(tip.saturating_add(1), milestones).round;
    let rows = database
        .round(round)
        .map_err(|e| Error::Oracle(format!("Heartwood's round rows: {e}")))?;
    let mut rows: Vec<(String, String)> = rows
        .into_iter()
        .map(|row| (row.public_key, row.balance.to_string()))
        .collect();
    rows.sort();
    Ok(Value::Array(
        rows.into_iter().map(|(k, b)| json!([k, b])).collect(),
    ))
}

/// Start the node of the rolled-back store in `scratch`, as a relay's start after the command;
/// its restart line follows `chain_digest`.
pub fn start_after(
    scratch: &ScratchDir,
    files: &CryptoFiles,
    chain_digest: &[u8; 32],
) -> std::result::Result<Started, StartProblem> {
    let node = StoreNode::start(scratch.path(), files, RestoreMode::Auto)?;
    let path = node.path.clone();
    let milestones = &files.milestones;
    let line = digest::height_digest(
        &node.state,
        milestones,
        Step::Restart,
        &[],
        None,
        chain_digest,
    );
    let failed = |e: Error| StartProblem::Failed(e.to_string());
    let summary = store_summary::summary(scratch.path()).map_err(failed)?;
    let active = active_rows(node.store.database(), node.state.height(), milestones);
    Ok(Started {
        restart: json!({"line": digests::line_value(&line), "summary": summary}),
        active: active.map_err(failed)?,
        node,
        path,
        line,
    })
}

/// The oracle's chain session on `corpus`'s blocks: `chain.open` on its crypto files, then every
/// block applied.
pub fn oracle_open(server: &mut OracleServer, corpus: &Corpus) -> Result<Value> {
    let opened = call(
        server,
        "chain.open",
        1,
        &json!({"crypto": corpus_crypto(corpus)?}),
    )?;
    let session = opened.get("session").cloned().unwrap_or(Value::Null);
    for block in &corpus.blocks {
        let args = json!({"session": session, "hex": hex::encode(&block.bytes)});
        let applied = call(server, "chain.apply", 1, &args)?;
        if applied.get("accepted") != Some(&Value::Bool(true)) {
            return Err(Error::Oracle(format!(
                "the oracle refused block {}: {applied}",
                block.height
            )));
        }
    }
    Ok(session)
}

/// The oracle's rollback of `session` with `flags`: `{rollback}`, and when it rolled back, the
/// start after it by `mode` (`chain.restart`) and the rows that start saved for the round after its
/// tip (`active` of `chain.dump`'s `#R`, sorted by public key): `{rollback, restart, active}`.
pub fn oracle_rollback(
    server: &mut OracleServer,
    session: &Value,
    flags: Flags,
    mode: &str,
) -> Result<Value> {
    let rollback = call(
        server,
        "chain.rollback",
        1,
        &flags.with(json!({"session": session})),
    )?;
    if rollback.get("refused").is_some() {
        return Ok(json!({ "rollback": rollback }));
    }
    let args = json!({"session": session, "mode": mode});
    let restart = call(server, "chain.restart", 1, &args)?;
    if restart.get("refused").is_some() {
        return Ok(json!({"rollback": rollback, "restart": restart}));
    }
    let dump = call(server, "chain.dump", 1, &json!({"session": session}))?;
    let round = dump["sections"]["R"].as_str().unwrap_or_default();
    let record: Value = round
        .lines()
        .nth(1)
        .and_then(|l| serde_json::from_str(l).ok())
        .unwrap_or(Value::Null);
    let mut active = record["active"].as_array().cloned().unwrap_or_default();
    active.sort_by(|a, b| a[0].as_str().cmp(&b[0].as_str()));
    Ok(json!({"rollback": rollback, "restart": restart, "active": active}))
}

/// The slot time of the block at `height` of `corpus`, in milliseconds since the Unix epoch.
fn slot_time_ms(block: &JobBlock, milestones: &Milestones) -> i64 {
    let epoch = slots::epoch_unix_ms(milestones.at(1).epoch()).unwrap_or(0);
    let timestamp = block.parts().map_or(0, |(header, _)| header.timestamp);
    epoch.saturating_add(i64::from(timestamp).saturating_mul(1000))
}

/// Import the blocks of `corpus` above the node's tip, up to the corpus's tip, into `node`
/// through the download path and the job, one block per job at its slot's time, as a follower
/// re-syncs; each block's line (step apply), or why a block was not accepted.
pub fn resync(node: StoreNode, corpus: &Corpus, scratch: &ScratchDir) -> Result<Resynced> {
    let files = node.files().clone();
    let milestones = &files.milestones;
    let (mut store, mut state, _) = node.into_parts();
    let pool_dir = scratch.path().join("pool");
    let options = PoolOptions::defaults(&|_| None, &pool_dir, 1);
    let pool = Pool::open(options, None).map_err(|e| Error::Oracle(format!("pool: {e}")))?;
    let mut pool = PoolTask::new(pool, DelegateVersions::new());
    let mut chain = ChainState::at(Arc::new(state.last_block().clone()));
    chain.reset_last_downloaded();
    chain.started = true;
    let chain = Mutex::new(chain);
    let clock = ManualClock::new(0);
    let (mut lines, mut previous) = (Vec::new(), [0; 32]);
    for height in state.height().saturating_add(1)..=corpus.tip() {
        let bytes = corpus.block(height).map(|b| b.bytes.clone());
        let block = JobBlock::of(bytes.ok_or_else(|| Error::Oracle(format!("no block {height}")))?);
        let params = milestones.at(state.height().saturating_add(1));
        let mut incoming = download(std::slice::from_ref(&block), params)
            .map_err(|e| Error::Oracle(format!("block {height}: {e}")))?;
        for b in &mut incoming {
            b.set_username(state.wallets());
        }
        let link = block.link().map_err(Error::Oracle)?;
        chain
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .last_downloaded = Some(link);
        clock.set(slot_time_ms(&block, milestones));
        let recorder = Recorder::new(Cow::Borrowed(milestones), previous, &[Step::Apply]);
        let outcome = {
            let mut observer = NoObserver;
            let mut ctx = JobContext {
                state: &mut state,
                store: &mut store,
                chain: &chain,
                pool: &mut pool,
                processor: &recorder,
                handlers: &Handlers,
                milestones,
                clock: &clock,
                observer: &mut observer,
                stall: None,
            };
            run_job(&mut ctx, Job { blocks: incoming })
                .map_err(|e| Error::Oracle(format!("the job of block {height} failed: {e}")))?
        };
        let seen = recorder.take_seen();
        let accepted = seen
            .first()
            .filter(|s| s.processed.result == BlockProcessorResult::Accepted);
        let Some(line) = accepted.and_then(|s| s.line.clone()) else {
            return Ok(Resynced {
                store,
                lines,
                refused: Some((
                    height,
                    format!("{:?}", seen.first().map(|s| s.processed.result)),
                )),
            });
        };
        previous = line.chain_digest;
        lines.push(line);
        if matches!(
            outcome.end,
            JobEnd::Clear | JobEnd::ForkCheck(_) | JobEnd::Fork(_)
        ) {
            chain
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .reset_last_downloaded();
        }
    }
    Ok(Resynced {
        store,
        lines,
        refused: None,
    })
}

/// A re-sync's outcome: the store, the lines of the blocks accepted, and the first block that was
/// not, with its result.
#[derive(Debug)]
pub struct Resynced {
    /// The store after it.
    pub store: ChainStore,
    /// The line of each accepted block.
    pub lines: Vec<DigestLine>,
    /// The first block not accepted, and what the processor answered.
    pub refused: Option<(u32, String)>,
}

/// Whether `ours` and `theirs` agree on `keys`: `Pass`, else the differences.
fn same_keys(theirs: &Value, ours: &Value, keys: &[&str]) -> Outcome {
    let pick = |line: &Value| {
        let map: serde_json::Map<String, Value> = keys
            .iter()
            .map(|k| {
                (
                    (*k).to_owned(),
                    line.get(*k).cloned().unwrap_or(Value::Null),
                )
            })
            .collect();
        Value::Object(map)
    };
    let (theirs, ours) = (pick(theirs), pick(ours));
    if theirs == ours {
        Outcome::Pass
    } else {
        Outcome::Mismatch(compare::diff(&theirs, &ours))
    }
}

/// `Pass` when `ours` equals `theirs`, else the differences.
fn equal(theirs: &Value, ours: &Value) -> Outcome {
    if theirs == ours {
        Outcome::Pass
    } else {
        Outcome::Mismatch(compare::diff(theirs, ours))
    }
}

/// The results of one corpus's replay, in order.
struct Results {
    file: String,
    results: Vec<CaseResult>,
}

impl Results {
    fn push(&mut self, label: String, height: u32, outcome: Outcome) {
        let line = self.results.len() + 1;
        self.results.push(CaseResult {
            file: self.file.clone(),
            line,
            op: "chain.rollback".into(),
            label,
            height: u64::from(height),
            outcome,
        });
    }
}

/// The rollback replay of `corpus` against `oracle`.
pub fn run(corpus: &Corpus, options: &RollbackOptions, oracle: &Oracle) -> Result<Report> {
    let corpus = &corpus.clone().as_judged(options.to)?;
    let name = corpus.name();
    let mut out = Results {
        file: format!("{name}/rollback"),
        results: Vec::new(),
    };
    let report = |out: Results| Report {
        title: TITLE.to_owned(),
        source: format!("snapshot rollback on {name}, cut at {}", corpus.tip()),
        files: 1,
        results: out.results,
    };
    let (tip, problems) = match TipStore::processed(corpus) {
        Ok(built) => built,
        Err(problem) => {
            let label = "the processor replay to the tip".to_owned();
            out.push(label, corpus.tip(), problem.outcome());
            return Ok(report(out));
        }
    };
    if let Some((height, why)) = problems.first() {
        let label = "the processor replay to the tip".to_owned();
        out.push(label, *height, Outcome::HarnessError(why.clone()));
        return Ok(report(out));
    }
    let forward = store::forward_lines(corpus)?;
    let mut server = oracle.server()?;
    let session = oracle_open(&mut server, corpus)?;
    let file = tip.database_file();
    let before = std::fs::read(&file).map_err(|e| Error::io(&file, e))?;
    for flags in refusals(tip.tip) {
        let theirs = oracle_rollback(&mut server, &session, flags, "rebuild")?;
        let ours = roll_back(&tip, flags)?;
        let label = format!("{}: refused as the reference refuses it", flags.name());
        let outcome = match (ours.rolled, theirs.get("restart")) {
            (false, None) => equal(&theirs["rollback"], &ours.answer),
            _ => Outcome::VerdictMismatch {
                oracle: theirs["rollback"].to_string(),
                heartwood: ours.answer.to_string(),
            },
        };
        out.push(label, tip.tip, outcome);
        let path = Settings::in_data_dir(ours.scratch.path()).path;
        let after = std::fs::read(&path).map_err(|e| Error::io(&path, e))?;
        let label = format!("{}: the store left byte for byte", flags.name());
        let outcome = if after == before {
            Outcome::Pass
        } else {
            Outcome::HarnessError("the refusal changed the store's file".into())
        };
        out.push(label, tip.tip, outcome);
    }
    call(&mut server, "chain.close", 1, &json!({"session": session}))?;
    for flags in rollbacks(tip.tip) {
        replay_one(corpus, &tip, &forward, flags, &mut server, &mut out)?;
    }
    Ok(report(out))
}

/// One rollback of the replay: the rollback on both sides, the start after it, and the re-sync.
fn replay_one(
    corpus: &Corpus,
    tip: &TipStore,
    forward: &BTreeMap<u32, Value>,
    flags: Flags,
    server: &mut OracleServer,
    out: &mut Results,
) -> Result<()> {
    let name = flags.name();
    let session = oracle_open(server, corpus)?;
    let theirs = oracle_rollback(server, &session, flags, "rebuild")?;
    call(server, "chain.close", 1, &json!({"session": session}))?;
    let ours = roll_back(tip, flags)?;
    let target = u32::try_from(theirs["rollback"]["target"].as_u64().unwrap_or(0)).unwrap_or(0);
    out.push(
        format!("{name}: the target, lines, deleted rows and summary"),
        target,
        equal(&theirs["rollback"], &ours.answer),
    );
    if !ours.rolled || theirs.get("active").is_none() {
        let why = format!("the oracle's start after it: {}", theirs["restart"]);
        out.push(
            format!("{name}: the start after it"),
            target,
            Outcome::HarnessError(why),
        );
        return Ok(());
    }
    let started = match start_after(&ours.scratch, &tip.files, &tip.chain_digest) {
        Ok(started) => started,
        Err(problem) => {
            out.push(
                format!("{name}: the start after it"),
                target,
                problem.outcome(),
            );
            return Ok(());
        }
    };
    let path = if started.path == RestorePath::Replay(ReplayReason::NoState) {
        Outcome::Pass
    } else {
        Outcome::HarnessError(format!("the start took the path {:?}", started.path))
    };
    out.push(
        format!("{name}: the start replays for want of a state"),
        target,
        path,
    );
    let line = &started.restart["line"];
    let judge = forward.get(&target).cloned().unwrap_or(Value::Null);
    let keys: Vec<&str> = ["h", "id"].into_iter().chain(SAME_STATE).collect();
    out.push(
        format!("{name}: the start's line at {target} is the judge's"),
        target,
        same_keys(&judge, line, &keys),
    );
    out.push(
        format!("{name}: the start's line is the oracle's restart line"),
        target,
        same_keys(&theirs["restart"]["line"], line, &keys),
    );
    let round = calculate_round(target.saturating_add(1), &tip.files.milestones).round;
    out.push(
        format!("{name}: round {round}'s rows after the start, as a set"),
        target,
        equal(&theirs["active"], &started.active),
    );
    out.push(
        format!("{name}: the store's summary after the start"),
        target,
        equal(&theirs["restart"]["summary"], &started.restart["summary"]),
    );
    // The rows the start wrote when the rollback left no missed row: the productivity backfill of
    // the slots before block 2, whose number the summary above compared with the oracle's start.
    // The node whose rows the anchors hold never started with an empty table, so it lacks them.
    let backfill = if ours.answer["summary"]["missed"] == 0 {
        let rows = started.node.store.database().missed_blocks();
        rows.map_err(|e| Error::Oracle(format!("Heartwood's missed rows: {e}")))?
    } else {
        Vec::new()
    };
    let resynced = resync(started.node, corpus, &ours.scratch)?;
    let mismatch = resynced.lines.iter().find_map(|line| {
        let line = digests::line_value(line);
        let height = u32::try_from(line["h"].as_u64().unwrap_or(0)).unwrap_or(0);
        let judge = forward.get(&height).cloned().unwrap_or(Value::Null);
        match same_keys(&judge, &line, &SAME_STEP) {
            Outcome::Pass => None,
            other => Some((height, other)),
        }
    });
    let resync_outcome = match (&resynced.refused, mismatch) {
        (Some((height, why)), _) => Outcome::HarnessError(format!("block {height}: {why}")),
        (None, Some((_, outcome))) => outcome,
        (None, None) => Outcome::Pass,
    };
    out.push(
        format!(
            "{name}: the re-sync of {} blocks, every line the judge's",
            resynced.lines.len()
        ),
        corpus.tip(),
        resync_outcome,
    );
    let round_length = u32::try_from(corpus.milestones.at(1).active_delegates()).unwrap_or(0);
    let last_round = store::last_round(corpus.tip(), round_length);
    match store::read_rows(resynced.store.database(), corpus.tip(), last_round) {
        Ok(mut rows) => {
            let kept = rows.missed.len();
            rows.missed.retain(|row| !backfill.contains(row));
            if !backfill.is_empty() {
                let label = format!(
                    "{name}: after the re-sync, the start's {} backfilled rows are still there",
                    backfill.len()
                );
                let outcome = if kept - rows.missed.len() == backfill.len() {
                    Outcome::Pass
                } else {
                    Outcome::HarnessError(format!("{} of them", kept - rows.missed.len()))
                };
                out.push(label, corpus.tip(), outcome);
            }
            for (label, outcome) in store::anchors(&corpus.dir, &rows, corpus.tip(), last_round) {
                out.push(
                    format!("{name}: after the re-sync, {label}"),
                    corpus.tip(),
                    outcome,
                );
            }
        }
        Err(why) => out.push(
            format!("{name}: the store's rows after the re-sync"),
            corpus.tip(),
            Outcome::HarnessError(why),
        ),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lists_are_the_classes_on_a_chain_of_265() {
        let names: Vec<String> = refusals(265).iter().map(Flags::name).collect();
        assert_eq!(names.len(), 12);
        assert_eq!(names[0], "no flag");
        assert!(names.contains(&"--height 300".to_owned()));
        let rolled: Vec<String> = rollbacks(265).iter().map(Flags::name).collect();
        assert_eq!(rolled.len(), 17);
        assert!(rolled.contains(&"--height 0 --number 60".to_owned()));
        // A shorter chain reaches fewer.
        let short = rollbacks(150);
        assert!(
            short
                .iter()
                .all(|f| f.height.is_none_or(|h| h == 0 || h < 150))
        );
        assert!(!short.contains(&Flags::height(160)));
    }
}
