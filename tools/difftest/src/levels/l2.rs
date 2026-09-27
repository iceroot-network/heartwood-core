//! L2: a corpus chain replayed block by block in Heartwood, with a state digest line after every
//! step compared with the oracle's line of the same step.
//!
//! The replay decodes every block as a node receives it, checks the scheduled generator and the
//! reward before applying it, runs the supply check after every step, and follows the corpus's
//! revert plan. It also checks, after every step, the one value of Heartwood's state that the
//! digest leaves out: each delegate's `resigned_at` must be the height of its latest delegate
//! resignation in the chain so far, which is what the reference looks up in its history and
//! what a restart has to rebuild. At the first difference it shows both sides' records of that
//! step.
//!
//! The steps of a replay are [`actions`]: the genesis step, then for every block an apply, and
//! after a block that the revert plan names, its reverts (one line each) and the re-applies of
//! the reverted blocks. The oracle's judge (`replay-chain.js`) walks the same steps, so line `i`
//! of both files describes the same state.
//!
//! The replay writes every step through Heartwood's store, as a node does: the state after the
//! first start comes from the store, and each applied or reverted block is committed with the
//! chunk writer, one block per chunk (and in a second run, chunks of 100 blocks). At the end the
//! store anchors ([`crate::store`]) compare the stored rows with the reference node's. With
//! restarts ([`L2Options::restart`]), the chain is replayed once per Heartwood restart mode with
//! the restarts of the corpus's restart plan ([`crate::restart`]): each restart closes the store,
//! opens it again and restores the state by that mode, and its line is compared with the
//! reference's restart lines and with the continuous line of its height; every later line is
//! compared with the continuous line of its step.

use std::collections::BTreeMap;
use std::path::Path;

use heartwood_blockchain::{ChainStore, RestoreMode, RestorePath};
use heartwood_crypto::identities::Address;
use heartwood_crypto::{Block, Milestones, TransactionType};
use heartwood_kernel::config::crypto::CryptoFiles;
use heartwood_kernel::utils::calculate_forging_info::calculate_forging_info;
use heartwood_state::State;
use heartwood_state::digest::{self, DigestLine, Sections, Step};
use heartwood_state::machine::{Applied, Reverted};
use heartwood_state::round_state::MissedBlock;
use heartwood_state::supply::check_supply;
use heartwood_transactions::Handlers;
use serde_json::{Value, json};

use crate::compare;
use crate::corpus::{ANCHORS, Corpus, TS_DIGESTS};
use crate::digests::{self, sections_from_json};
use crate::errors::{Error, Result};
use crate::oracle::OracleServer;
use crate::report::{CaseResult, Outcome, Report};
use crate::restart::{
    self, HeartwoodMode, RestartOutcome, RestartPlan, RestartRecord, SAME_STATE, SAME_STEP,
    differing_keys,
};
use crate::store::{self, ScratchDir, StartProblem, StoreNode};
use crate::subject::Documented;

/// What an L2 run covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct L2Options {
    /// Also run the synthetic cases on top of the chain (needs the oracle).
    pub synthetic: bool,
    /// Also replay the chain with the restarts of the corpus's restart plan.
    pub restart: bool,
    /// Write every step through Heartwood's store, with the store anchors.
    pub store: bool,
    /// Replay the chain cut at this height (the judge must have cut it there).
    pub to: Option<u32>,
}

impl Default for L2Options {
    fn default() -> L2Options {
        L2Options {
            synthetic: false,
            restart: false,
            store: true,
            to: None,
        }
    }
}

/// One step of a replay after the genesis step; each gives one digest line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Apply the block at this height for the first time.
    Apply(u32),
    /// Revert the last block.
    Revert,
    /// Apply again the block at this height, which a revert took back.
    Reapply(u32),
}

/// The steps of a replay of `corpus` after the genesis step, in order.
pub fn actions(corpus: &Corpus) -> Vec<Action> {
    plan_actions(corpus.blocks.iter().map(|b| b.height), &corpus.revert_plan)
}

/// The steps for blocks at `heights`, each followed by the reverts and re-applies that `plan`
/// runs after it.
pub fn plan_actions(
    heights: impl IntoIterator<Item = u32>,
    plan: &crate::corpus::RevertPlan,
) -> Vec<Action> {
    let mut out = Vec::new();
    for height in heights {
        out.push(Action::Apply(height));
        for step in plan.after(height) {
            out.extend(std::iter::repeat_n(Action::Revert, step.blocks as usize));
            let first = height - step.blocks + 1;
            out.extend((first..=height).map(Action::Reapply));
        }
    }
    out
}

/// A problem of the Heartwood side that is not a digest difference: a refused block, a schedule
/// or reward mismatch, a failed supply check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    /// The digest line the step writes (from 1: line 1 is the genesis step).
    pub line: usize,
    /// The block height of the step.
    pub height: u32,
    /// What went wrong.
    pub message: String,
}

/// Heartwood's replay of a corpus: its digest lines, and the problems met.
#[derive(Debug, Clone, Default)]
pub struct Replay {
    /// One line per step, the genesis step first.
    pub lines: Vec<DigestLine>,
    /// The problems, in step order.
    pub problems: Vec<Problem>,
    /// The records of the last step, when the replay was asked to stop at a line.
    pub records: Option<Sections>,
}

fn decode(corpus: &Corpus, height: u32) -> std::result::Result<Block, String> {
    let block = corpus
        .block(height)
        .ok_or_else(|| format!("the corpus has no block {height}"))?;
    let decoded =
        Block::receive(&block.bytes, &corpus.milestones).map_err(|e| format!("decode: {e}"))?;
    if decoded.id() != &block.id {
        return Err(format!(
            "decodes to id {}, the corpus says {}",
            decoded.id(),
            block.id
        ));
    }
    Ok(decoded)
}

/// The heights of the delegate resignations (1/7, of any kind) of each sender in the applied
/// chain, oldest first.
#[derive(Debug, Clone, Default)]
struct Resignations(BTreeMap<Address, Vec<u32>>);

impl Resignations {
    /// Add the resignations of `block`.
    fn apply(&mut self, block: &Block, network: u8) {
        for transaction in block.transactions() {
            if transaction.kind() == TransactionType::DelegateResignation {
                let sender =
                    Address::from_public_key(&transaction.data().sender_public_key, network);
                self.0.entry(sender).or_default().push(block.height());
            }
        }
    }

    /// Drop the resignations above `height`, the last block's height after a revert.
    fn revert_to(&mut self, height: u32) {
        for heights in self.0.values_mut() {
            while heights.last().is_some_and(|h| *h > height) {
                heights.pop();
            }
        }
    }

    /// The delegates of `state` whose `resigned_at` is not the height of their latest
    /// resignation.
    fn differences(&self, state: &State) -> Vec<String> {
        state
            .wallets()
            .delegates()
            .filter_map(|wallet| {
                let delegate = wallet.delegate()?;
                let latest = self.0.get(wallet.address()).and_then(|h| h.last().copied());
                (delegate.resigned_at != latest).then(|| {
                    format!(
                        "{}'s resigned_at is {:?}, its latest resignation is at {latest:?}",
                        delegate.username(),
                        delegate.resigned_at
                    )
                })
            })
            .collect()
    }
}

/// A step's writes, kept until the chunk commits: the state after the step with what the state
/// machine returned.
#[derive(Debug)]
enum Pending {
    Applied {
        state: State,
        block: Block,
        applied: Applied,
    },
    Reverted {
        state: State,
        reverted: Reverted,
    },
}

/// The store a replay writes through.
#[derive(Debug)]
struct Backing {
    scratch: ScratchDir,
    files: CryptoFiles,
    store: ChainStore,
    chunk: usize,
    pending: Vec<Pending>,
}

impl Backing {
    /// Commit the pending steps as one chunk.
    fn flush(&mut self, milestones: &Milestones) -> std::result::Result<(), String> {
        if self.pending.is_empty() {
            return Ok(());
        }
        let pending = std::mem::take(&mut self.pending);
        let written = store::guarded(|| -> std::result::Result<(), String> {
            let mut writer = self.store.chunk().map_err(|e| e.to_string())?;
            for step in &pending {
                match step {
                    Pending::Applied {
                        state,
                        block,
                        applied,
                    } => writer
                        .applied(state, block, applied, milestones)
                        .map_err(|e| format!("block {}: {e}", block.height()))?,
                    Pending::Reverted { state, reverted } => writer
                        .reverted(state, reverted, milestones)
                        .map_err(|e| format!("the revert of {}: {e}", reverted.block.height()))?,
                }
            }
            writer.commit().map_err(|e| e.to_string())
        });
        match written {
            Ok(result) => result.map_err(|e| format!("the store refused a chunk: {e}")),
            Err(store::Unbuilt(message)) => Err(message),
        }
    }
}

/// A replay of a corpus's steps in Heartwood: the checks of every step, one digest line per step,
/// and, when it has a store, the writes of every step.
#[derive(Debug)]
pub struct Walker<'c> {
    corpus: &'c Corpus,
    state: State,
    resignations: Resignations,
    chain: [u8; 32],
    stop_at: Option<usize>,
    backing: Option<Backing>,
    /// The replay's lines and problems.
    pub replay: Replay,
}

impl<'c> Walker<'c> {
    /// A replay of `corpus` in memory, from the state after its genesis block; the problem when
    /// the genesis block is refused.
    pub fn in_memory(
        corpus: &'c Corpus,
        stop_at: Option<usize>,
    ) -> std::result::Result<Walker<'c>, Problem> {
        match State::from_genesis(&corpus.genesis, &corpus.milestones, &Handlers) {
            Ok(state) => Ok(Walker::with_state(corpus, state, stop_at, None)),
            Err(e) => Err(Problem {
                line: 1,
                height: 1,
                message: format!("the genesis block was refused: {e}"),
            }),
        }
    }

    /// A replay of `corpus` through a new store in a temporary directory, committing every
    /// `chunk` steps; the first state is the store's first start.
    pub fn with_store(
        corpus: &'c Corpus,
        chunk: usize,
    ) -> std::result::Result<Walker<'c>, StartProblem> {
        let files = store::crypto_files(corpus)?;
        let scratch = ScratchDir::new("l2-store")
            .map_err(|e| StartProblem::Failed(format!("no scratch directory: {e}")))?;
        let node = StoreNode::start(scratch.path(), &files, RestoreMode::Auto)?;
        let (store, state, path) = node.into_parts();
        if path != RestorePath::FirstStart {
            return Err(StartProblem::Failed(format!(
                "a new store started by {path:?}, not as a first start"
            )));
        }
        Ok(Walker::with_state(
            corpus,
            state,
            None,
            Some(Backing {
                scratch,
                files,
                store,
                chunk: chunk.max(1),
                pending: Vec::new(),
            }),
        ))
    }

    fn with_state(
        corpus: &'c Corpus,
        state: State,
        stop_at: Option<usize>,
        backing: Option<Backing>,
    ) -> Walker<'c> {
        let mut resignations = Resignations::default();
        resignations.apply(&corpus.genesis, corpus.network.pub_key_hash);
        let mut walker = Walker {
            corpus,
            state,
            resignations,
            chain: [0; 32],
            stop_at,
            backing,
            replay: Replay::default(),
        };
        walker.push(Step::Genesis, &[], None);
        walker
    }

    /// The state.
    pub fn state(&self) -> &State {
        &self.state
    }

    fn problem(&mut self, line: usize, height: u32, message: String) {
        self.replay.problems.push(Problem {
            line,
            height,
            message,
        });
    }

    fn push(&mut self, step: Step, missed: &[MissedBlock], reward: Option<u64>) {
        let milestones = &self.corpus.milestones;
        let line =
            digest::height_digest(&self.state, milestones, step, missed, reward, &self.chain);
        self.chain = line.chain_digest;
        self.replay.lines.push(line);
        let at = self.replay.lines.len();
        let height = self.state.height();
        if let Err(e) = check_supply(&self.state) {
            self.problem(at, height, format!("supply check: {e}"));
        }
        for message in self.resignations.differences(&self.state) {
            self.problem(at, height, message);
        }
        if self.stop_at == Some(at) {
            self.replay.records = Some(digest::records(&self.state, milestones, missed));
        }
    }

    /// Whether the replay has reached the line it stops at.
    pub fn stopped(&self) -> bool {
        self.stop_at
            .is_some_and(|stop| self.replay.lines.len() >= stop)
    }

    /// Run one step; `false` when the replay cannot go on.
    pub fn step(&mut self, action: Action) -> bool {
        match action {
            Action::Revert => self.revert(),
            Action::Apply(height) => self.apply(height, Step::Apply),
            Action::Reapply(height) => self.apply(height, Step::Reapply),
        }
    }

    fn keep(&mut self, pending: Pending) -> bool {
        let line = self.replay.lines.len();
        let height = self.state.height();
        let milestones = &self.corpus.milestones;
        let Some(backing) = &mut self.backing else {
            return true;
        };
        backing.pending.push(pending);
        if backing.pending.len() < backing.chunk {
            return true;
        }
        match backing.flush(milestones) {
            Ok(()) => true,
            Err(message) => {
                self.problem(line, height, message);
                false
            }
        }
    }

    /// Revert the last block.
    pub fn revert(&mut self) -> bool {
        let line = self.replay.lines.len() + 1;
        match self.state.revert_block(&self.corpus.milestones, &Handlers) {
            Ok(reverted) => {
                self.resignations.revert_to(self.state.height());
                self.push(Step::Revert, &[], None);
                let state = self.state.clone();
                self.keep(Pending::Reverted { state, reverted })
            }
            Err(e) => {
                let height = self.state.height();
                self.problem(line, height, format!("revert refused: {e}"));
                false
            }
        }
    }

    /// Apply the corpus's block at `height` as `step`, after the checks of its schedule and
    /// reward.
    pub fn apply(&mut self, height: u32, step: Step) -> bool {
        let line = self.replay.lines.len() + 1;
        let milestones = &self.corpus.milestones;
        let block = match decode(self.corpus, height) {
            Ok(block) => block,
            Err(message) => {
                self.problem(line, height, message);
                return false;
            }
        };
        let header = block.header();
        let generator = header.generator_public_key;
        let info = calculate_forging_info(i64::from(header.timestamp), height, milestones);
        let scheduled = usize::try_from(info.current_forger)
            .ok()
            .and_then(|i| self.state.forging_delegates().get(i));
        if scheduled.map(|d| d.public_key) != Some(generator) {
            let message = format!(
                "the schedule names {} for the slot, the block's generator is {}",
                scheduled.map_or_else(|| "nobody".into(), |d| d.username.clone()),
                generator.to_hex()
            );
            self.problem(line, height, message);
        }
        let reward = match self
            .state
            .reward_for_block_in_round(height, &generator, milestones)
        {
            Ok(r) => {
                if r.reward != header.reward {
                    let message = format!(
                        "the reward in its round is {}, the header says {}",
                        r.reward, header.reward
                    );
                    self.problem(line, height, message);
                }
                Some(r.reward)
            }
            Err(e) => {
                self.problem(line, height, format!("no reward for the generator: {e}"));
                None
            }
        };
        match self.state.apply_block(&block, milestones, &Handlers) {
            Ok(applied) => {
                self.resignations
                    .apply(&block, self.corpus.network.pub_key_hash);
                self.push(step, &applied.missed, reward);
                let state = self.state.clone();
                self.keep(Pending::Applied {
                    state,
                    block,
                    applied,
                })
            }
            Err(e) => {
                self.problem(line, height, format!("refused: {e}"));
                false
            }
        }
    }

    /// Commit what is pending; `false` when the store refused it.
    pub fn flush(&mut self) -> bool {
        let line = self.replay.lines.len();
        let height = self.state.height();
        let Some(backing) = &mut self.backing else {
            return true;
        };
        match backing.flush(&self.corpus.milestones) {
            Ok(()) => true,
            Err(message) => {
                self.problem(line, height, message);
                false
            }
        }
    }

    /// Stop the node and start it again by `mode`: the store is closed and opened again and the
    /// state restored from it; the restart's line is pushed. Returns the path the start took.
    pub fn restart(
        &mut self,
        mode: HeartwoodMode,
    ) -> std::result::Result<RestorePath, StartProblem> {
        if !self.flush() {
            return Err(StartProblem::Failed("the store refused a chunk".into()));
        }
        let backing = self
            .backing
            .take()
            .ok_or_else(|| StartProblem::Failed("a replay in memory cannot restart".into()))?;
        let Backing {
            scratch,
            files,
            store,
            chunk,
            ..
        } = backing;
        drop(store);
        let node = StoreNode::start(scratch.path(), &files, store::restore_mode(mode))?;
        let (store, state, path) = node.into_parts();
        self.state = state;
        self.backing = Some(Backing {
            scratch,
            files,
            store,
            chunk,
            pending: Vec::new(),
        });
        self.push(Step::Restart, &[], None);
        Ok(path)
    }

    /// The store's rows for the anchors, at the tip.
    pub fn store_rows(&mut self) -> Option<std::result::Result<store::StoreRows, String>> {
        if !self.flush() {
            return Some(Err("the store refused a chunk".into()));
        }
        let tip = self.state.height();
        let round_length = self.corpus.revert_plan.round_length;
        let backing = self.backing.as_ref()?;
        let last_round = store::last_round(tip, round_length);
        Some(
            store::guarded(|| store::read_rows(backing.store.database(), tip, last_round))
                .unwrap_or_else(|store::Unbuilt(m)| Err(m)),
        )
    }
}

/// Replay `corpus` in Heartwood in memory, up to and including line `stop_at` (all lines when
/// `None`), keeping the records of that line.
pub fn replay(corpus: &Corpus, stop_at: Option<usize>) -> Replay {
    let mut walker = match Walker::in_memory(corpus, stop_at) {
        Ok(walker) => walker,
        Err(problem) => {
            return Replay {
                problems: vec![problem],
                ..Replay::default()
            };
        }
    };
    for action in actions(corpus) {
        if walker.stopped() || !walker.step(action) {
            break;
        }
    }
    walker.replay
}

/// Replay `corpus` through a new store that commits every `chunk` steps; the walker is returned
/// for its store's rows.
pub fn replay_through_store(
    corpus: &Corpus,
    chunk: usize,
) -> std::result::Result<Walker<'_>, StartProblem> {
    let mut walker = Walker::with_store(corpus, chunk)?;
    for action in actions(corpus) {
        if !walker.step(action) {
            break;
        }
    }
    walker.flush();
    Ok(walker)
}

/// The judge's anchor results of a corpus, and whether they hold.
pub fn judge_report(dir: &Path) -> Result<(bool, Value)> {
    let path = dir.join(ANCHORS);
    let text = std::fs::read_to_string(&path).map_err(|e| Error::io(&path, e))?;
    let value: Value = serde_json::from_str(&text).map_err(|e| Error::Corpus {
        path: path.clone(),
        message: e.to_string(),
    })?;
    Ok((value.get("ok") == Some(&Value::Bool(true)), value))
}

fn describe_judge(judge: &Value) -> String {
    let short = |v: Option<&Value>| {
        v.and_then(Value::as_str)
            .map(|s| s.chars().take(12).collect::<String>())
            .unwrap_or_else(|| "unknown".into())
    };
    let cut = judge
        .get("to")
        .and_then(Value::as_u64)
        .map(|to| format!(", cut at {to}"))
        .unwrap_or_default();
    format!(
        "judge: reference commit {} (tree {}), oracle {} {}, {} lines{cut}",
        short(judge.pointer("/reference/commit")),
        short(judge.pointer("/reference/tree")),
        judge
            .pointer("/oracle/mode")
            .and_then(Value::as_str)
            .unwrap_or("?"),
        short(judge.pointer("/oracle/image")),
        judge.get("lines").and_then(Value::as_u64).unwrap_or(0)
    )
}

fn step_label(line: &Value) -> String {
    format!(
        "{} {}",
        line.get("step").and_then(Value::as_str).unwrap_or("?"),
        line.get("h").and_then(Value::as_u64).unwrap_or(0)
    )
}

/// The oracle's records at digest line `line`: a session of the oracle walks the corpus's steps
/// up to that line and dumps them.
pub fn oracle_records(server: &mut OracleServer, corpus: &Corpus, line: usize) -> Result<Sections> {
    let crypto = corpus_crypto(corpus)?;
    let open = call(server, "chain.open", 1, &json!({"crypto": crypto}))?;
    let session = open.get("session").cloned().unwrap_or(Value::Null);
    for (done, action) in (1..).zip(actions(corpus)) {
        if done >= line {
            break;
        }
        match action {
            Action::Revert => {
                call(
                    server,
                    "chain.revert",
                    1,
                    &json!({"session": session, "count": 1}),
                )?;
            }
            Action::Apply(h) | Action::Reapply(h) => {
                let bytes = corpus
                    .block(h)
                    .map(|b| heartwood_crypto::utils::hex::encode(&b.bytes));
                let step = if matches!(action, Action::Apply(_)) {
                    "apply"
                } else {
                    "reapply"
                };
                call(
                    server,
                    "chain.apply",
                    1,
                    &json!({"session": session, "hex": bytes, "step": step}),
                )?;
            }
        }
    }
    let dump = call(server, "chain.dump", 1, &json!({"session": session}))?;
    call(server, "chain.close", 1, &json!({"session": session}))?;
    dump.get("sections")
        .and_then(sections_from_json)
        .ok_or_else(|| Error::Oracle("chain.dump sent no sections".into()))
}

/// The chain definition files of a corpus, as `chain.open` takes them.
pub fn corpus_crypto(corpus: &Corpus) -> Result<Value> {
    let mut crypto = serde_json::Map::new();
    for name in crate::corpus::CRYPTO_FILES {
        let path = corpus.dir.join("crypto").join(format!("{name}.json"));
        let text = std::fs::read_to_string(&path).map_err(|e| Error::io(&path, e))?;
        let value: Value = serde_json::from_str(&text).map_err(|e| Error::Corpus {
            path: path.clone(),
            message: e.to_string(),
        })?;
        crypto.insert(name.to_owned(), value);
    }
    Ok(Value::Object(crypto))
}

/// One oracle call that must succeed.
pub fn call(server: &mut OracleServer, op: &str, height: u64, args: &Value) -> Result<Value> {
    server
        .call(op, "devnet", height, args)?
        .map_err(|e| Error::Oracle(format!("{op} failed: {}: {}", e.class, e.message)))
}

/// A result of this run, in the file `file`.
fn result(
    file: &str,
    line: usize,
    op: &str,
    label: String,
    height: u64,
    outcome: Outcome,
) -> CaseResult {
    CaseResult {
        file: file.to_owned(),
        line,
        op: op.to_owned(),
        label,
        height,
        outcome,
    }
}

/// Compare Heartwood's `lines` with the oracle's `oracle_lines`, line by line; the results, and
/// the first differing line.
fn compare_lines(
    file: &str,
    oracle_lines: &[Value],
    lines: &[DigestLine],
    results: &mut Vec<CaseResult>,
) -> Option<usize> {
    let mut first_mismatch = None;
    for i in 0..oracle_lines.len().max(lines.len()) {
        let theirs = oracle_lines.get(i).cloned().unwrap_or(Value::Null);
        let ours = lines.get(i).map_or(Value::Null, digests::line_value);
        let label = step_label(if theirs.is_null() { &ours } else { &theirs });
        let differences = compare::diff(&theirs, &ours);
        let outcome = if differences.is_empty() {
            Outcome::Pass
        } else {
            first_mismatch.get_or_insert(i + 1);
            Outcome::Mismatch(differences)
        };
        let height = theirs.get("h").and_then(Value::as_u64).unwrap_or(0);
        let op = theirs
            .get("step")
            .and_then(Value::as_str)
            .unwrap_or("line")
            .to_owned();
        results.push(result(file, i + 1, &op, label, height, outcome));
    }
    first_mismatch
}

/// Replay `corpus` and compare every step with its recorded oracle digests. With `oracle`, the
/// first differing step is dumped on both sides and diffed by section, wallet and field, and the
/// synthetic cases run when `options` asks for them.
pub fn run(
    corpus: &Corpus,
    options: &L2Options,
    mut oracle: Option<&mut OracleServer>,
) -> Result<Report> {
    let corpus = &corpus.clone().as_judged(options.to)?;
    let (judge_ok, judge) = judge_report(&corpus.dir)?;
    let oracle_lines = digests::read(&corpus.dir.join(TS_DIGESTS))?;
    let file = format!("{}/{TS_DIGESTS}", corpus.name());
    let mut results = Vec::new();
    if !judge_ok {
        results.push(result(
            &file,
            0,
            "judge",
            "anchors".into(),
            0,
            Outcome::HarnessError(
                "the oracle's replay failed its anchors: it is not a judge (see anchors.json)"
                    .into(),
            ),
        ));
    }
    // The replay, through the store when the tree has one, else in memory.
    let mut walker = None;
    if options.store {
        match replay_through_store(corpus, 1) {
            Ok(w) => walker = Some(w),
            Err(problem) => results.push(result(
                &file,
                0,
                "store",
                "the replay through the store".into(),
                0,
                problem.outcome(),
            )),
        }
    }
    let heartwood = match &walker {
        Some(w) => w.replay.clone(),
        None => replay(corpus, None),
    };
    for problem in &heartwood.problems {
        results.push(result(
            &file,
            problem.line,
            "check",
            format!("height {}", problem.height),
            u64::from(problem.height),
            Outcome::HarnessError(problem.message.clone()),
        ));
    }
    let first_mismatch = compare_lines(&file, &oracle_lines, &heartwood.lines, &mut results);
    let ours: Vec<Value> = heartwood.lines.iter().map(digests::line_value).collect();
    for message in digests::revert_asymmetries(&ours) {
        results.push(result(
            &file,
            0,
            "revert",
            "symmetry".into(),
            0,
            Outcome::HarnessError(message),
        ));
    }
    if let Some(walker) = &mut walker {
        results.extend(store_results(corpus, walker, &heartwood));
    }
    if let (Some(line), Some(server)) = (first_mismatch, oracle.as_deref_mut()) {
        let theirs = oracle_records(server, corpus, line)?;
        let ours = replay(corpus, Some(line)).records;
        let message = match ours {
            Some(ours) => digests::section_differences(&theirs, &ours).join("\n"),
            None => "Heartwood's replay stopped before this step".into(),
        };
        results.push(result(
            &file,
            line,
            "dump",
            format!("records at line {line}"),
            0,
            Outcome::HarnessError(message),
        ));
    }
    if options.restart {
        results.extend(restart_results(corpus, &oracle_lines)?);
    }
    if options.synthetic {
        match oracle {
            Some(server) => results.extend(crate::synthetic::run(corpus, server)?),
            None => return Err(Error::Usage("the synthetic cases need the oracle".into())),
        }
    }
    Ok(Report {
        title: super::Level::L2.title().to_string(),
        source: describe_judge(&judge),
        files: 1,
        results,
    })
}

/// The store anchors of a replay through the store, and a second replay in chunks of 100 blocks
/// whose lines and rows must equal the first's.
fn store_results(corpus: &Corpus, walker: &mut Walker<'_>, heartwood: &Replay) -> Vec<CaseResult> {
    let file = format!("{}/store", corpus.name());
    let mut out = Vec::new();
    let tip = walker.state().height();
    let last_round = store::last_round(tip, corpus.revert_plan.round_length);
    let rows = match walker.store_rows() {
        Some(Ok(rows)) => rows,
        Some(Err(message)) => {
            out.push(result(
                &file,
                0,
                "anchor",
                "the store's rows".into(),
                u64::from(tip),
                Outcome::HarnessError(message),
            ));
            return out;
        }
        None => return out,
    };
    for (i, (label, outcome)) in store::anchors(&corpus.dir, &rows, tip, last_round)
        .into_iter()
        .enumerate()
    {
        out.push(result(
            &file,
            i + 1,
            "anchor",
            label,
            u64::from(tip),
            outcome,
        ));
    }
    // The same chain in chunks of 100 blocks.
    let chunked = match replay_through_store(corpus, 100) {
        Ok(mut chunked) => {
            let same_lines = chunked.replay.lines == heartwood.lines
                && chunked.replay.problems == heartwood.problems;
            let same_rows = chunked.store_rows().and_then(std::result::Result::ok) == Some(rows);
            if same_lines && same_rows {
                Outcome::Pass
            } else {
                Outcome::HarnessError(format!(
                    "chunks of 100 blocks: the lines are {}, the stored rows are {}",
                    if same_lines { "the same" } else { "different" },
                    if same_rows { "the same" } else { "different" }
                ))
            }
        }
        Err(problem) => problem.outcome(),
    };
    out.push(result(
        &file,
        4,
        "chunks",
        "the replay in chunks of 100 blocks equals the replay block by block".into(),
        u64::from(tip),
        chunked,
    ));
    out
}

/// The restart runs of `corpus`: for each of Heartwood's modes, the chain replayed through a
/// store with the restarts of the corpus's restart plan, each line compared with the continuous
/// oracle line of its step, and each restart line with the reference's restart lines.
fn restart_results(corpus: &Corpus, oracle_lines: &[Value]) -> Result<Vec<CaseResult>> {
    let plan = corpus.restart_plan.clone().ok_or_else(|| {
        Error::Usage(format!(
            "{} has no restart plan for its tip {}: judge it with RESTARTS=every or RESTARTS=rounds",
            corpus.dir.display(),
            corpus.tip()
        ))
    })?;
    let path = corpus.dir.join(restart::TS_RESTARTS);
    let records = restart::read_restarts(&path)?;
    let mut forward: BTreeMap<u32, Value> = BTreeMap::new();
    for line in oracle_lines {
        if matches!(
            line.get("step").and_then(Value::as_str),
            Some("genesis" | "apply")
        ) {
            let h = line.get("h").and_then(Value::as_u64).unwrap_or(0);
            forward.insert(u32::try_from(h).unwrap_or(0), line.clone());
        }
    }
    let mut out = Vec::new();
    let steps = restart::steps(&actions(corpus), &plan);
    for mode in plan.heartwood.clone() {
        out.extend(restart_run(
            corpus,
            &plan,
            mode,
            &steps,
            oracle_lines,
            &forward,
            &records,
        ));
    }
    Ok(out)
}

/// What the reference's restart lines say of a Heartwood restart line `ours` at `height`.
fn against_reference(records: &[RestartRecord], height: u32, ours: &Value) -> Outcome {
    let at: Vec<&RestartRecord> = records.iter().filter(|r| r.height == height).collect();
    if at.is_empty() {
        return Outcome::HarnessError(format!(
            "the judge wrote no restart line at {height}: judge the corpus again"
        ));
    }
    let keys = ["h", "id", "b", "s", "r", "m", "w", "sd"];
    let mut problems = Vec::new();
    let mut documented = false;
    for record in at {
        match &record.outcome {
            RestartOutcome::Refused(reason) => problems.push(format!(
                "the reference's {} restart was refused ({reason}), Heartwood's started",
                record.mode.name()
            )),
            RestartOutcome::Line(line) => {
                let differing = differing_keys(ours, line, &keys);
                if differing.is_empty() {
                    continue;
                }
                if record.noted("savedStateUsername") {
                    documented = true;
                } else {
                    problems.push(format!(
                        "the reference's {} restart line differs in {}",
                        record.mode.name(),
                        differing.join(", ")
                    ));
                }
            }
        }
    }
    if !problems.is_empty() {
        Outcome::HarnessError(problems.join("; "))
    } else if documented {
        Outcome::Documented(Documented::SavedStateUsername.reason().to_owned())
    } else {
        Outcome::Pass
    }
}

/// One restart run by `mode`.
fn restart_run(
    corpus: &Corpus,
    plan: &RestartPlan,
    mode: HeartwoodMode,
    steps: &[restart::Step],
    oracle_lines: &[Value],
    forward: &BTreeMap<u32, Value>,
    records: &[RestartRecord],
) -> Vec<CaseResult> {
    let file = format!("{}/restart-{}", corpus.name(), mode.name());
    let mut out = Vec::new();
    let mut walker = match Walker::with_store(corpus, 1) {
        Ok(walker) => walker,
        Err(problem) => {
            out.push(result(
                &file,
                0,
                "restart",
                format!("the {} restarts", mode.name()),
                0,
                problem.outcome(),
            ));
            return out;
        }
    };
    let by_mode: Vec<RestartRecord> = records
        .iter()
        .filter(|r| plan.reference.contains(&r.mode))
        .cloned()
        .collect();
    let compare = |walker: &Walker<'_>, what: String, want: Option<&Value>, keys: &[&str]| {
        let Some(line) = walker.replay.lines.last().map(digests::line_value) else {
            return Outcome::HarnessError("no line".into());
        };
        match want {
            None => Outcome::HarnessError(format!("{what}: no continuous line to compare with")),
            Some(want) => {
                let differing = differing_keys(&line, want, keys);
                if differing.is_empty() {
                    Outcome::Pass
                } else {
                    Outcome::HarnessError(format!("{what}: differs in {}", differing.join(", ")))
                }
            }
        }
    };
    let push = |out: &mut Vec<CaseResult>, walker: &Walker<'_>, label: String, outcome: Outcome| {
        out.push(result(
            &file,
            walker.replay.lines.len(),
            "restart",
            label,
            u64::from(walker.state().height()),
            outcome,
        ));
    };
    let first = compare(
        &walker,
        "the genesis step".into(),
        oracle_lines.first(),
        &SAME_STEP,
    );
    push(&mut out, &walker, "genesis".into(), first);
    let mut every_step = ["step"].to_vec();
    every_step.extend(SAME_STEP);
    for step in steps {
        let problems_before = walker.replay.problems.len();
        let (label, outcome) = match *step {
            restart::Step::Continuous { action, line } => {
                if !walker.step(action) {
                    push(
                        &mut out,
                        &walker,
                        format!("step {line}"),
                        Outcome::HarnessError("the replay stopped".into()),
                    );
                    break;
                }
                let outcome = compare(
                    &walker,
                    format!("step {line}"),
                    oracle_lines.get(line),
                    &every_step,
                );
                (format!("{:?}", action), outcome)
            }
            restart::Step::Restart(height) => match walker.restart(mode) {
                Err(problem) => {
                    push(
                        &mut out,
                        &walker,
                        format!("restart at {height}"),
                        problem.outcome(),
                    );
                    break;
                }
                Ok(path) => {
                    let line = walker
                        .replay
                        .lines
                        .last()
                        .map_or(Value::Null, digests::line_value);
                    let mut outcome = compare(
                        &walker,
                        format!("the restart at {height}"),
                        forward.get(&height),
                        &SAME_STATE,
                    );
                    if outcome == Outcome::Pass {
                        outcome = against_reference(&by_mode, height, &line);
                    }
                    if !store::took_its_path(mode, &path) {
                        outcome = Outcome::HarnessError(format!(
                            "the {} restart at {height} took the path {path:?}",
                            mode.name()
                        ));
                    }
                    (format!("restart at {height}"), outcome)
                }
            },
            restart::Step::RevertAfterRestart => {
                if !walker.revert() {
                    break;
                }
                let h = walker.state().height();
                let outcome = compare(
                    &walker,
                    format!("the revert to {h} after a restart"),
                    forward.get(&h),
                    &SAME_STATE,
                );
                (format!("revert to {h} after a restart"), outcome)
            }
            restart::Step::ReapplyAfterRestart(h) => {
                if !walker.apply(h, Step::Reapply) {
                    break;
                }
                let outcome = compare(
                    &walker,
                    format!("the re-apply of {h} after a restart"),
                    forward.get(&h),
                    &SAME_STEP,
                );
                (format!("re-apply of {h} after a restart"), outcome)
            }
        };
        let problems: Vec<String> = walker.replay.problems[problems_before..]
            .iter()
            .map(|p| p.message.clone())
            .collect();
        let outcome = if problems.is_empty() || outcome.is_failure() {
            outcome
        } else {
            Outcome::HarnessError(problems.join("; "))
        };
        push(&mut out, &walker, label, outcome);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::corpus::{RevertPlan, RevertStep};
    use crate::restart::ReferenceMode;

    #[test]
    fn actions_follow_the_revert_plan() {
        let plan = RevertPlan {
            seed: 0,
            round_length: 53,
            tip: 5,
            steps: vec![
                RevertStep {
                    after_height: 3,
                    blocks: 2,
                },
                RevertStep {
                    after_height: 3,
                    blocks: 1,
                },
            ],
        };
        use Action::*;
        assert_eq!(
            plan_actions([2, 3, 4], &plan),
            vec![
                Apply(2),
                Apply(3),
                Revert,
                Revert,
                Reapply(2),
                Reapply(3),
                Revert,
                Reapply(3),
                Apply(4)
            ]
        );
    }

    #[test]
    fn a_restart_line_is_judged_against_both_reference_paths() {
        let line = json!({"h": 5, "step": "restart", "id": "i", "b": "b", "s": "s", "r": "r", "m": "m", "w": "w", "sd": "d", "cd": "x"});
        let record = |mode, line: &Value, notes: Value| RestartRecord {
            mode,
            height: 5,
            outcome: RestartOutcome::Line(line.clone()),
            notes,
        };
        let mut other = line.clone();
        other["w"] = json!("other");
        other["cd"] = json!("y");
        let same = vec![
            record(ReferenceMode::Rebuild, &line, Value::Null),
            record(ReferenceMode::Saved, &line, Value::Null),
        ];
        assert_eq!(against_reference(&same, 5, &line), Outcome::Pass);
        assert!(against_reference(&same, 6, &line).is_failure());
        let saved_differs = vec![
            record(ReferenceMode::Rebuild, &line, Value::Null),
            record(ReferenceMode::Saved, &other, Value::Null),
        ];
        assert!(against_reference(&saved_differs, 5, &line).is_failure());
        let noted = vec![
            record(ReferenceMode::Rebuild, &line, Value::Null),
            record(
                ReferenceMode::Saved,
                &other,
                json!({"savedStateUsername": true}),
            ),
        ];
        assert!(matches!(
            against_reference(&noted, 5, &line),
            Outcome::Documented(_)
        ));
        let refused = vec![RestartRecord {
            mode: ReferenceMode::Rebuild,
            height: 5,
            outcome: RestartOutcome::Refused("State Generation failed".into()),
            notes: Value::Null,
        }];
        assert!(against_reference(&refused, 5, &line).is_failure());
    }
}
