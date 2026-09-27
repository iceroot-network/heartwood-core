//! The synthetic processor and fork cases on a corpus chain's prefix, each against the oracle's
//! `chain.process`, `chain.job` and session operations: every unchained status, a double-forged
//! block and its recovery at fixed depths, a refused block after a slot gap (the missed rows of a
//! refused block), an unchained first block, a milestone cut, conflicting and already forged
//! transactions, a removal across a round end with pool re-adds, a future slot, a removed type in
//! a block (as a job's first block and after an accepted one), a vote at a round's end with a
//! competing branch, and a milestone at exactly the last downloaded height.
//!
//! Both sides start from the corpus's tip: the oracle's chain session (`chain.open`, then every
//! corpus block), Heartwood's [`JobRig`] (the store's first start, then the corpus's blocks). Each
//! job runs on both sides, the oracle's through the reference's own job (`chain.job`), Heartwood's
//! through the download path and `run_job`, and is judged with [`judge_job`]: every result,
//! unchained status, missed row and digest line, the job's end, the committed count and the last
//! downloaded and forked blocks. A revert runs on both sides and each revert line is compared; the
//! enqueueing's cuts are compared where a case downloads several blocks at once.
//!
//! Blocks are built as the scheduled delegate of their slot builds them, with keys derived at run
//! time from the corpus's public devnet seed, on a mirror of the oracle's chain: Heartwood's state machine
//! applying the blocks the oracle accepted. So the oracle's side of every case runs even where
//! Heartwood's processor or job is still a stub; Heartwood's side of each step is then reported as
//! not built yet.

use heartwood_blockchain::Channel;
use heartwood_crypto::blocks::{BlockHeader, serialiser};
use heartwood_crypto::crypto::hash::sha256;
use heartwood_crypto::crypto::sig::{self, SchemeId, Signature, SigningDomain};
use heartwood_crypto::crypto::slots;
use heartwood_crypto::genesis::DerivedKey;
use heartwood_crypto::identities::Address;
use heartwood_crypto::transactions::builders::TransactionBuilder;
use heartwood_crypto::transactions::types::Votes;
use heartwood_crypto::utils::hex;
use heartwood_crypto::{Aux, Block, Milestones, Transaction};
use heartwood_kernel::config::crypto::CryptoTexts;
use heartwood_kernel::utils::calculate_forging_info::calculate_forging_info;
use heartwood_pool::Entry;
use heartwood_state::State;
use heartwood_transactions::Handlers;
use serde_json::{Value, json};

use crate::compare;
use crate::corpus::Corpus;
use crate::digests;
use crate::errors::{Error, Result};
use crate::levels::l2::{call, corpus_crypto};
use crate::levels::l2_processor::{
    JOB_BLOCKS, JobBlock, JobInputs, JobRig, Queued, RigJob, RigProblem, crypto_of, cut_jobs,
    judge_job, milestone_heights,
};
use crate::oracle::OracleServer;
use crate::report::{CaseResult, Outcome};
use crate::store::{self, guarded};
use crate::subject::Documented;
use crate::synthetic::{FEE, Keys, corpus_keys, transfer};

/// The variable naming a directory where each case writes the oracle's answer of each of its
/// steps (`<case>.json`), for the unit tests' recorded answers.
pub const RECORD_DIR: &str = "DIFFTEST_PROCESSOR_RECORD";

/// The aux of every synthetic signature.
const AUX: [u8; 32] = [0x5b; 32];
/// An IPFS hash for the removed type's transaction.
const IPFS_HASH: &str = "QmR45FmbVVrixReBwJkhEKde2qwHYaQzGxu4ZoDeswuF9w";

/// A synthetic processor case.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProcessorCase {
    /// Each unchained status once, inside a job after an accepted block.
    Unchained,
    /// A second block for the tip's slot by its scheduled delegate, then the recovery at a fixed
    /// depth and the other branch applied.
    DoubleForging(u32),
    /// A block refused in its apply after a 3-slot gap: the missed rows of a refused block.
    RefusedAfterGap,
    /// A job whose first block does not chain: the fork check.
    UnchainedFirst,
    /// A milestone at the tip plus three: a download cut into jobs there.
    MilestoneCut,
    /// Two transactions of one unique type from one sender, and a transaction forged earlier in
    /// the same job.
    ConflictingAndForged,
    /// A removal across a round end with pool transactions, re-added in chain order.
    RemovalAcrossRoundEnd,
    /// A block in a future slot after an accepted one.
    FutureSlot,
    /// A block with a transaction of a removed type (first and after an accepted block), and a
    /// rebuild that fails (first and after an accepted block).
    RemovedTypeAndRebuild,
    /// A vote in the last block of a round, a revert of two blocks and a competing branch that
    /// votes differently.
    VoteAtRoundEndFork,
    /// A milestone at exactly the last downloaded height, then a longer download: no milestone
    /// cut in it.
    MilestoneAtLastDownloaded,
}

impl ProcessorCase {
    /// Every case.
    pub const ALL: [ProcessorCase; 13] = [
        ProcessorCase::Unchained,
        ProcessorCase::DoubleForging(1),
        ProcessorCase::DoubleForging(3),
        ProcessorCase::DoubleForging(54),
        ProcessorCase::RefusedAfterGap,
        ProcessorCase::UnchainedFirst,
        ProcessorCase::MilestoneCut,
        ProcessorCase::ConflictingAndForged,
        ProcessorCase::RemovalAcrossRoundEnd,
        ProcessorCase::FutureSlot,
        ProcessorCase::RemovedTypeAndRebuild,
        ProcessorCase::VoteAtRoundEndFork,
        ProcessorCase::MilestoneAtLastDownloaded,
    ];

    /// The case's name in reports.
    pub fn name(self) -> String {
        match self {
            ProcessorCase::Unchained => "unchained-statuses".into(),
            ProcessorCase::DoubleForging(depth) => format!("double-forging-recovery-{depth}"),
            ProcessorCase::RefusedAfterGap => "refused-after-gap".into(),
            ProcessorCase::UnchainedFirst => "unchained-first-block".into(),
            ProcessorCase::MilestoneCut => "milestone-cut".into(),
            ProcessorCase::ConflictingAndForged => "conflicting-and-forged".into(),
            ProcessorCase::RemovalAcrossRoundEnd => "removal-across-round-end".into(),
            ProcessorCase::FutureSlot => "future-slot".into(),
            ProcessorCase::RemovedTypeAndRebuild => "removed-type-and-rebuild".into(),
            ProcessorCase::VoteAtRoundEndFork => "vote-at-round-end-fork".into(),
            ProcessorCase::MilestoneAtLastDownloaded => "milestone-at-last-downloaded".into(),
        }
    }

    /// The heights of the milestone entries the case adds to the corpus's, relative to the tip.
    fn milestones(self, tip: u32) -> Vec<u32> {
        match self {
            ProcessorCase::MilestoneCut => vec![tip + 3],
            ProcessorCase::MilestoneAtLastDownloaded => vec![tip, tip + 3],
            _ => Vec::new(),
        }
    }
}

/// The corpus's milestone file with an entry of the first milestone's block rules at each of
/// `heights` (the rules do not change), as JSON text; `None` without heights.
pub fn milestones_with(corpus: &Corpus, heights: &[u32]) -> Result<Option<String>> {
    if heights.is_empty() {
        return Ok(None);
    }
    let path = corpus.dir.join("crypto").join("milestones.json");
    let text = std::fs::read_to_string(&path).map_err(|e| Error::io(&path, e))?;
    let mut entries: Value = serde_json::from_str(&text).map_err(|e| Error::Corpus {
        path: path.clone(),
        message: e.to_string(),
    })?;
    let block = entries[0]["block"].clone();
    let list = entries.as_array_mut().ok_or_else(|| Error::Corpus {
        path: path.clone(),
        message: "the milestone file is not a list".into(),
    })?;
    for height in heights {
        list.push(json!({"height": height, "block": block}));
    }
    Ok(Some(entries.to_string()))
}

/// The slot time of the block `bytes` in milliseconds since the Unix epoch.
fn slot_time(bytes: &[u8], milestones: &Milestones) -> i64 {
    let epoch = slots::epoch_unix_ms(milestones.at(1).epoch()).unwrap_or(0);
    JobBlock::of(bytes.to_vec())
        .parts()
        .map_or(0, |(header, _)| {
            epoch.saturating_add(i64::from(header.timestamp).saturating_mul(1000))
        })
}

/// A block built by [`block_on`]: its bytes and the delegate scheduled for its slot.
struct Built {
    bytes: Vec<u8>,
    scheduled: heartwood_crypto::identities::PublicKeyBytes,
}

/// The next block on `state`, with `transactions`, in the first slot at least `gap` slots after
/// the next one whose delegate the seed derives, signed by `signer` (the scheduled delegate when
/// `None`), with the reward of the scheduled delegate in its round.
fn block_on(
    state: &State,
    milestones: &Milestones,
    keys: &Keys,
    transactions: &[Transaction],
    gap: u32,
    signer: Option<&DerivedKey>,
) -> Result<Built> {
    let last = state.last_block().header().clone();
    let height = last.height + 1;
    let params = milestones.at(height);
    let block_time = u32::try_from(params.block_time()).unwrap_or(8);
    for extra in gap..gap + 200 {
        let timestamp = last.timestamp + block_time * (1 + extra);
        let info = calculate_forging_info(i64::from(timestamp), height, milestones);
        let Some(scheduled) = usize::try_from(info.current_forger)
            .ok()
            .and_then(|i| state.forging_delegates().get(i))
        else {
            continue;
        };
        let Some(forger) = keys
            .delegates()
            .find(|k| k.keys.public_key().to_compressed() == scheduled.public_key)
        else {
            continue;
        };
        let signer = signer.unwrap_or(forger);
        let reward = state
            .reward_for_block_in_round(height, &scheduled.public_key, milestones)
            .map(|r| r.reward)
            .unwrap_or(0);
        let mut ids = Vec::new();
        let (mut total_amount, mut total_fee) = (0u64, 0u64);
        for t in transactions {
            ids.extend_from_slice(t.id().as_bytes());
            total_amount = total_amount.saturating_add(t.amount());
            total_fee = total_fee.saturating_add(t.fee());
        }
        let count = u32::try_from(transactions.len()).unwrap_or(u32::MAX);
        let mut header = BlockHeader {
            version: params.block_version(),
            timestamp,
            height,
            previous_block: *state.last_block().id().as_bytes(),
            number_of_transactions: count,
            total_amount,
            total_fee,
            reward,
            payload_length: count.saturating_mul(32),
            payload_hash: sha256(&ids),
            generator_public_key: signer.keys.public_key().to_compressed(),
            block_signature: Signature::from_array([0; 64]),
        };
        header.block_signature = sig::sign(
            SchemeId::Secp256k1Bip340,
            SigningDomain::Block,
            &serialiser::signing_hash(&header),
            signer.keys.secret_key(),
            Aux::fixed(AUX),
        )
        .map_err(|e| Error::Oracle(format!("signing a block: {e}")))?;
        return Ok(Built {
            bytes: serialiser::serialise_with_transactions(&header, transactions),
            scheduled: scheduled.public_key,
        });
    }
    Err(Error::Oracle(format!(
        "no slot at height {height} has a delegate the seed derives"
    )))
}

/// Both sides of a case at the same chain.
struct Pair<'a> {
    corpus: &'a Corpus,
    keys: &'a Keys,
    name: String,
    milestones: Milestones,
    milestones_text: Option<String>,
    session: Value,
    /// Heartwood's state machine, following the oracle's chain: the base of every block built.
    mirror: State,
    /// Heartwood's node, or why it has none.
    rig: std::result::Result<JobRig, RigProblem>,
    /// Whether the oracle's session has the reference's pool.
    pool: bool,
    results: Vec<CaseResult>,
    step: usize,
    /// The oracle's answer of each step, for [`RECORD_DIR`].
    answers: Vec<Value>,
}

impl<'a> Pair<'a> {
    /// Both sides at the corpus's tip, with the milestone file `milestones_text` when given and
    /// the reference's pool in the oracle's session when `pool`.
    fn open(
        corpus: &'a Corpus,
        keys: &'a Keys,
        server: &mut OracleServer,
        name: String,
        milestones_text: Option<String>,
        pool: bool,
    ) -> Result<Pair<'a>> {
        let milestones = match &milestones_text {
            Some(text) => Milestones::load(text, &corpus.network).map_err(|e| Error::Corpus {
                path: corpus.dir.join("crypto/milestones.json"),
                message: format!("the case's milestones: {e}"),
            })?,
            None => corpus.milestones.clone(),
        };
        let mut blocks = Vec::new();
        for block in &corpus.blocks {
            blocks.push(Block::receive(&block.bytes, &milestones).map_err(|e| {
                Error::Oracle(format!("block {} does not decode: {e}", block.height))
            })?);
        }
        let mut mirror = State::from_genesis(&corpus.genesis, &milestones, &Handlers)
            .map_err(|e| Error::Oracle(format!("Heartwood refused the genesis block: {e}")))?;
        for block in &blocks {
            mirror
                .apply_block(block, &milestones, &Handlers)
                .map_err(|e| Error::Oracle(format!("block {}: {e}", block.height())))?;
        }
        let rig = Self::rig(corpus, milestones_text.as_deref(), &blocks);
        let mut args = json!({"crypto": corpus_crypto(corpus)?});
        if let Some(text) = &milestones_text {
            args["milestones"] = json!(text);
        }
        if pool {
            args["pool"] = json!({"options": {}});
        }
        let opened = call(server, "chain.open", 1, &args)?;
        let session = opened.get("session").cloned().unwrap_or(Value::Null);
        for block in &corpus.blocks {
            let r = call(
                server,
                "chain.apply",
                1,
                &json!({"session": session, "hex": hex::encode(&block.bytes)}),
            )?;
            if r.get("accepted") != Some(&Value::Bool(true)) {
                return Err(Error::Oracle(format!(
                    "the oracle refused block {}: {r}",
                    block.height
                )));
            }
        }
        Ok(Pair {
            corpus,
            keys,
            name,
            milestones,
            milestones_text,
            session,
            mirror,
            rig,
            pool,
            results: Vec::new(),
            step: 0,
            answers: Vec::new(),
        })
    }

    /// Heartwood's node at the corpus's tip.
    fn rig(
        corpus: &Corpus,
        milestones_text: Option<&str>,
        blocks: &[Block],
    ) -> std::result::Result<JobRig, RigProblem> {
        let read = |name: &str| std::fs::read_to_string(corpus.dir.join("crypto").join(name)).ok();
        let files = crypto_of(&CryptoTexts {
            network: read("network.json"),
            milestones: milestones_text
                .map(str::to_owned)
                .or_else(|| read("milestones.json")),
            genesis: read("genesisBlock.json"),
            exceptions: read("exceptions.json"),
        })?;
        let mut rig = JobRig::start(&files)?;
        rig.extend(blocks)?;
        Ok(rig)
    }

    fn record(&mut self, what: &str, outcome: Outcome) {
        self.step += 1;
        self.results.push(CaseResult {
            file: format!("{}/synthetic-processor", self.corpus.name()),
            line: self.step,
            op: self.name.clone(),
            label: format!("{} {}: {what}", self.name, self.step),
            height: u64::from(self.mirror.height()),
            outcome,
        });
    }

    /// Record Heartwood's side as not run, with the reason its node is missing.
    fn not_run(&mut self, what: &str) {
        let outcome = match &self.rig {
            Err(problem) => problem.outcome(),
            Ok(_) => Outcome::HarnessError("Heartwood's node is there".into()),
        };
        self.record(what, outcome);
    }

    /// The tip's height.
    fn tip(&self) -> u32 {
        self.mirror.height()
    }

    /// The next block on the mirror (see [`block_on`]).
    fn next(
        &self,
        transactions: &[Transaction],
        gap: u32,
        signer: Option<&DerivedKey>,
    ) -> Result<Built> {
        block_on(
            &self.mirror,
            &self.milestones,
            self.keys,
            transactions,
            gap,
            signer,
        )
    }

    /// `n` empty blocks on the mirror, each on the one before it (the mirror is not changed).
    fn empty_blocks(&self, n: usize) -> Result<Vec<Vec<u8>>> {
        let mut state = self.mirror.clone();
        let mut out = Vec::new();
        for _ in 0..n {
            let built = block_on(&state, &self.milestones, self.keys, &[], 0, None)?;
            let block = Block::receive(&built.bytes, &self.milestones)
                .map_err(|e| Error::Oracle(format!("a built block: {e}")))?;
            state
                .apply_block(&block, &self.milestones, &Handlers)
                .map_err(|e| Error::Oracle(format!("a built block was refused: {e}")))?;
            out.push(built.bytes);
        }
        Ok(out)
    }

    /// A transaction by `key` under the rules of the next height.
    fn build(&self, builder: TransactionBuilder, key: &DerivedKey) -> Result<Transaction> {
        let params = self.milestones.at(self.tip() + 1);
        builder
            .network(params.network())
            .build(&key.keys, Aux::fixed(AUX), params)
            .map_err(|e| Error::Oracle(format!("building a transaction: {e}")))
    }

    /// The nonce of `address` on the mirror.
    fn nonce(&self, address: &Address) -> u64 {
        self.mirror
            .wallets()
            .find_by_address(address)
            .map_or(0, heartwood_state::Wallet::nonce)
    }

    /// A test wallet of the seed with at least `amount` and its key bound, after `skip` others.
    fn funded(&self, amount: u64, skip: usize) -> Result<&'a DerivedKey> {
        let mirror = &self.mirror;
        self.keys
            .wallets()
            .filter(|k| {
                mirror
                    .wallets()
                    .find_by_address(&k.address)
                    .is_some_and(|w| w.balance() >= amount && w.public_key().is_some())
            })
            .nth(skip)
            .ok_or_else(|| Error::Oracle(format!("{}: no funded test wallet fits", self.name)))
    }

    /// A transfer of `amount` from `from` to `to`, at `from`'s nonce plus `ahead`.
    fn pay(
        &self,
        from: &DerivedKey,
        to: &DerivedKey,
        amount: u64,
        ahead: u64,
    ) -> Result<Transaction> {
        let nonce = self.nonce(&from.address) + ahead;
        self.build(
            transfer(&[(to.address, amount)]).nonce(nonce).fee(FEE),
            from,
        )
    }

    /// Apply the blocks the oracle accepted to the mirror, in the job's order.
    fn follow(&mut self, blocks: &[JobBlock], oracle: &Value) -> Result<()> {
        let results = oracle["results"].as_array().cloned().unwrap_or_default();
        for (block, result) in blocks.iter().zip(&results) {
            if result["result"] != "Accepted" {
                break;
            }
            let decoded = Block::receive(&block.bytes, &self.milestones)
                .map_err(|e| Error::Oracle(format!("an accepted block: {e}")))?;
            self.mirror
                .apply_block(&decoded, &self.milestones, &Handlers)
                .map_err(|e| {
                    Error::Oracle(format!(
                        "the mirror refused a block the oracle accepted: {e}"
                    ))
                })?;
        }
        Ok(())
    }

    /// One job on both sides, judged; the oracle's answer.
    fn job(
        &mut self,
        server: &mut OracleServer,
        what: &str,
        blocks: Vec<JobBlock>,
        inputs: JobInputs,
    ) -> Result<Value> {
        let hexes: Vec<Value> = blocks
            .iter()
            .map(|b| match b.count {
                None => json!(hex::encode(&b.bytes)),
                Some(n) => {
                    json!({"hex": hex::encode(&b.bytes), "set": {"numberOfTransactions": n}})
                }
            })
            .collect();
        let mut args = inputs.to_json();
        args["session"] = self.session.clone();
        args["hexes"] = Value::Array(hexes);
        let oracle = call(server, "chain.job", 1, &args)?;
        self.answers
            .push(json!({"what": what, "op": "chain.job", "answer": oracle}));
        self.follow(&blocks, &oracle)?;
        let ran = match &mut self.rig {
            Ok(rig) => rig.run(&blocks, &inputs, &[]),
            Err(_) => {
                self.not_run(what);
                return Ok(oracle);
            }
        };
        match ran {
            Ok(ours) => {
                let outcome = judge_job(&blocks, &oracle, &ours);
                self.record(what, outcome);
                if let RigJob::Refused(_) = ours {
                    self.resync(server, what, &blocks, &oracle)?;
                }
            }
            Err(problem) => {
                self.rig = Err(problem);
                self.not_run(what);
            }
        }
        Ok(oracle)
    }

    /// After Heartwood's download refused a listing, the blocks the oracle accepted from it
    /// arrive again without the refused one, as the next download brings them; their lines must
    /// be the oracle's.
    fn resync(
        &mut self,
        _server: &mut OracleServer,
        what: &str,
        blocks: &[JobBlock],
        oracle: &Value,
    ) -> Result<()> {
        let results = oracle["results"].as_array().cloned().unwrap_or_default();
        let accepted: Vec<(JobBlock, Value)> = blocks
            .iter()
            .zip(&results)
            .take_while(|(_, r)| r["result"] == "Accepted")
            .map(|(b, r)| (b.clone(), r["line"].clone()))
            .collect();
        if accepted.is_empty() {
            return Ok(());
        }
        let job: Vec<JobBlock> = accepted.iter().map(|(b, _)| b.clone()).collect();
        let now = job
            .last()
            .map_or(0, |b| slot_time(&b.bytes, &self.milestones));
        let what = format!("{what}: the accepted blocks downloaded again");
        let Ok(rig) = &mut self.rig else {
            self.not_run(&what);
            return Ok(());
        };
        match rig.run(&job, &JobInputs::at(now), &[]) {
            Ok(RigJob::Ran { seen, .. }) => {
                let theirs: Vec<Value> = accepted.into_iter().map(|(_, line)| line).collect();
                let ours: Vec<Value> = seen
                    .iter()
                    .map(|s| s.line.as_ref().map_or(Value::Null, digests::line_value))
                    .collect();
                let differences = compare::diff(&Value::Array(theirs), &Value::Array(ours));
                let outcome = if differences.is_empty() {
                    Outcome::Pass
                } else {
                    Outcome::Mismatch(differences)
                };
                self.record(&what, outcome);
            }
            Ok(RigJob::Refused(why)) => self.record(
                &what,
                Outcome::HarnessError(format!("refused again: {why}")),
            ),
            Err(problem) => {
                self.rig = Err(problem);
                self.not_run(&what);
            }
        }
        Ok(())
    }

    /// The enqueueing's cuts of `blocks` with the last downloaded block at the tip, on both
    /// sides, compared; the oracle's cuts as indices into `blocks`.
    fn enqueue(
        &mut self,
        server: &mut OracleServer,
        what: &str,
        blocks: &[JobBlock],
    ) -> Result<Vec<Vec<usize>>> {
        let mut queued = Vec::new();
        for b in blocks {
            let (header, _) = b.parts().map_err(Error::Oracle)?;
            queued.push(Queued {
                height: header.height,
                transactions: header.number_of_transactions,
            });
        }
        let tip = self.tip();
        let listed: Vec<Value> = queued
            .iter()
            .map(|q| json!({"height": q.height, "numberOfTransactions": q.transactions}))
            .collect();
        let mut args = json!({"enqueue": {"blocks": listed, "lastDownloadedHeight": tip}});
        if let Some(text) = &self.milestones_text {
            args["milestones"] = json!(text);
        }
        let oracle = call(server, "chain.job", 1, &args)?;
        self.answers
            .push(json!({"what": what, "op": "chain.job", "answer": oracle}));
        let theirs: Vec<Vec<usize>> = oracle["jobs"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|job| {
                job.as_array()
                    .cloned()
                    .unwrap_or_default()
                    .iter()
                    .filter_map(|h| {
                        queued
                            .iter()
                            .position(|q| Some(u64::from(q.height)) == h.as_u64())
                    })
                    .collect()
            })
            .collect();
        let ours = cut_jobs(
            &queued,
            tip,
            &milestone_heights(&self.milestones),
            JOB_BLOCKS,
        );
        let outcome = if ours == theirs {
            Outcome::Pass
        } else {
            Outcome::HarnessError(format!(
                "the reference cuts the download as {theirs:?}, Heartwood as {ours:?}"
            ))
        };
        self.record(what, outcome);
        Ok(theirs)
    }

    /// Revert the last `count` blocks on both sides, each revert line compared. With the
    /// reference's pool, the removed transactions are re-added in chain order on both sides and
    /// the pools compared.
    fn revert(&mut self, server: &mut OracleServer, what: &str, count: u32) -> Result<()> {
        let op = if self.pool {
            "pool.revert"
        } else {
            "chain.revert"
        };
        let answer = call(
            server,
            op,
            1,
            &json!({"session": self.session, "count": count}),
        )?;
        self.answers
            .push(json!({"what": what, "op": op, "answer": answer}));
        for _ in 0..count {
            self.mirror
                .revert_block(&self.milestones, &Handlers)
                .map_err(|e| Error::Oracle(format!("the mirror refused a revert: {e}")))?;
        }
        let Ok(rig) = &mut self.rig else {
            self.not_run(what);
            return Ok(());
        };
        let mut removed = Vec::new();
        let mut problem = None;
        for _ in 0..count {
            match rig.revert() {
                Ok(reverted) => removed.push(reverted.block),
                Err(why) => {
                    problem = Some(why);
                    break;
                }
            }
        }
        if let Some(why) = problem {
            self.record(what, Outcome::HarnessError(why));
            return Ok(());
        }
        let theirs = answer["lines"].clone();
        let lines = rig.lines();
        let from = lines.len().saturating_sub(count as usize);
        let ours = Value::Array(
            lines
                .get(from..)
                .unwrap_or_default()
                .iter()
                .map(digests::line_value)
                .collect(),
        );
        let differences = compare::diff(&theirs, &ours);
        let outcome = if differences.is_empty() {
            Outcome::Pass
        } else {
            Outcome::Mismatch(differences)
        };
        self.record(what, outcome);
        if self.pool {
            // The reference re-adds the removed transactions in chain order.
            let transactions: Vec<Transaction> = removed
                .iter()
                .rev()
                .flat_map(|b| b.transactions().iter().cloned())
                .collect();
            let now = self.mirror.last_block().header().timestamp;
            let now = slots::epoch_unix_ms(self.milestones.at(1).epoch())
                .unwrap_or(0)
                .saturating_add(i64::from(now).saturating_mul(1000));
            let what = format!("{what}: the pool after the re-add");
            let Ok(rig) = &mut self.rig else {
                return Ok(());
            };
            let ours = rig
                .readd_removed(&transactions, now)
                .and_then(|()| rig.pool_ids());
            match ours {
                Ok(ids) => {
                    let theirs = answer["contents"]["highest"].clone();
                    let differences = compare::diff(&theirs, &json!(ids));
                    let outcome = if differences.is_empty() {
                        Outcome::Pass
                    } else {
                        Outcome::Mismatch(differences)
                    };
                    self.record(&what, outcome);
                }
                Err(problem) => {
                    let outcome = problem.outcome();
                    self.record(&what, outcome);
                }
            }
        }
        Ok(())
    }

    /// A job of `blocks` at the latest of their slot times, so that none takes a future slot.
    fn job_at_latest(
        &mut self,
        server: &mut OracleServer,
        what: &str,
        blocks: Vec<Vec<u8>>,
    ) -> Result<Value> {
        let now = blocks
            .iter()
            .map(|b| slot_time(b, &self.milestones))
            .max()
            .unwrap_or(0);
        let blocks = blocks.into_iter().map(JobBlock::of).collect();
        self.job(server, what, blocks, JobInputs::at(now))
    }
}

/// The derived delegate key of `public_key`.
fn delegate_key<'k>(
    keys: &'k Keys,
    public_key: &heartwood_crypto::identities::PublicKeyBytes,
) -> Option<&'k DerivedKey> {
    keys.delegates()
        .find(|k| &k.keys.public_key().to_compressed() == public_key)
}

/// Each unchained status inside a job, after an accepted block.
fn unchained(p: &mut Pair<'_>, server: &mut OracleServer) -> Result<()> {
    // EqualToLastBlock: an empty block and the same block again.
    let b1 = p.next(&[], 0, None)?.bytes;
    p.job_at_latest(
        server,
        "a block and the same block again",
        vec![b1.clone(), b1],
    )?;
    // AlreadyInBlockchain: a new block, then the block before it.
    let old = p.mirror.last_block().clone();
    let b2 = p.next(&[], 0, None)?.bytes;
    let old_bytes = serialiser::serialise(&old);
    p.job_at_latest(
        server,
        "a block, then the one before it",
        vec![b2, old_bytes],
    )?;
    // NotReadyToAcceptNewHeight: a block, then one two heights above it.
    let three = p.empty_blocks(3)?;
    p.job_at_latest(
        server,
        "a block, then one two heights above it",
        vec![three[0].clone(), three[2].clone()],
    )?;
    // InvalidTimestamp: a block one slot late, then another at its height in the slot before.
    let late = p.next(&[], 1, None)?.bytes;
    let early = p.next(&[], 0, None)?.bytes;
    let now = slot_time(&late, &p.milestones);
    p.job(
        server,
        "a block, then another at its height with an earlier timestamp",
        vec![JobBlock::of(late), JobBlock::of(early)],
        JobInputs::at(now),
    )?;
    // GeneratorMismatch: a block, then another at its height in a later slot, signed by a
    // delegate that is not the one scheduled there.
    let first = p.next(&[], 0, None)?.bytes;
    let probe = p.next(&[], 3, None)?;
    let other = p
        .keys
        .delegates()
        .find(|k| k.keys.public_key().to_compressed() != probe.scheduled)
        .ok_or_else(|| Error::Oracle("no other delegate".into()))?;
    let mismatch = p.next(&[], 3, Some(other))?.bytes;
    let now = slot_time(&mismatch, &p.milestones);
    p.job(
        server,
        "a block, then another at its height by a delegate not scheduled",
        vec![JobBlock::of(first), JobBlock::of(mismatch)],
        JobInputs::at(now),
    )?;
    // DoubleForging: a block, then a second block for its slot by the same delegate.
    let from = p.funded(1_000_000_000, 0)?;
    let to = p.funded(0, 1)?;
    let t = p.pay(from, to, 100_000_000, 1)?;
    let plain = p.next(&[], 0, None)?.bytes;
    let twin = p.next(&[t], 0, None)?.bytes;
    p.job_at_latest(
        server,
        "a block, then a second block for its slot by its delegate",
        vec![plain, twin],
    )?;
    Ok(())
}

/// A double-forged block, the recovery at `depth`, and the other branch applied.
fn double_forging(p: &mut Pair<'_>, server: &mut OracleServer, depth: u32) -> Result<()> {
    let from = p.funded(1_000_000_000, 0)?;
    let to = p.funded(0, 1)?;
    let t = p.pay(from, to, 100_000_000, 1)?;
    let plain = p.next(&[], 0, None)?.bytes;
    let twin = p.next(&[t], 0, None)?.bytes;
    let height = p.tip() + 1;
    p.job_at_latest(
        server,
        "a block, then a second block for its slot by its delegate",
        vec![plain, twin.clone()],
    )?;
    // The recovery: `depth` blocks removed, then the branch of the forked block downloaded.
    p.revert(
        server,
        &format!("the recovery removes {depth} blocks"),
        depth,
    )?;
    let mut branch = Vec::new();
    for h in (height - depth + 1)..height {
        let bytes = match p.corpus.block(h) {
            Some(b) => b.bytes.clone(),
            None => {
                return Err(Error::Oracle(format!("the corpus has no block {h}")));
            }
        };
        branch.push(bytes);
    }
    branch.push(twin);
    p.job_at_latest(server, "the forked block's branch", branch)?;
    Ok(())
}

/// A block refused in its apply after a 3-slot gap; Heartwood's store keeps no missed row of it.
fn refused_after_gap(p: &mut Pair<'_>, server: &mut OracleServer) -> Result<()> {
    let from = p.funded(1_000_000_000, 0)?;
    let to = p.funded(0, 1)?;
    let balance = p
        .mirror
        .wallets()
        .find_by_address(&from.address)
        .map_or(0, heartwood_state::Wallet::balance);
    let first = p.pay(from, to, balance / 2, 1)?;
    let second = p.pay(from, to, balance / 2 + 1, 2)?;
    let bytes = p.next(&[first, second], 3, None)?.bytes;
    let height = p.tip() + 1;
    p.job_at_latest(server, "an overspend after 3 missed slots", vec![bytes])?;
    let Ok(rig) = &p.rig else {
        p.not_run("Heartwood's store keeps no missed row of the refused block");
        return Ok(());
    };
    let tip = rig.state().height();
    let round_length = p.corpus.revert_plan.round_length;
    let rows = guarded(|| {
        store::read_rows(
            rig.store().database(),
            tip,
            store::last_round(tip, round_length),
        )
    });
    let outcome = match rows {
        Ok(Ok(rows)) => {
            let kept: Vec<_> = rows.missed.iter().filter(|r| r.height == height).collect();
            if kept.is_empty() {
                Outcome::Documented(Documented::MissedRowsOfRefusedBlock.reason().to_owned())
            } else {
                Outcome::HarnessError(format!(
                    "Heartwood's store keeps {} missed rows of the refused block",
                    kept.len()
                ))
            }
        }
        Ok(Err(why)) => Outcome::HarnessError(why),
        Err(store::Unbuilt(m)) => Outcome::Skipped(format!("not built yet: {m}")),
    };
    p.record(
        "Heartwood's store keeps no missed row of the refused block",
        outcome,
    );
    Ok(())
}

/// A download of `n` blocks cut into jobs by the enqueueing, each job run on both sides: the
/// download's last block is the last downloaded block before its first job, as a node syncs.
fn download_in_jobs(p: &mut Pair<'_>, server: &mut OracleServer, n: usize) -> Result<()> {
    let bytes = p.empty_blocks(n)?;
    let blocks: Vec<JobBlock> = bytes.into_iter().map(JobBlock::of).collect();
    let jobs = p.enqueue(server, "the enqueueing's cuts", &blocks)?;
    let now = blocks
        .last()
        .map_or(0, |b| slot_time(&b.bytes, &p.milestones));
    for (i, job) in jobs.iter().enumerate() {
        let job_blocks: Vec<JobBlock> =
            job.iter().filter_map(|&k| blocks.get(k).cloned()).collect();
        let mut inputs = JobInputs::at(now);
        inputs.downloaded = false;
        if i == 0 {
            inputs.last_downloaded = blocks.last().map(|b| b.bytes.clone());
        }
        p.job(
            server,
            &format!("job {} of the download", i + 1),
            job_blocks,
            inputs,
        )?;
    }
    Ok(())
}

/// A transaction of a removed type, by `key`, built by the reference's builder.
fn removed_type(p: &Pair<'_>, server: &mut OracleServer, key: &DerivedKey) -> Result<Vec<u8>> {
    let nonce = p.nonce(&key.address) + 1;
    let answer = call(
        server,
        "tx.build",
        2,
        &json!({
            "builder": "ipfs",
            "calls": [
                ["ipfsAsset", IPFS_HASH],
                ["nonce", nonce.to_string()],
                ["fee", FEE.to_string()],
                ["network", p.corpus.network.pub_key_hash],
            ],
            "sign": key.passphrase.as_str(),
        }),
    )?;
    let text = answer["hex"]
        .as_str()
        .ok_or_else(|| Error::Oracle("tx.build sent no hex".into()))?;
    hex::decode(text).map_err(|e| Error::Oracle(format!("tx.build's hex: {e}")))
}

/// Block bytes with a transaction's raw bytes as given (a type the decoder refuses), signed by
/// the scheduled delegate of the next slot on the mirror.
fn block_with_raw(p: &Pair<'_>, state: &State, raw: &[u8]) -> Result<Vec<u8>> {
    // The header is built for an empty block and then given the transaction: the payload hash
    // is over the transaction's id, the SHA-256 of its bytes.
    let id = sha256(raw);
    let last = state.last_block().header().clone();
    let height = last.height + 1;
    let params = p.milestones.at(height);
    let block_time = u32::try_from(params.block_time()).unwrap_or(8);
    let timestamp = last.timestamp + block_time;
    let info = calculate_forging_info(i64::from(timestamp), height, &p.milestones);
    let scheduled = usize::try_from(info.current_forger)
        .ok()
        .and_then(|i| state.forging_delegates().get(i))
        .ok_or_else(|| Error::Oracle("no delegate for the slot".into()))?;
    let signer = delegate_key(p.keys, &scheduled.public_key)
        .ok_or_else(|| Error::Oracle("the slot's delegate is not the seed's".into()))?;
    let reward = state
        .reward_for_block_in_round(height, &scheduled.public_key, &p.milestones)
        .map(|r| r.reward)
        .unwrap_or(0);
    let mut header = BlockHeader {
        version: params.block_version(),
        timestamp,
        height,
        previous_block: *state.last_block().id().as_bytes(),
        number_of_transactions: 1,
        total_amount: 0,
        total_fee: FEE,
        reward,
        payload_length: 32,
        payload_hash: sha256(&id),
        generator_public_key: signer.keys.public_key().to_compressed(),
        block_signature: Signature::from_array([0; 64]),
    };
    header.block_signature = sig::sign(
        SchemeId::Secp256k1Bip340,
        SigningDomain::Block,
        &serialiser::signing_hash(&header),
        signer.keys.secret_key(),
        Aux::fixed(AUX),
    )
    .map_err(|e| Error::Oracle(format!("signing a block: {e}")))?;
    let mut bytes = serialiser::serialise_header(&header, true);
    let length = u32::try_from(raw.len()).unwrap_or(u32::MAX);
    bytes.extend_from_slice(&length.to_le_bytes());
    bytes.extend_from_slice(raw);
    Ok(bytes)
}

/// A removed type as a job's first block and after an accepted one; a rebuild that fails as a
/// job's first block and after an accepted one.
fn removed_type_and_rebuild(p: &mut Pair<'_>, server: &mut OracleServer) -> Result<()> {
    let sender = p.funded(1_000_000_000, 0)?;
    let raw = removed_type(p, server, sender)?;
    let first = block_with_raw(p, &p.mirror.clone(), &raw)?;
    p.job_at_latest(
        server,
        "a removed type in the job's first block",
        vec![first],
    )?;
    let plain = p.next(&[], 0, None)?.bytes;
    let mut state = p.mirror.clone();
    let decoded = Block::receive(&plain, &p.milestones)
        .map_err(|e| Error::Oracle(format!("a built block: {e}")))?;
    state
        .apply_block(&decoded, &p.milestones, &Handlers)
        .map_err(|e| Error::Oracle(format!("a built block was refused: {e}")))?;
    let second = block_with_raw(p, &state, &raw)?;
    p.job_at_latest(
        server,
        "a removed type after an accepted block",
        vec![plain, second],
    )?;
    // A listing whose transaction count is not its transactions': the rebuild fails.
    let from = p.funded(1_000_000_000, 0)?;
    let to = p.funded(0, 1)?;
    let t = p.pay(from, to, 100_000_000, 1)?;
    let bad = p.next(std::slice::from_ref(&t), 0, None)?.bytes;
    let now = slot_time(&bad, &p.milestones);
    p.job(
        server,
        "a rebuild that fails as the job's first block",
        vec![JobBlock {
            bytes: bad,
            count: Some(2),
        }],
        JobInputs::at(now),
    )?;
    let blocks = p.empty_blocks(1)?;
    let mut state = p.mirror.clone();
    let decoded = Block::receive(&blocks[0], &p.milestones)
        .map_err(|e| Error::Oracle(format!("a built block: {e}")))?;
    state
        .apply_block(&decoded, &p.milestones, &Handlers)
        .map_err(|e| Error::Oracle(format!("a built block was refused: {e}")))?;
    let t = p.pay(from, to, 100_000_000, 1)?;
    let bad = block_on(&state, &p.milestones, p.keys, &[t], 0, None)?.bytes;
    let now = slot_time(&bad, &p.milestones);
    p.job(
        server,
        "a rebuild that fails after an accepted block",
        vec![
            JobBlock::of(blocks[0].clone()),
            JobBlock {
                bytes: bad,
                count: Some(2),
            },
        ],
        JobInputs::at(now),
    )?;
    Ok(())
}

/// Two votes of one sender in a block; a transaction forged earlier in the same job.
fn conflicting_and_forged(p: &mut Pair<'_>, server: &mut OracleServer) -> Result<()> {
    let voter = p.funded(1_000_000_000, 0)?;
    let names: Vec<String> = p
        .mirror
        .wallets()
        .delegates()
        .filter_map(|w| w.delegate())
        .filter(|d| d.resigned.is_none())
        .map(|d| d.username().to_owned())
        .take(2)
        .collect();
    let [a, b] = names.as_slice() else {
        return Err(Error::Oracle("fewer than two delegates".into()));
    };
    let mut first = Votes::new();
    first.insert(a.clone(), 10_000);
    let mut second = Votes::new();
    second.insert(b.clone(), 10_000);
    let nonce = p.nonce(&voter.address);
    let v1 = p.build(
        TransactionBuilder::vote(first).nonce(nonce + 1).fee(FEE),
        voter,
    )?;
    let v2 = p.build(
        TransactionBuilder::vote(second).nonce(nonce + 2).fee(FEE),
        voter,
    )?;
    let bytes = p.next(&[v1, v2], 0, None)?.bytes;
    p.job_at_latest(server, "two votes of one sender in a block", vec![bytes])?;
    let to = p.funded(0, 1)?;
    let t = p.pay(voter, to, 100_000_000, 1)?;
    let first = p.next(std::slice::from_ref(&t), 0, None)?.bytes;
    let mut state = p.mirror.clone();
    let decoded = Block::receive(&first, &p.milestones)
        .map_err(|e| Error::Oracle(format!("a built block: {e}")))?;
    state
        .apply_block(&decoded, &p.milestones, &Handlers)
        .map_err(|e| Error::Oracle(format!("a built block was refused: {e}")))?;
    let again = block_on(&state, &p.milestones, p.keys, &[t], 0, None)?.bytes;
    p.job_at_latest(
        server,
        "a transaction forged earlier in the same job",
        vec![first, again],
    )?;
    Ok(())
}

/// A removal of three blocks across the round end at the tip, with pool transactions.
fn removal_across_round_end(p: &mut Pair<'_>, server: &mut OracleServer) -> Result<()> {
    let a = p.funded(1_000_000_000, 0)?;
    let b = p.funded(1_000_000_000, 1)?;
    let to = p.funded(0, 2)?;
    let t1 = p.pay(a, to, 100_000_000, 1)?;
    let b1 = p.next(&[t1], 0, None)?.bytes;
    p.job_at_latest(
        server,
        "a block with a transfer after the round end",
        vec![b1],
    )?;
    let t2 = p.pay(b, to, 100_000_000, 1)?;
    let b2 = p.next(&[t2], 0, None)?.bytes;
    p.job_at_latest(server, "a second block with a transfer", vec![b2])?;
    // A transaction waiting in both pools.
    let pending = p.pay(a, to, 50_000_000, 1)?;
    let now = slot_time(&serialiser::serialise(p.mirror.last_block()), &p.milestones);
    call(
        server,
        "pool.process",
        1,
        &json!({"session": p.session, "entries": [{"hex": hex::encode(pending.bytes())}], "now": now, "channel": "api"}),
    )?;
    let what = "a transaction admitted to the pool";
    match &mut p.rig {
        Ok(rig) => {
            match rig.process_entries(
                Channel::Api,
                vec![Entry::Bytes(pending.bytes().to_vec())],
                now,
            ) {
                Ok(_) => p.record(what, Outcome::Pass),
                Err(problem) => {
                    let outcome = problem.outcome();
                    p.record(what, outcome);
                }
            }
        }
        Err(_) => p.not_run(what),
    }
    p.revert(server, "three blocks removed across the round end", 3)?;
    Ok(())
}

/// A vote in the last block of a round, a revert of two blocks and a competing branch that
/// votes otherwise.
fn vote_at_round_end_fork(p: &mut Pair<'_>, server: &mut OracleServer) -> Result<()> {
    p.revert(server, "back to the block before the round's last", 1)?;
    let voter = p.funded(1_000_000_000, 0)?;
    let mut names: Vec<(u32, String)> = p
        .mirror
        .wallets()
        .delegates()
        .filter_map(|w| w.delegate())
        .filter(|d| d.resigned.is_none())
        .map(|d| (d.rank.unwrap_or(u32::MAX), d.username().to_owned()))
        .collect();
    names.sort();
    let (Some((_, last)), Some((_, before))) = (names.last(), names.iter().rev().nth(1)) else {
        return Err(Error::Oracle("fewer than two delegates".into()));
    };
    let vote = |p: &Pair<'_>, name: &str| -> Result<Transaction> {
        let mut votes = Votes::new();
        votes.insert(name.to_owned(), 10_000);
        let nonce = p.nonce(&voter.address) + 1;
        p.build(TransactionBuilder::vote(votes).nonce(nonce).fee(FEE), voter)
    };
    let v = vote(p, last)?;
    let bytes = p.next(&[v], 0, None)?.bytes;
    p.job_at_latest(server, "a vote in the last block of a round", vec![bytes])?;
    p.revert(server, "two blocks reverted across the round end", 2)?;
    let first = p.next(&[], 1, None)?.bytes;
    let mut state = p.mirror.clone();
    let decoded = Block::receive(&first, &p.milestones)
        .map_err(|e| Error::Oracle(format!("a built block: {e}")))?;
    state
        .apply_block(&decoded, &p.milestones, &Handlers)
        .map_err(|e| Error::Oracle(format!("a built block was refused: {e}")))?;
    let mut votes = Votes::new();
    votes.insert(before.clone(), 10_000);
    let nonce = state
        .wallets()
        .find_by_address(&voter.address)
        .map_or(0, heartwood_state::Wallet::nonce)
        + 1;
    let v = p.build(TransactionBuilder::vote(votes).nonce(nonce).fee(FEE), voter)?;
    let second = block_on(&state, &p.milestones, p.keys, &[v], 0, None)?.bytes;
    p.job_at_latest(
        server,
        "a competing branch that votes otherwise",
        vec![first, second],
    )?;
    Ok(())
}

/// Run one case.
fn run_case(
    case: ProcessorCase,
    corpus: &Corpus,
    keys: &Keys,
    server: &mut OracleServer,
) -> Result<Vec<CaseResult>> {
    let milestones = milestones_with(corpus, &case.milestones(corpus.tip()))?;
    let pool = matches!(case, ProcessorCase::RemovalAcrossRoundEnd);
    let mut p = Pair::open(corpus, keys, server, case.name(), milestones, pool)?;
    if let Err(problem) = &p.rig {
        let outcome = problem.outcome();
        p.record("Heartwood's node at the corpus's tip", outcome);
    }
    match case {
        ProcessorCase::Unchained => unchained(&mut p, server)?,
        ProcessorCase::DoubleForging(depth) => double_forging(&mut p, server, depth)?,
        ProcessorCase::RefusedAfterGap => refused_after_gap(&mut p, server)?,
        ProcessorCase::UnchainedFirst => {
            let two = p.empty_blocks(2)?;
            p.job_at_latest(
                server,
                "a first block that does not chain",
                vec![two[1].clone()],
            )?;
        }
        ProcessorCase::MilestoneCut | ProcessorCase::MilestoneAtLastDownloaded => {
            download_in_jobs(&mut p, server, 6)?;
        }
        ProcessorCase::ConflictingAndForged => conflicting_and_forged(&mut p, server)?,
        ProcessorCase::RemovalAcrossRoundEnd => removal_across_round_end(&mut p, server)?,
        ProcessorCase::FutureSlot => {
            let two = p.empty_blocks(2)?;
            let now = slot_time(&two[0], &p.milestones);
            let blocks = two.into_iter().map(JobBlock::of).collect();
            p.job(
                server,
                "a block in a future slot after an accepted one",
                blocks,
                JobInputs::at(now),
            )?;
        }
        ProcessorCase::RemovedTypeAndRebuild => removed_type_and_rebuild(&mut p, server)?,
        ProcessorCase::VoteAtRoundEndFork => vote_at_round_end_fork(&mut p, server)?,
    }
    call(server, "chain.close", 1, &json!({"session": p.session}))?;
    if let Some(dir) = std::env::var_os(RECORD_DIR) {
        let path = std::path::Path::new(&dir).join(format!("{}.json", p.name));
        let text = json!({"case": p.name, "steps": p.answers}).to_string();
        std::fs::write(&path, text + "\n").map_err(|e| Error::io(&path, e))?;
    }
    Ok(p.results)
}

/// Run every synthetic processor case on `corpus`'s chain.
pub fn run(corpus: &Corpus, oracle: &mut OracleServer) -> Result<Vec<CaseResult>> {
    let keys = corpus_keys(corpus)?;
    let mut out = Vec::new();
    for case in ProcessorCase::ALL {
        out.extend(run_case(case, corpus, &keys, oracle)?);
    }
    Ok(out)
}
