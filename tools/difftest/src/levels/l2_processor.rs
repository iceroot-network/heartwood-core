//! L2 through the processor: a corpus replayed as a follower imports it, and the job's cases.
//!
//! Each block goes through the download path (a getBlocks-shaped listing, its codec, its reply
//! check and `heartwood_p2p::download::read_listed`), then through the enqueueing's cuts and the
//! job, committed per job to a new store, with a manual clock at each job's last slot. Every
//! result must be accepted, every line must equal the judge's (`ts-digests.jsonl`), every result
//! and job end must be the judge's through the reference's job (`ts-processor.jsonl`), the store
//! anchors must hold, and the reference-chunked and one-block modes must give equal lines.
//! [`job_case`] answers the `chain.job` records.
//!
//! The pieces the replay, the golden vectors and the synthetic cases share:
//!
//! - [`cut_jobs`]: the reference's enqueueing, which cuts a download into jobs;
//! - [`JobBlock`], [`listed`] and [`download`]: a block as a peer lists it, and the download path;
//! - [`JobRig`]: a node's state, store, chain state and pool on which jobs run through
//!   [`heartwood_blockchain::run_job`] and the block processor, each processed block recorded
//!   with its digest line, and the blockchain service's answer to the job's end applied;
//! - [`judge_job`]: a job's outcome against the oracle's `chain.job` answer, with the documented
//!   differences a job can show.

use std::collections::VecDeque;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::{Arc, Mutex, PoisonError};

use heartwood_blockchain::chain_state::ChainState;
use heartwood_blockchain::events::NoObserver;
use heartwood_blockchain::pool_task::PoolEnv;
use heartwood_blockchain::{
    BlockProcess, BlockProcessor, BlockProcessorResult, ChainStore, Channel, IncomingBlock, Job,
    JobContext, JobEnd, JobOutcome, PoolTask, ProcessContext, Processed, RestoreMode, RestorePath,
    UnchainedStatus, run_job,
};
use heartwood_crypto::blocks::deserialiser::deserialise_header;
use heartwood_crypto::blocks::serialiser;
use heartwood_crypto::crypto::slots;
use heartwood_crypto::utils::bytes::Reader;
use heartwood_crypto::utils::hex;
use heartwood_crypto::{Block, BlockHeader, BlockId, Milestones, Network, Params, Transaction};
use heartwood_kernel::config::crypto::{self as crypto_files, CryptoFiles, CryptoTexts};
use heartwood_kernel::{ChainLink, ManualClock};
use heartwood_p2p::download::read_listed;
use heartwood_p2p_wire::Amount;
use heartwood_p2p_wire::codecs::blocks::{
    ListedBlock, get_blocks_decode_response, get_blocks_encode_response,
};
use heartwood_p2p_wire::messages::blocks::BlockHeader as ListedHeader;
use heartwood_p2p_wire::replies::check_blocks;
use heartwood_p2p_wire::status::StatusHeaderData;
use heartwood_pool::{DelegateVersions, Entry, Options as PoolOptions, Pool, ProcessorResult};
use heartwood_state::State;
use heartwood_state::digest::{self, DigestLine, Step};
use heartwood_state::machine::Reverted;
use heartwood_state::round_state::MissedBlock;
use heartwood_transactions::Handlers;
use serde_json::{Map, Value, json};

use super::l2::{self, Action};
use crate::compare;
use crate::corpus::{ANCHORS, Corpus, TS_DIGESTS, TS_PROCESSOR};
use crate::digests;
use crate::errors::{Error, Result};
use crate::oracle::OracleServer;
use crate::report::{CaseResult, Outcome, Report};
use crate::store::{self, ScratchDir, StartProblem, StoreNode, guarded};
use crate::subject::{Case, Documented, SubjectError, Verdict};
use crate::subjects::Devnet;

/// How the replay cuts the chain into jobs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Chunks {
    /// As the reference's enqueueing cuts them (150 transactions, 100 blocks, milestone heights).
    Reference,
    /// One block per job.
    One,
}

impl Chunks {
    /// The mode's name in reports.
    pub fn name(self) -> &'static str {
        match self {
            Chunks::Reference => "reference",
            Chunks::One => "1",
        }
    }
}

/// The options of a processor replay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct L2ProcessorOptions {
    /// How the chain is cut into jobs; both modes when `None`.
    pub chunks: Option<Chunks>,
    /// Replay the chain cut at this height.
    pub to: Option<u32>,
    /// Also run the synthetic processor and fork cases (they need the oracle).
    pub synthetic: bool,
}

// ---- the enqueueing -----------------------------------------------------------------------------

/// The transactions after which the enqueueing closes a job.
pub const JOB_TRANSACTIONS: u64 = 150;
/// The most blocks of a job: the reference's `min(maxLastBlocks, 100)`.
pub const JOB_BLOCKS: usize = 100;

/// A block as the enqueueing reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Queued {
    /// The height.
    pub height: u32,
    /// The number of transactions its header states.
    pub transactions: u32,
}

/// The reference's enqueueing (`Blockchain.enqueueBlocks`): the jobs `blocks` are cut into, as
/// indices into `blocks`. A job is closed after the block that brings it to 150 transactions, to
/// `min(max_last_blocks, 100)` blocks, or whose height is the first of the sorted milestone
/// heights at or above `last_downloaded`; that list shifts only when such a cut happens, so a
/// milestone at exactly `last_downloaded` stays first and suppresses every later milestone cut.
/// The remaining blocks form a last job, empty when the last block closed one. No block, no job.
pub fn cut_jobs(
    blocks: &[Queued],
    last_downloaded: u32,
    milestone_heights: &[u32],
    max_last_blocks: usize,
) -> Vec<Vec<usize>> {
    if blocks.is_empty() {
        return Vec::new();
    }
    let mut heights: Vec<u32> = milestone_heights
        .iter()
        .copied()
        .filter(|height| *height >= last_downloaded)
        .collect();
    heights.sort_unstable();
    let mut heights = VecDeque::from(heights);
    let limit = max_last_blocks.min(JOB_BLOCKS);
    let (mut jobs, mut current, mut transactions) = (Vec::new(), Vec::new(), 0u64);
    for (index, block) in blocks.iter().enumerate() {
        current.push(index);
        transactions = transactions.saturating_add(u64::from(block.transactions));
        let milestone = heights
            .front()
            .is_some_and(|height| *height != 0 && *height == block.height);
        if transactions >= JOB_TRANSACTIONS || current.len() >= limit || milestone {
            jobs.push(std::mem::take(&mut current));
            transactions = 0;
            if milestone {
                heights.pop_front();
            }
        }
    }
    jobs.push(current);
    jobs
}

/// The heights of the milestones of `milestones`, as the reference's `getMilestones` lists them.
pub fn milestone_heights(milestones: &Milestones) -> Vec<u32> {
    milestones.all().iter().map(Params::height).collect()
}

// ---- the download path --------------------------------------------------------------------------

/// The address the harness's downloads come from (a documentation address).
const PEER: IpAddr = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1));

/// A block of a job as a peer lists it: the block's bytes, and the transaction count its listing
/// states when that is not the block's own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobBlock {
    /// The full block bytes (header, signature, transactions).
    pub bytes: Vec<u8>,
    /// The transaction count the listing states instead of the header's.
    pub count: Option<u32>,
}

impl JobBlock {
    /// The block `bytes` as they are.
    pub fn of(bytes: Vec<u8>) -> JobBlock {
        JobBlock { bytes, count: None }
    }

    /// The block's header, and each transaction's bytes.
    pub fn parts(&self) -> std::result::Result<(BlockHeader, Vec<Vec<u8>>), String> {
        let mut reader = Reader::new(&self.bytes);
        let header = deserialise_header(&mut reader).map_err(|e| format!("the header: {e}"))?;
        let mut transactions = Vec::new();
        if !reader.is_empty() {
            let mut lengths = Vec::new();
            for _ in 0..header.number_of_transactions {
                let bytes = reader
                    .array::<4>()
                    .map_err(|_| "a transaction length is cut".to_owned())?;
                lengths.push(u32::from_le_bytes(bytes) as usize);
            }
            for length in lengths {
                let bytes = reader
                    .bytes(length)
                    .map_err(|_| "a transaction is cut".to_owned())?;
                transactions.push(bytes.to_vec());
            }
        }
        Ok((header, transactions))
    }

    /// The block's height and id (the id of its header).
    pub fn link(&self) -> std::result::Result<ChainLink, String> {
        let (header, _) = self.parts()?;
        Ok(ChainLink::of_header(&header, serialiser::block_id(&header)))
    }
}

/// `block` as a getBlocks reply lists it: the header's fields, its three amounts and its
/// transactions' bytes, with the stated transaction count of `block.count` when it has one.
pub fn listed(block: &JobBlock) -> std::result::Result<ListedBlock, String> {
    let (header, transactions) = block.parts()?;
    let fields = StatusHeaderData::from_header(&header);
    Ok(ListedBlock {
        header: ListedHeader {
            id: Some(fields.id),
            version: Some(fields.version),
            timestamp: Some(fields.timestamp),
            previous_block: Some(fields.previous_block),
            height: Some(fields.height),
            number_of_transactions: Some(block.count.unwrap_or(fields.number_of_transactions)),
            payload_length: Some(fields.payload_length),
            payload_hash: Some(fields.payload_hash),
            generator_public_key: Some(fields.generator_public_key),
            block_signature: Some(fields.block_signature),
            ..ListedHeader::default()
        },
        total_amount: Amount::from(fields.total_amount),
        total_fee: Amount::from(fields.total_fee),
        reward: Amount::from(fields.reward),
        transactions,
    })
}

/// The download of `blocks` from a peer, as the client reads a getBlocks reply: the reply's codec
/// both ways, its check, and each block's transactions read as the reference's `fromBytesUnsafe`
/// reads them under `params`. A refusal refuses the whole reply, as the client does.
pub fn download(
    blocks: &[JobBlock],
    params: &Params,
) -> std::result::Result<Vec<IncomingBlock>, String> {
    let listing = blocks
        .iter()
        .map(listed)
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let reply = get_blocks_encode_response(&listing);
    let mut decoded =
        get_blocks_decode_response(&reply).map_err(|e| format!("the reply's codec: {e}"))?;
    check_blocks(&mut decoded).map_err(|e| format!("the reply check: {e:?}"))?;
    decoded
        .into_iter()
        .map(|listed| {
            read_listed(listed, PEER, false, params)
                .map(IncomingBlock::downloaded)
                .map_err(|e| format!("the download: {e}"))
        })
        .collect()
}

/// Whether the transaction `bytes` are of a removed type (type group 1, types 0, 3, 4, 5, 8, 9
/// and 10), read on the bytes themselves.
pub fn is_removed_type(bytes: &[u8]) -> bool {
    let group = bytes
        .get(3..7)
        .and_then(|b| <[u8; 4]>::try_from(b).ok())
        .map(u32::from_le_bytes);
    let kind = bytes
        .get(7..9)
        .and_then(|b| <[u8; 2]>::try_from(b).ok())
        .map(u16::from_le_bytes);
    bytes.first() == Some(&0xff)
        && group == Some(1)
        && matches!(kind, Some(0 | 3 | 4 | 5 | 8 | 9 | 10))
}

// ---- the rig ------------------------------------------------------------------------------------

/// What a job runs with besides its blocks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobInputs {
    /// The clock, in milliseconds since the Unix epoch.
    pub now_ms: i64,
    /// The state machine's state (the blockchain service broadcasts only in `newBlock`).
    pub machine_state: String,
    /// Whether the blockchain has started.
    pub started: bool,
    /// Whether the last downloaded block is set to the job's last block first, as a download
    /// sets it before it enqueues.
    pub downloaded: bool,
    /// The block (its bytes) the last downloaded block is set to first instead: a download's
    /// last block, before the first of its jobs.
    pub last_downloaded: Option<Vec<u8>>,
}

impl JobInputs {
    /// A downloaded job in `newBlock` on a started node, at `now_ms`.
    pub fn at(now_ms: i64) -> JobInputs {
        JobInputs {
            now_ms,
            machine_state: "newBlock".into(),
            started: true,
            downloaded: true,
            last_downloaded: None,
        }
    }

    /// The inputs of an oracle `chain.job` job (`now`, `machineState`, `started`,
    /// `downloaded`), with the oracle's defaults.
    pub fn from_json(job: &Value) -> std::result::Result<JobInputs, String> {
        let now_ms = job
            .get("now")
            .and_then(Value::as_i64)
            .ok_or("a job needs now, in milliseconds")?;
        let flag = |key: &str| job.get(key).map_or(Some(true), Value::as_bool);
        Ok(JobInputs {
            now_ms,
            machine_state: job
                .get("machineState")
                .map_or(Some("newBlock"), Value::as_str)
                .ok_or("machineState must be a state's name")?
                .to_owned(),
            started: flag("started").ok_or("started must be a boolean")?,
            downloaded: flag("downloaded").ok_or("downloaded must be a boolean")?,
            last_downloaded: match job.get("lastDownloaded") {
                None => None,
                Some(text) => Some(
                    hex::decode(text.as_str().ok_or("lastDownloaded must be hex")?)
                        .map_err(|e| format!("lastDownloaded: {e}"))?,
                ),
            },
        })
    }

    /// The inputs as an oracle `chain.job` job takes them, without the blocks.
    pub fn to_json(&self) -> Value {
        let mut out = json!({
            "now": self.now_ms,
            "machineState": self.machine_state,
            "started": self.started,
            "downloaded": self.downloaded,
        });
        if let Some(bytes) = &self.last_downloaded {
            out["lastDownloaded"] = json!(hex::encode(bytes));
        }
        out
    }
}

/// A block the processor answered in a job, as the rig recorded it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seen {
    /// The block's height.
    pub height: u32,
    /// The block's id.
    pub id: BlockId,
    /// What the processor answered.
    pub processed: Processed,
    /// The digest line after the block, when it was accepted.
    pub line: Option<DigestLine>,
}

/// A job's run on the rig.
#[derive(Debug, Clone)]
pub enum RigJob {
    /// The download refused the listing: no job ran.
    Refused(String),
    /// The job ran.
    Ran {
        /// What the job answered.
        outcome: JobOutcome,
        /// Each block the processor answered, in order.
        seen: Vec<Seen>,
        /// The job's answer in the oracle's `chain.job` shape.
        value: Value,
    },
}

/// Why a rig could not run a job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RigProblem {
    /// A component is still a stub.
    Unbuilt(String),
    /// The job failed as a node's job fails (a store or pool failure, a failed revert).
    Failed(String),
}

impl RigProblem {
    /// The outcome a report gives this problem.
    pub fn outcome(&self) -> Outcome {
        match self {
            RigProblem::Unbuilt(message) => Outcome::Skipped(format!("not built yet: {message}")),
            RigProblem::Failed(message) => Outcome::HarnessError(message.clone()),
        }
    }
}

impl From<StartProblem> for RigProblem {
    fn from(problem: StartProblem) -> RigProblem {
        match problem {
            StartProblem::Unbuilt(message) => RigProblem::Unbuilt(message),
            StartProblem::Failed(message) => RigProblem::Failed(message),
        }
    }
}

/// The block processor with a recorder around it: each answer, and the digest line after each
/// accepted block with the reward read before it, in the steps the job's blocks are given.
struct Recorder<'m> {
    milestones: &'m Milestones,
    log: Mutex<RecorderLog>,
}

struct RecorderLog {
    seen: Vec<Seen>,
    steps: VecDeque<Step>,
    chain: [u8; 32],
}

impl BlockProcess for Recorder<'_> {
    fn process(&self, ctx: &mut ProcessContext<'_>, block: &Block) -> Processed {
        let reward = ctx
            .state
            .reward_for_block_in_round(
                block.height(),
                &block.header().generator_public_key,
                ctx.milestones,
            )
            .ok()
            .map(|r| r.reward);
        let processed = BlockProcessor.process(ctx, block);
        let mut log = self.log.lock().unwrap_or_else(PoisonError::into_inner);
        let step = log.steps.pop_front().unwrap_or(Step::Apply);
        let line = (processed.result == BlockProcessorResult::Accepted).then(|| {
            let missed = processed.applied.as_ref().map_or(&[][..], |a| &a.missed);
            let line =
                digest::height_digest(ctx.state, self.milestones, step, missed, reward, &log.chain);
            log.chain = line.chain_digest;
            line
        });
        log.seen.push(Seen {
            height: block.height(),
            id: *block.id(),
            processed: processed.clone(),
            line,
        });
        processed
    }
}

/// A node on which jobs run: its state, store, chain state and pool, a manual clock, and every
/// digest line since its genesis block.
pub struct JobRig {
    scratch: ScratchDir,
    state: State,
    store: ChainStore,
    chain: ChainState,
    pool: PoolTask,
    milestones: Milestones,
    network: Network,
    clock: ManualClock,
    lines: Vec<DigestLine>,
}

impl std::fmt::Debug for JobRig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JobRig")
            .field("height", &self.state.height())
            .field("lines", &self.lines.len())
            .finish_non_exhaustive()
    }
}

/// The crypto files of `texts`, loaded by the kernel's loader.
pub fn crypto_of(texts: &CryptoTexts) -> std::result::Result<CryptoFiles, StartProblem> {
    guarded(|| crypto_files::parse(texts))?
        .map_err(|e| StartProblem::Failed(format!("the crypto files are refused: {e}")))
}

/// The crypto files of an oracle `crypto` object ({network, milestones, genesisBlock,
/// exceptions}), with the milestone file `milestones` instead when given.
pub fn crypto_of_json(
    crypto: &Value,
    milestones: Option<&str>,
) -> std::result::Result<CryptoFiles, StartProblem> {
    let text = |key: &str| crypto.get(key).map(|v| v.to_string());
    crypto_of(&CryptoTexts {
        network: text("network"),
        milestones: milestones.map(str::to_owned).or_else(|| text("milestones")),
        genesis: text("genesisBlock"),
        exceptions: text("exceptions"),
    })
}

impl JobRig {
    /// A node at the genesis block of `files`, as its first start leaves it, its store and pool
    /// in a new scratch directory.
    pub fn start(files: &CryptoFiles) -> std::result::Result<JobRig, StartProblem> {
        let milestones = files.milestones.clone();
        let network = files.network.clone();
        let scratch = ScratchDir::new("l2-processor")
            .map_err(|e| StartProblem::Failed(format!("no scratch directory: {e}")))?;
        let node = StoreNode::start(scratch.path(), files, RestoreMode::Auto)?;
        let (store, state, path) = node.into_parts();
        if path != RestorePath::FirstStart {
            return Err(StartProblem::Failed(format!(
                "a new store started by {path:?}, not as a first start"
            )));
        }
        let options = PoolOptions::defaults(&|_| None, scratch.path(), 1);
        let pool = guarded(|| Pool::open(options, None))?
            .map_err(|e| StartProblem::Failed(format!("the pool does not open: {e}")))?;
        let pool = PoolTask::new(pool, DelegateVersions::new());
        let chain = ChainState::at(Arc::new(state.last_block().clone()));
        let first = digest::height_digest(&state, &milestones, Step::Genesis, &[], None, &[0; 32]);
        Ok(JobRig {
            scratch,
            state,
            store,
            chain,
            pool,
            milestones,
            network,
            clock: ManualClock::new(0),
            lines: vec![first],
        })
    }

    /// Apply `blocks` as a node that already has them: each applied to the state with its line,
    /// all committed in one chunk, and the chain state at the new tip.
    pub fn extend(&mut self, blocks: &[Block]) -> std::result::Result<(), StartProblem> {
        let mut applied = Vec::new();
        for block in blocks {
            let header = block.header();
            let reward = self
                .state
                .reward_for_block_in_round(
                    header.height,
                    &header.generator_public_key,
                    &self.milestones,
                )
                .ok()
                .map(|r| r.reward);
            let done = self
                .state
                .apply_block(block, &self.milestones, &Handlers)
                .map_err(|e| {
                    StartProblem::Failed(format!("block {} was refused: {e}", block.height()))
                })?;
            self.push(Step::Apply, &done.missed, reward);
            applied.push((self.state.clone(), block.clone(), done));
            self.chain.set_last_block(Arc::new(block.clone()));
        }
        let milestones = &self.milestones;
        let written = guarded(|| -> std::result::Result<(), String> {
            let mut writer = self.store.chunk().map_err(|e| e.to_string())?;
            for (state, block, done) in &applied {
                writer
                    .applied(state, block, done, milestones)
                    .map_err(|e| format!("block {}: {e}", block.height()))?;
            }
            writer.commit().map_err(|e| e.to_string())
        })?;
        written.map_err(|e| StartProblem::Failed(format!("the store refused a chunk: {e}")))?;
        self.chain.last_stored_height = self.state.height();
        self.chain.reset_last_downloaded();
        Ok(())
    }

    fn push(&mut self, step: Step, missed: &[MissedBlock], reward: Option<u64>) {
        let previous = self.lines.last().map_or([0; 32], |l| l.chain_digest);
        let line = digest::height_digest(
            &self.state,
            &self.milestones,
            step,
            missed,
            reward,
            &previous,
        );
        self.lines.push(line);
    }

    /// The state.
    pub fn state(&self) -> &State {
        &self.state
    }

    /// The chain state.
    pub fn chain(&self) -> &ChainState {
        &self.chain
    }

    /// The store.
    pub fn store(&self) -> &ChainStore {
        &self.store
    }

    /// The pool and the online record.
    pub fn pool(&mut self) -> &mut PoolTask {
        &mut self.pool
    }

    /// The milestones.
    pub fn milestones(&self) -> &Milestones {
        &self.milestones
    }

    /// Every digest line since the genesis block.
    pub fn lines(&self) -> &[DigestLine] {
        &self.lines
    }

    /// The scratch directory of the store and the pool.
    pub fn dir(&self) -> &std::path::Path {
        self.scratch.path()
    }

    /// Set the last downloaded block.
    pub fn set_last_downloaded(&mut self, link: Option<ChainLink>) {
        self.chain.last_downloaded = link;
    }

    /// Process `entries` with the pool's `channel` processor at `now_ms`, as the node does with a
    /// batch: the processor's result.
    pub fn process_entries(
        &mut self,
        channel: Channel,
        entries: Vec<Entry>,
        now_ms: i64,
    ) -> std::result::Result<ProcessorResult, RigProblem> {
        self.clock.set(now_ms);
        let env = PoolEnv {
            milestones: &self.milestones,
            network: &self.network,
            clock: &self.clock,
        };
        let (state, pool) = (&self.state, &mut self.pool);
        guarded(|| pool.process(channel, state, &env, entries))
            .map_err(|store::Unbuilt(m)| RigProblem::Unbuilt(m))?
            .map(|(result, _)| result)
            .map_err(|e| RigProblem::Failed(format!("the pool's storage: {e}")))
    }

    /// Re-add `removed`, the transactions of removed blocks in chain order, at `now_ms`, as the
    /// node does after a removal.
    pub fn readd_removed(
        &mut self,
        removed: &[Transaction],
        now_ms: i64,
    ) -> std::result::Result<(), RigProblem> {
        self.clock.set(now_ms);
        let env = PoolEnv {
            milestones: &self.milestones,
            network: &self.network,
            clock: &self.clock,
        };
        let (state, pool) = (&self.state, &mut self.pool);
        guarded(|| pool.readd_removed(state, &env, removed))
            .map_err(|store::Unbuilt(m)| RigProblem::Unbuilt(m))?
            .map(|_| ())
            .map_err(|e| RigProblem::Failed(format!("the pool's storage: {e}")))
    }

    /// The pool's transactions from the highest priority, by id.
    pub fn pool_ids(&self) -> std::result::Result<Vec<String>, RigProblem> {
        let view =
            guarded(|| self.pool.view()).map_err(|store::Unbuilt(m)| RigProblem::Unbuilt(m))?;
        Ok(view.by_priority.iter().map(|t| t.id().to_hex()).collect())
    }

    /// Revert the last block as a node's removal does, one block at a time: the state, the store
    /// (one chunk), the chain state at the new tip, the last downloaded block there, and a
    /// revert line.
    pub fn revert(&mut self) -> std::result::Result<Reverted, String> {
        let reverted = self
            .state
            .revert_block(&self.milestones, &Handlers)
            .map_err(|e| format!("the revert was refused: {e}"))?;
        self.push(Step::Revert, &[], None);
        let (state, milestones) = (&self.state, &self.milestones);
        let written = guarded(|| -> std::result::Result<(), String> {
            let mut writer = self.store.chunk().map_err(|e| e.to_string())?;
            writer
                .reverted(state, &reverted, milestones)
                .map_err(|e| e.to_string())?;
            writer.commit().map_err(|e| e.to_string())
        })
        .map_err(|store::Unbuilt(m)| m)?;
        written.map_err(|e| format!("the store refused the revert: {e}"))?;
        self.chain
            .set_last_block(Arc::new(self.state.last_block().clone()));
        self.chain.last_stored_height = self.state.height();
        self.chain.reset_last_downloaded();
        Ok(reverted)
    }

    /// Run `blocks` as one job with `inputs`: the download of their listing, then
    /// [`heartwood_blockchain::run_job`] with the block processor and the rig's clock at
    /// `inputs.now_ms`, then the blockchain service's answer to the job's end (`Fork`: the forked
    /// block set and the last downloaded block at the tip; `Clear` and `ForkCheck`: the last
    /// downloaded block at the tip). `steps` are the digest steps of the blocks in order (apply
    /// for those beyond it).
    pub fn run(
        &mut self,
        blocks: &[JobBlock],
        inputs: &JobInputs,
        steps: &[Step],
    ) -> std::result::Result<RigJob, RigProblem> {
        let params = self.milestones.at(self.state.height().saturating_add(1));
        let incoming = match download(blocks, params) {
            Ok(incoming) => incoming,
            Err(why) => return Ok(RigJob::Refused(why)),
        };
        if let Some(bytes) = &inputs.last_downloaded {
            let link = JobBlock::of(bytes.clone()).link();
            self.chain.last_downloaded = Some(link.map_err(RigProblem::Failed)?);
        } else if inputs.downloaded
            && let Some(last) = blocks.last()
        {
            self.chain.last_downloaded = Some(last.link().map_err(RigProblem::Failed)?);
        }
        self.chain.started = inputs.started;
        self.clock.set(inputs.now_ms);
        let recorder = Recorder {
            milestones: &self.milestones,
            log: Mutex::new(RecorderLog {
                seen: Vec::new(),
                steps: steps.iter().copied().collect(),
                chain: self.lines.last().map_or([0; 32], |l| l.chain_digest),
            }),
        };
        let outcome = {
            let mut observer = NoObserver;
            let mut ctx = JobContext {
                state: &mut self.state,
                store: &mut self.store,
                chain: &mut self.chain,
                pool: &mut self.pool,
                processor: &recorder,
                handlers: &Handlers,
                milestones: &self.milestones,
                clock: &self.clock,
                observer: &mut observer,
            };
            let job = Job { blocks: incoming };
            guarded(|| run_job(&mut ctx, job))
                .map_err(|store::Unbuilt(m)| RigProblem::Unbuilt(m))?
                .map_err(|e| RigProblem::Failed(format!("the job failed: {e}")))?
        };
        let RecorderLog { seen, .. } = recorder
            .log
            .into_inner()
            .unwrap_or_else(PoisonError::into_inner);
        self.lines
            .extend(seen.iter().filter_map(|s| s.line.clone()));
        match &outcome.end {
            JobEnd::Fork(block) => {
                self.chain.forked_block = Some(Arc::clone(block));
                self.chain.reset_last_downloaded();
            }
            JobEnd::Clear | JobEnd::ForkCheck(_) => self.chain.reset_last_downloaded(),
            JobEnd::Broadcast(_) | JobEnd::Dropped => {}
        }
        let value = job_value(&JobParts {
            blocks,
            inputs,
            outcome: &outcome,
            seen: &seen,
            chain: &self.chain,
            tip: (self.state.height(), *self.state.last_block().id()),
            milestones: &self.milestones,
        });
        Ok(RigJob::Ran {
            outcome,
            seen,
            value,
        })
    }
}

/// What a job's answer is made of: the job's blocks and inputs, what it answered, each processed
/// block, and the chain state and tip after it.
#[derive(Debug, Clone, Copy)]
pub struct JobParts<'a> {
    /// The job's blocks.
    pub blocks: &'a [JobBlock],
    /// The job's inputs.
    pub inputs: &'a JobInputs,
    /// What the job answered.
    pub outcome: &'a JobOutcome,
    /// Each block the processor answered.
    pub seen: &'a [Seen],
    /// The chain state after the job and the blockchain service's answer to its end.
    pub chain: &'a ChainState,
    /// The tip after the job: its height and id.
    pub tip: (u32, BlockId),
    /// The milestones.
    pub milestones: &'a Milestones,
}

/// A job's answer in the shape of the oracle's `chain.job` job.
pub fn job_value(parts: &JobParts<'_>) -> Value {
    let JobParts {
        blocks,
        inputs,
        outcome,
        seen,
        chain,
        tip,
        milestones,
    } = *parts;
    let link = |height: u32, id: &BlockId| json!({"height": height, "id": id.to_hex()});
    let results: Vec<Value> = seen.iter().map(seen_value).collect();
    let mut out = Map::new();
    out.insert("results".into(), Value::Array(results));
    out.insert("committed".into(), json!(outcome.committed));
    let (end, block) = match &outcome.end {
        JobEnd::Broadcast(b) => ("broadcast", Some(link(b.height(), b.id()))),
        JobEnd::Fork(b) => ("fork", Some(link(b.height(), b.id()))),
        JobEnd::Clear => ("clear", None),
        JobEnd::ForkCheck(_) => ("forkCheck", None),
        JobEnd::Dropped if blocks.is_empty() => ("empty", None),
        JobEnd::Dropped => ("dropped", None),
    };
    out.insert("end".into(), json!(end));
    if let Some(block) = block {
        out.insert("block".into(), block);
    }
    let broadcast = matches!(outcome.end, JobEnd::Broadcast(_))
        && inputs.started
        && inputs.machine_state == "newBlock";
    out.insert("broadcast".into(), json!(broadcast));
    out.insert("clearQueue".into(), json!(outcome.clear_queue));
    let stop = stop_of(blocks, seen, &outcome.end, inputs.now_ms, milestones);
    out.insert("failed".into(), json!(stop == Some(Stop::Failed)));
    if let Some(Stop::FutureSlot(height)) = stop {
        out.insert("futureSlot".into(), json!(height));
    }
    let chain_link = |l: &ChainLink| link(l.height, &l.id);
    out.insert(
        "lastDownloaded".into(),
        chain
            .last_downloaded
            .as_ref()
            .map_or(Value::Null, chain_link),
    );
    out.insert(
        "forked".into(),
        chain
            .forked_block
            .as_ref()
            .map_or(Value::Null, |b| link(b.height(), b.id())),
    );
    out.insert("rollback".into(), json!(chain.blocks_to_rollback));
    let missed: Vec<Value> = seen
        .iter()
        .filter_map(|s| s.processed.applied.as_ref())
        .flat_map(|a| a.missed.iter().map(missed_value))
        .collect();
    out.insert("missed".into(), Value::Array(missed));
    out.insert("tip".into(), link(tip.0, &tip.1));
    Value::Object(out)
}

/// A missed-slot row as the oracle writes it.
pub fn missed_value(row: &MissedBlock) -> Value {
    json!({"timestamp": row.timestamp, "height": row.height, "username": row.username})
}

/// A processor result's name, as the reference's enum names it.
pub fn result_name(result: BlockProcessorResult) -> &'static str {
    match result {
        BlockProcessorResult::Accepted => "Accepted",
        BlockProcessorResult::DiscardedButCanBeBroadcasted => "DiscardedButCanBeBroadcasted",
        BlockProcessorResult::Rejected => "Rejected",
        BlockProcessorResult::Rollback => "Rollback",
        BlockProcessorResult::Reverted => "Reverted",
        BlockProcessorResult::Corrupted => "Corrupted",
    }
}

/// An unchained status's name, as the reference's unchained handler names it.
pub fn unchained_name(status: UnchainedStatus) -> &'static str {
    match status {
        UnchainedStatus::NotReadyToAcceptNewHeight => "NotReadyToAcceptNewHeight",
        UnchainedStatus::AlreadyInBlockchain => "AlreadyInBlockchain",
        UnchainedStatus::EqualToLastBlock => "EqualToLastBlock",
        UnchainedStatus::GeneratorMismatch => "GeneratorMismatch",
        UnchainedStatus::DoubleForging => "DoubleForging",
        UnchainedStatus::InvalidTimestamp => "InvalidTimestamp",
    }
}

/// A processed block in the shape of the oracle's `results` entries.
pub fn seen_value(seen: &Seen) -> Value {
    let mut out = Map::new();
    out.insert("height".into(), json!(seen.height));
    out.insert("id".into(), json!(seen.id.to_hex()));
    out.insert("result".into(), json!(result_name(seen.processed.result)));
    if let Some(status) = seen.processed.unchained {
        out.insert("unchained".into(), json!(unchained_name(status)));
    }
    let missed: Vec<Value> = seen
        .processed
        .applied
        .as_ref()
        .map(|a| a.missed.iter().map(missed_value).collect())
        .unwrap_or_default();
    if !missed.is_empty() {
        out.insert("missed".into(), Value::Array(missed));
    }
    if let Some(line) = &seen.line {
        out.insert("line".into(), digests::line_value(line));
    }
    Value::Object(out)
}

/// Why a job's loop stopped before its last block without a result for the block it stopped at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stop {
    /// The block at this height takes a future slot.
    FutureSlot(u32),
    /// The block could not be rebuilt or processed: the job's catch around its loop.
    Failed,
}

/// Where a job's loop stopped without a result: past the answered blocks, at a block that the
/// loop reached (none answered yet, or the last answer was an acceptance), either a future slot
/// against the clock at `now_ms` or a failure. `None` when the loop answered every block, broke
/// on a result, or the job never reached its loop (dropped, or a first block that does not
/// chain to the tip).
pub fn stop_of(
    blocks: &[JobBlock],
    seen: &[Seen],
    end: &JobEnd,
    now_ms: i64,
    milestones: &Milestones,
) -> Option<Stop> {
    if matches!(end, JobEnd::Dropped | JobEnd::ForkCheck(_)) {
        return None;
    }
    let next = blocks.get(seen.len())?;
    if seen
        .last()
        .is_some_and(|s| s.processed.result != BlockProcessorResult::Accepted)
    {
        return None;
    }
    let (header, _) = next.parts().ok()?;
    let params = milestones.at(header.height);
    let epoch = slots::epoch_unix_ms(params.epoch())?;
    let block_time = i64::try_from(params.block_time()).ok()?.max(1);
    let now = slots::epoch_time(now_ms, epoch);
    let slot = |seconds: i64| seconds.div_euclid(block_time);
    if slot(i64::from(header.timestamp)) > slot(now) {
        Some(Stop::FutureSlot(header.height))
    } else {
        Some(Stop::Failed)
    }
}

// ---- judging a job ------------------------------------------------------------------------------

/// Judge Heartwood's run of a job against the oracle's `chain.job` answer `oracle`. The run must
/// equal the answer, except where a documented difference explains it:
///
/// - the reference refuses a block with a transaction of a removed type when it applies it,
///   where Heartwood's download refuses the listing (the transaction bytes are checked);
/// - the reference keeps the missed-slot rows of a block it refused in its apply, where
///   Heartwood records none (the refusal must be Heartwood's too, and the rows must be the
///   reference's rows of that block only).
pub fn judge_job(blocks: &[JobBlock], oracle: &Value, ours: &RigJob) -> Outcome {
    match ours {
        RigJob::Refused(why) => {
            let removed = blocks.iter().any(|b| {
                b.parts()
                    .is_ok_and(|(_, transactions)| transactions.iter().any(|t| is_removed_type(t)))
            });
            if removed {
                Outcome::Documented(Documented::RemovedType.reason().to_owned())
            } else {
                Outcome::VerdictMismatch {
                    oracle: format!("a job ending {}", oracle["end"]),
                    heartwood: format!("the download refused the listing: {why}"),
                }
            }
        }
        RigJob::Ran { value, seen, .. } => {
            let differences = compare::diff(oracle, value);
            if differences.is_empty() {
                return Outcome::Pass;
            }
            if refused_rows_only(oracle, value, seen) {
                return Outcome::Documented(
                    Documented::MissedRowsOfRefusedBlock.reason().to_owned(),
                );
            }
            Outcome::Mismatch(differences)
        }
    }
}

/// Whether `oracle` and `ours` differ only by the missed-slot rows the reference recorded for
/// blocks that both refused in a way Heartwood records no rows for: the oracle's answer without
/// those rows (in the block's result and in the job's list) equals Heartwood's.
pub fn refused_rows_only(oracle: &Value, ours: &Value, seen: &[Seen]) -> bool {
    let Some(results) = oracle["results"].as_array() else {
        return false;
    };
    let mut adjusted = oracle.clone();
    let mut excluded = Vec::new();
    for (index, result) in results.iter().enumerate() {
        let refused = result["result"] == "Rejected"
            && seen
                .get(index)
                .is_some_and(|s| s.processed.result == BlockProcessorResult::Rejected);
        if refused && let Some(rows) = result["missed"].as_array() {
            excluded.extend(rows.iter().cloned());
            if let Some(entry) = adjusted["results"][index].as_object_mut() {
                entry.shift_remove("missed");
            }
        }
    }
    if excluded.is_empty() {
        return false;
    }
    if let Some(rows) = adjusted["missed"].as_array_mut() {
        for row in &excluded {
            if let Some(at) = rows.iter().position(|r| r == row) {
                rows.remove(at);
            }
        }
    }
    compare::diff(&adjusted, ours).is_empty()
}

// ---- the replay ---------------------------------------------------------------------------------

/// A block of the replay: its height and the step its line is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Planned {
    /// The height.
    pub height: u32,
    /// Apply or reapply.
    pub step: Step,
}

/// A step of a processor replay: a download of consecutive blocks, or a revert of the last block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Download {
    /// Blocks downloaded together, in height order.
    Blocks(Vec<Planned>),
    /// Revert the last block.
    Revert,
}

/// The replay's downloads: each run of applies and re-applies between two reverts is one download,
/// as a node downloads again from its new tip after a rollback.
pub fn downloads(actions: &[Action]) -> Vec<Download> {
    let mut out = Vec::new();
    let mut run = Vec::new();
    for action in actions {
        match *action {
            Action::Revert => {
                if !run.is_empty() {
                    out.push(Download::Blocks(std::mem::take(&mut run)));
                }
                out.push(Download::Revert);
            }
            Action::Apply(height) => run.push(Planned {
                height,
                step: Step::Apply,
            }),
            Action::Reapply(height) => run.push(Planned {
                height,
                step: Step::Reapply,
            }),
        }
    }
    if !run.is_empty() {
        out.push(Download::Blocks(run));
    }
    out
}

/// A processor replay of a corpus: its lines, each block's result and job end, and the problems.
#[derive(Debug)]
pub struct ProcessorReplay {
    /// The job cut.
    pub chunks: Chunks,
    /// The rig, for its store's rows.
    pub rig: JobRig,
    /// Each processed block: its height, step, result and the end of its job.
    pub results: Vec<(u32, Step, &'static str, &'static str)>,
    /// What went wrong, with the height it went wrong at.
    pub problems: Vec<(u32, String)>,
}

/// The slot time of `header` in milliseconds since the Unix epoch.
fn slot_time_ms(header: &BlockHeader, milestones: &Milestones) -> i64 {
    let epoch = slots::epoch_unix_ms(milestones.at(1).epoch()).unwrap_or(0);
    epoch.saturating_add(i64::from(header.timestamp).saturating_mul(1000))
}

fn end_name(end: &JobEnd) -> &'static str {
    match end {
        JobEnd::Broadcast(_) => "broadcast",
        JobEnd::Fork(_) => "fork",
        JobEnd::Clear => "clear",
        JobEnd::ForkCheck(_) => "forkCheck",
        JobEnd::Dropped => "dropped",
    }
}

/// Replay `corpus` through jobs cut as `chunks` says, from a new store: each download's blocks
/// through the download path and the jobs, every job at its last block's slot time. In the
/// reference's cut the last downloaded block is the download's last block before its first job
/// runs, as a node syncs; one block per job is each block's own download.
pub fn replay_through_jobs(
    corpus: &Corpus,
    chunks: Chunks,
) -> std::result::Result<ProcessorReplay, RigProblem> {
    let files = store::crypto_files(corpus)?;
    let mut rig = JobRig::start(&files)?;
    let heights = milestone_heights(&corpus.milestones);
    let mut results = Vec::new();
    let mut problems = Vec::new();
    for download in downloads(&l2::actions(corpus)) {
        let planned = match download {
            Download::Revert => match rig.revert() {
                Ok(_) => continue,
                Err(why) => {
                    problems.push((rig.state().height(), why));
                    break;
                }
            },
            Download::Blocks(planned) => planned,
        };
        match run_download(corpus, &mut rig, &planned, chunks, &heights, &mut results) {
            Ok(None) => {}
            Ok(Some(problem)) => {
                problems.push(problem);
                break;
            }
            Err(problem @ RigProblem::Unbuilt(_)) => return Err(problem),
            Err(RigProblem::Failed(why)) => {
                problems.push((rig.state().height(), why));
                break;
            }
        }
    }
    Ok(ProcessorReplay {
        chunks,
        rig,
        results,
        problems,
    })
}

/// One download of the replay: its jobs in turn. The first problem, with its height.
fn run_download(
    corpus: &Corpus,
    rig: &mut JobRig,
    planned: &[Planned],
    chunks: Chunks,
    heights: &[u32],
    results: &mut Vec<(u32, Step, &'static str, &'static str)>,
) -> std::result::Result<Option<(u32, String)>, RigProblem> {
    let mut blocks = Vec::new();
    let mut queued = Vec::new();
    for p in planned {
        let Some(b) = corpus.block(p.height) else {
            return Ok(Some((
                p.height,
                format!("the corpus has no block {}", p.height),
            )));
        };
        let block = JobBlock::of(b.bytes.clone());
        let header = match block.parts() {
            Ok((header, _)) => header,
            Err(why) => return Ok(Some((p.height, why))),
        };
        queued.push(Queued {
            height: header.height,
            transactions: header.number_of_transactions,
        });
        blocks.push(block);
    }
    let jobs: Vec<Vec<usize>> = match chunks {
        Chunks::One => (0..blocks.len()).map(|i| vec![i]).collect(),
        Chunks::Reference => {
            let last_downloaded = rig
                .chain()
                .last_downloaded
                .as_ref()
                .map_or_else(|| rig.state().height(), |l| l.height);
            let cut = cut_jobs(&queued, last_downloaded, heights, JOB_BLOCKS);
            // The download sets the last downloaded block after the enqueueing, before the jobs.
            if let Some(last) = blocks.last() {
                rig.set_last_downloaded(last.link().ok());
            }
            cut
        }
    };
    for job in jobs {
        let mut job_blocks = Vec::new();
        let mut steps = Vec::new();
        for &i in &job {
            if let (Some(block), Some(p)) = (blocks.get(i), planned.get(i)) {
                job_blocks.push(block.clone());
                steps.push(p.step);
            }
        }
        let now_ms = match job_blocks.last().map(JobBlock::parts) {
            Some(Ok((header, _))) => slot_time_ms(&header, &corpus.milestones),
            _ => 0,
        };
        let inputs = JobInputs {
            now_ms,
            machine_state: "syncing".into(),
            started: true,
            downloaded: chunks == Chunks::One,
            last_downloaded: None,
        };
        let (outcome, seen) = match rig.run(&job_blocks, &inputs, &steps)? {
            RigJob::Refused(why) => {
                let height = queued.first().map_or(0, |q| q.height);
                return Ok(Some((
                    height,
                    format!("the download refused the listing: {why}"),
                )));
            }
            RigJob::Ran { outcome, seen, .. } => (outcome, seen),
        };
        let end = end_name(&outcome.end);
        for (s, step) in seen.iter().zip(&steps) {
            results.push((s.height, *step, result_name(s.processed.result), end));
        }
        let all_accepted = seen.len() == job_blocks.len()
            && seen
                .iter()
                .all(|s| s.processed.result == BlockProcessorResult::Accepted);
        if !all_accepted {
            let answered: Vec<String> = seen
                .iter()
                .map(|s| format!("{} {}", s.height, result_name(s.processed.result)))
                .collect();
            let height = seen.last().map_or(rig.state().height(), |s| s.height);
            return Ok(Some((
                height,
                format!(
                    "a job of {} blocks answered {} ({}), ending {end}",
                    job_blocks.len(),
                    seen.len(),
                    answered.join(", ")
                ),
            )));
        }
    }
    Ok(None)
}

/// The title of the level's reports.
const TITLE: &str = "L2 block-sequence replay through the processor and the job";

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

fn step_label(line: &Value) -> String {
    format!(
        "{} {}",
        line.get("step").and_then(Value::as_str).unwrap_or("?"),
        line.get("h").and_then(Value::as_u64).unwrap_or(0)
    )
}

/// The results of one mode's replay: its problems, each line against the judge's, each block's
/// result and job end against the judge's processor mode, and the store anchors.
fn mode_results(
    corpus: &Corpus,
    replay: &ProcessorReplay,
    oracle_lines: &[Value],
    processor_lines: Option<&[Value]>,
) -> Vec<CaseResult> {
    let mode = replay.chunks.name();
    let name = corpus.name();
    let mut out = Vec::new();
    let file = format!("{name}/{TS_DIGESTS} (jobs: {mode})");
    for (height, message) in &replay.problems {
        out.push(result(
            &file,
            0,
            "job",
            format!("height {height}"),
            u64::from(*height),
            Outcome::HarnessError(message.clone()),
        ));
    }
    let ours: Vec<Value> = replay.rig.lines().iter().map(digests::line_value).collect();
    for i in 0..oracle_lines.len().max(ours.len()) {
        let theirs = oracle_lines.get(i).cloned().unwrap_or(Value::Null);
        let mine = ours.get(i).cloned().unwrap_or(Value::Null);
        let differences = compare::diff(&theirs, &mine);
        let outcome = if differences.is_empty() {
            Outcome::Pass
        } else {
            Outcome::Mismatch(differences)
        };
        let shown = if theirs.is_null() { &mine } else { &theirs };
        let op = shown.get("step").and_then(Value::as_str).unwrap_or("line");
        let height = shown.get("h").and_then(Value::as_u64).unwrap_or(0);
        out.push(result(&file, i + 1, op, step_label(shown), height, outcome));
    }
    let file = format!("{name}/{TS_PROCESSOR} (jobs: {mode})");
    match processor_lines {
        None => out.push(result(
            &file,
            0,
            "processor",
            "the judge's processor results".into(),
            0,
            Outcome::Skipped(format!(
                "{name} was judged without --processor: judge it again with PROCESSOR=1"
            )),
        )),
        Some(lines) => {
            let theirs: Vec<&Value> = lines
                .iter()
                .filter(|l| {
                    l.get("result").is_some()
                        || matches!(l["step"].as_str(), Some("apply" | "reapply"))
                })
                .collect();
            for i in 0..theirs.len().max(replay.results.len()) {
                let (label, outcome, height) = match (theirs.get(i), replay.results.get(i)) {
                    (Some(t), Some((height, step, answer, end))) => {
                        let mut problems = Vec::new();
                        if t["h"].as_u64() != Some(u64::from(*height)) || t["step"] != step.as_str()
                        {
                            problems.push(format!("the judge's step is {}", step_label(t)));
                        }
                        if t["result"] != *answer {
                            problems.push(format!(
                                "the judge's result is {}, Heartwood's {answer}",
                                t["result"]
                            ));
                        }
                        if replay.chunks == Chunks::One && t["end"] != *end {
                            problems.push(format!(
                                "the judge's job ended {}, Heartwood's {end}",
                                t["end"]
                            ));
                        }
                        let outcome = if problems.is_empty() {
                            Outcome::Pass
                        } else {
                            Outcome::HarnessError(problems.join("; "))
                        };
                        (
                            format!("{} {height}", step.as_str()),
                            outcome,
                            u64::from(*height),
                        )
                    }
                    (Some(t), None) => (
                        step_label(t),
                        Outcome::HarnessError("Heartwood processed no such block".into()),
                        t["h"].as_u64().unwrap_or(0),
                    ),
                    (None, Some((height, step, answer, _))) => (
                        format!("{} {height}", step.as_str()),
                        Outcome::HarnessError(format!(
                            "the judge has no such block (Heartwood: {answer})"
                        )),
                        u64::from(*height),
                    ),
                    (None, None) => break,
                };
                out.push(result(&file, i + 1, "result", label, height, outcome));
            }
        }
    }
    let file = format!("{name}/store (jobs: {mode})");
    let tip = replay.rig.state().height();
    let last_round = store::last_round(tip, corpus.revert_plan.round_length);
    match guarded(|| store::read_rows(replay.rig.store().database(), tip, last_round)) {
        Ok(Ok(rows)) => {
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
        }
        Ok(Err(message)) => out.push(result(
            &file,
            0,
            "anchor",
            "the store's rows".into(),
            u64::from(tip),
            Outcome::HarnessError(message),
        )),
        Err(store::Unbuilt(message)) => out.push(result(
            &file,
            0,
            "anchor",
            "the store's rows".into(),
            u64::from(tip),
            Outcome::Skipped(format!("not built yet: {message}")),
        )),
    }
    out
}

/// The judge's own report of its processor mode: whether it ran and held.
fn judge_processor(judge: &Value) -> Option<Outcome> {
    let processor = judge.get("processor")?;
    Some(if processor.get("ok") == Some(&Value::Bool(true)) {
        Outcome::Pass
    } else {
        Outcome::HarnessError(format!(
            "the judge's processor mode failed: {} not accepted, {} lines differ from its plain replay",
            processor["notAccepted"].as_array().map_or(0, Vec::len),
            processor["differences"].as_array().map_or(0, Vec::len)
        ))
    })
}

/// Replay `corpus` through the processor with `options`, and the synthetic cases with `oracle`.
pub fn run(
    corpus: &Corpus,
    options: &L2ProcessorOptions,
    oracle: Option<&mut OracleServer>,
) -> Result<Report> {
    let corpus = &corpus.clone().as_judged(options.to)?;
    let name = corpus.name();
    if options.synthetic {
        let server = oracle
            .ok_or_else(|| Error::Usage("the synthetic processor cases need the oracle".into()))?;
        let results = crate::synthetic_processor::run(corpus, server)?;
        return Ok(Report {
            title: TITLE.to_owned(),
            source: format!("the synthetic processor and fork cases on the tip of {name}"),
            files: 1,
            results,
        });
    }
    let (judge_ok, judge) = l2::judge_report(&corpus.dir)?;
    let oracle_lines = digests::read(&corpus.dir.join(TS_DIGESTS))?;
    let processor_path = corpus.dir.join(TS_PROCESSOR);
    let processor_lines = if processor_path.exists() {
        Some(digests::read(&processor_path)?)
    } else {
        None
    };
    let file = format!("{name}/{ANCHORS}");
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
    if let Some(outcome) = judge_processor(&judge) {
        results.push(result(
            &file,
            0,
            "judge",
            "the processor mode".into(),
            0,
            outcome,
        ));
    }
    let modes = match options.chunks {
        Some(chunks) => vec![chunks],
        None => vec![Chunks::Reference, Chunks::One],
    };
    let mut replays = Vec::new();
    for mode in modes {
        match replay_through_jobs(corpus, mode) {
            Ok(replay) => {
                results.extend(mode_results(
                    corpus,
                    &replay,
                    &oracle_lines,
                    processor_lines.as_deref(),
                ));
                replays.push(replay);
            }
            Err(problem) => results.push(result(
                &format!("{name}/{TS_DIGESTS} (jobs: {})", mode.name()),
                0,
                "replay",
                "the replay through the jobs".into(),
                0,
                problem.outcome(),
            )),
        }
    }
    if let [first, second] = replays.as_slice() {
        let same = first.rig.lines() == second.rig.lines()
            && first
                .results
                .iter()
                .map(|r| (r.0, r.1, r.2))
                .eq(second.results.iter().map(|r| (r.0, r.1, r.2)));
        let outcome = if same {
            Outcome::Pass
        } else {
            Outcome::HarnessError(
                "the reference's job cut and one block per job give different lines or results"
                    .into(),
            )
        };
        results.push(result(
            &format!("{name}/jobs"),
            0,
            "chunks",
            "the reference's job cut equals one block per job".into(),
            u64::from(first.rig.state().height()),
            outcome,
        ));
    }
    Ok(Report {
        title: TITLE.to_owned(),
        source: format!(
            "judge of {name}: {} lines, {}",
            oracle_lines.len(),
            if processor_lines.is_some() {
                "with its processor results"
            } else {
                "without its processor results"
            }
        ),
        files: 1,
        results,
    })
}

// ---- the golden vectors -------------------------------------------------------------------------

/// A job's blocks as an oracle `chain.job` job gives them: block bytes in hex, or `{hex, set}`
/// with the transaction count the listing states otherwise.
pub fn job_blocks(hexes: &Value) -> std::result::Result<Vec<JobBlock>, String> {
    let items = hexes.as_array().ok_or("hexes must be a list")?;
    items
        .iter()
        .map(|item| {
            let (text, set) = match item {
                Value::String(text) => (text.as_str(), None),
                Value::Object(o) => (
                    o.get("hex")
                        .and_then(Value::as_str)
                        .ok_or("a block needs hex")?,
                    o.get("set"),
                ),
                _ => return Err("a block is hex, or {hex, set}".to_owned()),
            };
            let bytes = hex::decode(text).map_err(|e| format!("a block's hex: {e}"))?;
            let count = match set {
                None => None,
                Some(set) => {
                    let object = set.as_object().ok_or("set must be an object")?;
                    if object.keys().any(|k| k != "numberOfTransactions") {
                        return Err("set holds only numberOfTransactions".into());
                    }
                    object
                        .get("numberOfTransactions")
                        .map(|v| {
                            v.as_u64()
                                .and_then(|n| u32::try_from(n).ok())
                                .ok_or("numberOfTransactions must be a count")
                        })
                        .transpose()?
                }
            };
            Ok(JobBlock { bytes, count })
        })
        .collect()
}

/// The heights of a milestone file's entries (JSON text).
fn heights_of_text(text: &str) -> std::result::Result<Vec<u32>, String> {
    let entries: Value = serde_json::from_str(text).map_err(|e| format!("milestones: {e}"))?;
    let mut heights: Vec<u32> = entries
        .as_array()
        .ok_or("milestones must be a list")?
        .iter()
        .map(|entry| {
            entry["height"]
                .as_u64()
                .and_then(|h| u32::try_from(h).ok())
                .ok_or("a milestone needs a height")
        })
        .collect::<std::result::Result<_, _>>()?;
    heights.sort_unstable();
    heights.dedup();
    Ok(heights)
}

/// The enqueueing's cuts of a `chain.job` record's `enqueue`.
fn enqueue_case(
    devnet: &Devnet,
    input: &Value,
    enqueue: &Value,
) -> std::result::Result<Value, SubjectError> {
    let bad = |why: &str| SubjectError::Failed(format!("enqueue: {why}"));
    let blocks = enqueue["blocks"]
        .as_array()
        .ok_or_else(|| bad("blocks must be a list"))?
        .iter()
        .map(|b| {
            let field = |key: &str| b[key].as_u64().and_then(|n| u32::try_from(n).ok());
            match (field("height"), field("numberOfTransactions")) {
                (Some(height), Some(transactions)) => Ok(Queued {
                    height,
                    transactions,
                }),
                _ => Err(bad("a block is {height, numberOfTransactions}")),
            }
        })
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let last_downloaded = enqueue["lastDownloadedHeight"]
        .as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .ok_or_else(|| bad("lastDownloadedHeight must be a height"))?;
    let max_last_blocks = match enqueue.get("maxLastBlocks") {
        None => JOB_BLOCKS,
        Some(n) => n
            .as_u64()
            .and_then(|n| usize::try_from(n).ok())
            .ok_or_else(|| bad("maxLastBlocks must be a count"))?,
    };
    let heights = match input.get("milestones") {
        Some(text) => heights_of_text(
            text.as_str()
                .ok_or_else(|| bad("milestones must be text"))?,
        )
        .map_err(|why| bad(&why))?,
        None => milestone_heights(&devnet.milestones),
    };
    let jobs: Vec<Vec<u32>> = cut_jobs(&blocks, last_downloaded, &heights, max_last_blocks)
        .into_iter()
        .map(|job| {
            job.iter()
                .filter_map(|&i| blocks.get(i).map(|b| b.height))
                .collect()
        })
        .collect();
    Ok(json!({ "jobs": jobs }))
}

/// Whether Heartwood refused the block of `block` in its apply after missed slots, on the state
/// `before` it: the block is the next one on the tip, its header shows missed slots before it,
/// and the state's apply refuses it.
fn refused_in_apply_after_gap(before: &State, block: &JobBlock, milestones: &Milestones) -> bool {
    let Ok(decoded) = Block::receive(&block.bytes, milestones) else {
        return false;
    };
    let header = decoded.header();
    let next = header.height == before.height().saturating_add(1)
        && &header.previous_block == before.last_block().id().as_bytes();
    let gap = next
        && !before
            .detect_missed_blocks(decoded.header(), milestones)
            .is_empty();
    gap && before
        .clone()
        .apply_block(&decoded, milestones, &Handlers)
        .is_err()
}

/// A `chain.job` record's session in one call: the chain definition, its blocks, then each job.
fn chain_case(input: &Value) -> std::result::Result<Verdict, SubjectError> {
    let unbuilt = |problem: StartProblem| match problem {
        StartProblem::Unbuilt(_) => SubjectError::NotImplemented,
        StartProblem::Failed(why) => SubjectError::Failed(why),
    };
    let chain = &input["chain"];
    let files = crypto_of_json(&chain["crypto"], None).map_err(unbuilt)?;
    let mut rig = JobRig::start(&files).map_err(unbuilt)?;
    let prefix = chain["blocks"]
        .as_array()
        .ok_or_else(|| SubjectError::Failed("chain.blocks must be a list".into()))?
        .iter()
        .map(|hex| {
            let bytes = hex::decode(hex.as_str().unwrap_or_default())
                .map_err(|e| SubjectError::Failed(format!("a chain block's hex: {e}")))?;
            Block::receive(&bytes, rig.milestones())
                .map_err(|e| SubjectError::Failed(format!("a chain block: {e}")))
        })
        .collect::<std::result::Result<Vec<_>, _>>()?;
    rig.extend(&prefix).map_err(unbuilt)?;
    let jobs = input["jobs"]
        .as_array()
        .ok_or_else(|| SubjectError::Failed("jobs must be a list".into()))?;
    let mut values = Vec::new();
    let mut documented = None;
    for job in jobs {
        let blocks = job_blocks(&job["hexes"]).map_err(SubjectError::Failed)?;
        let inputs = JobInputs::from_json(job).map_err(SubjectError::Failed)?;
        let before = rig.state().clone();
        let ran = rig
            .run(&blocks, &inputs, &[])
            .map_err(|problem| match problem {
                RigProblem::Unbuilt(_) => SubjectError::NotImplemented,
                RigProblem::Failed(why) => SubjectError::Failed(why),
            })?;
        match ran {
            RigJob::Refused(why) => {
                let removed = blocks.iter().any(|b| {
                    b.parts()
                        .is_ok_and(|(_, txs)| txs.iter().any(|t| is_removed_type(t)))
                });
                documented.get_or_insert((Documented::RemovedType, removed));
                values.push(json!({ "refused": why }));
            }
            RigJob::Ran { value, seen, .. } => {
                let refused = seen.iter().position(|s| {
                    s.processed.result == BlockProcessorResult::Rejected
                        && s.processed.applied.is_none()
                });
                if let Some(at) = refused
                    && value["missed"].as_array().is_some_and(Vec::is_empty)
                    && let Some(block) = blocks.get(at)
                {
                    let mut state = before.clone();
                    let accepted = blocks.iter().take(at).all(|b| {
                        Block::receive(&b.bytes, rig.milestones()).is_ok_and(|d| {
                            state.apply_block(&d, rig.milestones(), &Handlers).is_ok()
                        })
                    });
                    if accepted && refused_in_apply_after_gap(&state, block, rig.milestones()) {
                        documented.get_or_insert((Documented::MissedRowsOfRefusedBlock, true));
                    }
                }
                values.push(value);
            }
        }
    }
    Ok(match documented {
        Some((difference, holds)) => Verdict::RefusedByDesign { difference, holds },
        None => Verdict::Output(json!({ "jobs": values })),
    })
}

/// `chain.job`: the job of a record's blocks on its session, and the enqueueing's cuts.
pub fn job_case(devnet: &Devnet, case: &Case<'_>) -> std::result::Result<Verdict, SubjectError> {
    let input = case.input;
    if let Some(enqueue) = input.get("enqueue") {
        return enqueue_case(devnet, input, enqueue).map(Verdict::Output);
    }
    if input.get("chain").is_some() {
        return chain_case(input);
    }
    Err(SubjectError::Unsupported(
        "a job on an oracle session: the golden vectors run whole sessions".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::corpus::RevertPlan;
    use heartwood_p2p::to_block;
    use std::path::Path;

    fn corpus() -> Corpus {
        Corpus::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("corpus/txgen-one-node")).unwrap()
    }

    /// One of V27's enqueueing records as the reference answered it: the first block's height,
    /// each block's transaction count (or `empty` blocks without any), the last downloaded
    /// height, `maxLastBlocks`, the milestone heights and the sizes of the jobs pushed.
    struct Cut {
        first: u32,
        transactions: &'static [u32],
        empty: usize,
        last_downloaded: u32,
        max_last_blocks: usize,
        milestones: &'static [u32],
        jobs: &'static [usize],
    }

    /// The devnet's milestone heights.
    const DEVNET: &[u32] = &[1, 2];

    const fn cut(
        first: u32,
        transactions: &'static [u32],
        empty: usize,
        milestones: &'static [u32],
        jobs: &'static [usize],
    ) -> Cut {
        Cut {
            first,
            transactions,
            empty,
            last_downloaded: first - 1,
            max_last_blocks: 100,
            milestones,
            jobs,
        }
    }

    #[test]
    fn the_enqueueing_cuts_as_the_reference_did() {
        let cases = [
            cut(11, &[100, 49, 1, 5], 0, DEVNET, &[3, 1]),
            cut(11, &[100, 49], 0, DEVNET, &[2]),
            cut(11, &[0, 150, 0], 0, DEVNET, &[2, 1]),
            cut(11, &[], 100, DEVNET, &[100, 0]),
            cut(2, &[], 250, DEVNET, &[100, 100, 50]),
            Cut {
                max_last_blocks: 150,
                ..cut(11, &[], 120, DEVNET, &[100, 20])
            },
            cut(11, &[], 5, &[1, 2, 12], &[2, 3]),
            cut(11, &[], 3, &[1, 2, 11], &[1, 2]),
            cut(11, &[], 6, &[1, 2, 12, 14], &[2, 2, 2]),
            cut(11, &[], 6, &[1, 2, 10, 13], &[6]),
            cut(11, &[], 4, &[1, 2, 8, 12], &[2, 2]),
            cut(11, &[150, 0, 0, 0], 0, &[1, 2, 13], &[1, 2, 1]),
            cut(11, &[], 2, &[1, 2, 12], &[2, 0]),
            cut(2, &[], 3, DEVNET, &[3]),
            cut(11, &[], 0, DEVNET, &[]),
        ];
        for (i, c) in cases.iter().enumerate() {
            let counts: Vec<u32> = if c.transactions.is_empty() {
                vec![0; c.empty]
            } else {
                c.transactions.to_vec()
            };
            let blocks: Vec<Queued> = (c.first..)
                .zip(counts)
                .map(|(height, transactions)| Queued {
                    height,
                    transactions,
                })
                .collect();
            let jobs = cut_jobs(&blocks, c.last_downloaded, c.milestones, c.max_last_blocks);
            let sizes: Vec<usize> = jobs.iter().map(Vec::len).collect();
            assert_eq!(sizes, c.jobs, "case {}", i + 1);
            let flat: Vec<usize> = jobs.concat();
            assert_eq!(
                flat,
                (0..blocks.len()).collect::<Vec<_>>(),
                "case {}",
                i + 1
            );
        }
    }

    #[test]
    fn a_replay_downloads_again_after_each_revert() {
        use Action::*;
        let got = downloads(&[
            Apply(2),
            Apply(3),
            Revert,
            Revert,
            Reapply(2),
            Reapply(3),
            Apply(4),
            Revert,
        ]);
        let planned = |height, step| Planned { height, step };
        assert_eq!(
            got,
            vec![
                Download::Blocks(vec![planned(2, Step::Apply), planned(3, Step::Apply)]),
                Download::Revert,
                Download::Revert,
                Download::Blocks(vec![
                    planned(2, Step::Reapply),
                    planned(3, Step::Reapply),
                    planned(4, Step::Apply)
                ]),
                Download::Revert,
            ]
        );
        let corpus = corpus();
        let all = downloads(&l2::actions(&corpus));
        let blocks: usize = all
            .iter()
            .map(|d| match d {
                Download::Blocks(b) => b.len(),
                Download::Revert => 0,
            })
            .sum();
        let reverts = all.iter().filter(|d| **d == Download::Revert).count();
        let plan: &RevertPlan = &corpus.revert_plan;
        let reverted: u32 = plan.steps.iter().map(|s| s.blocks).sum();
        assert_eq!(reverts, reverted as usize);
        assert_eq!(blocks, corpus.blocks.len() + reverted as usize);
    }

    #[test]
    fn the_download_path_rebuilds_every_block_of_the_committed_chain() {
        let corpus = corpus();
        let blocks: Vec<JobBlock> = corpus
            .blocks
            .iter()
            .map(|b| JobBlock::of(b.bytes.clone()))
            .collect();
        for chunk in blocks.chunks(100) {
            let first = chunk[0].parts().unwrap().0.height;
            let params = corpus.milestones.at(first);
            let incoming = download(chunk, params).unwrap();
            assert_eq!(incoming.len(), chunk.len());
            for (listed, block) in incoming.iter().zip(chunk) {
                let rebuilt = to_block(&listed.block, &corpus.milestones).unwrap();
                let (header, _) = block.parts().unwrap();
                assert_eq!(rebuilt.id(), &serialiser::block_id(&header));
                assert_eq!(serialiser::serialise(&rebuilt), block.bytes);
                assert!(!listed.from_forger);
            }
        }
    }

    #[test]
    fn a_listing_that_misstates_its_count_downloads_and_does_not_rebuild() {
        let corpus = corpus();
        let with_transactions = corpus
            .blocks
            .iter()
            .find(|b| JobBlock::of(b.bytes.clone()).parts().unwrap().1.len() == 1)
            .unwrap();
        for count in [0, 2] {
            let block = JobBlock {
                bytes: with_transactions.bytes.clone(),
                count: Some(count),
            };
            let params = corpus.milestones.at(with_transactions.height);
            let incoming = download(&[block], params).unwrap();
            assert!(to_block(&incoming[0].block, &corpus.milestones).is_err());
        }
    }

    fn seen(block: &Block, result: BlockProcessorResult) -> Seen {
        Seen {
            height: block.height(),
            id: *block.id(),
            processed: Processed {
                result,
                applied: None,
                unchained: None,
            },
            line: None,
        }
    }

    #[test]
    fn a_stop_is_a_future_slot_or_a_failure() {
        let corpus = corpus();
        let milestones = &corpus.milestones;
        let decoded: Vec<Block> = corpus.blocks[..2]
            .iter()
            .map(|b| Block::receive(&b.bytes, milestones).unwrap())
            .collect();
        let blocks: Vec<JobBlock> = corpus.blocks[..2]
            .iter()
            .map(|b| JobBlock::of(b.bytes.clone()))
            .collect();
        let accepted = [seen(&decoded[0], BlockProcessorResult::Accepted)];
        let broadcast = JobEnd::Broadcast(Arc::new(decoded[0].clone()));
        let at = |block: &JobBlock| slot_time_ms(&block.parts().unwrap().0, milestones);
        assert_eq!(
            stop_of(&blocks, &accepted, &broadcast, at(&blocks[0]), milestones),
            Some(Stop::FutureSlot(decoded[1].height()))
        );
        assert_eq!(
            stop_of(&blocks, &accepted, &broadcast, at(&blocks[1]), milestones),
            Some(Stop::Failed)
        );
        assert_eq!(
            stop_of(&blocks, &[], &JobEnd::Clear, at(&blocks[1]), milestones),
            Some(Stop::Failed)
        );
        let rejected = [seen(&decoded[0], BlockProcessorResult::Rejected)];
        assert_eq!(
            stop_of(
                &blocks,
                &rejected,
                &JobEnd::Clear,
                at(&blocks[1]),
                milestones
            ),
            None
        );
        assert_eq!(
            stop_of(&blocks, &[], &JobEnd::Dropped, at(&blocks[1]), milestones),
            None
        );
        let both = [
            accepted[0].clone(),
            seen(&decoded[1], BlockProcessorResult::Accepted),
        ];
        assert_eq!(
            stop_of(&blocks, &both, &broadcast, at(&blocks[1]), milestones),
            None
        );
    }

    /// The oracle's answer to a job of one block with two votes of one sender (the synthetic
    /// case conflicting-and-forged on the committed chain), as the reference gave it.
    const TWO_VOTES: &str = r#"{"results": [{"height": 266, "id": "aa1843039747a117107c3fa4f691ff3afdfe866ffd3cdd4769340becec8d24e2", "result": "Rejected"}], "committed": 0, "end": "clear", "broadcast": false, "clearQueue": false, "failed": false, "lastDownloaded": {"height": 265, "id": "f41d004098f2f8d90363101e68d06bf088ea3d6c454c90840c2370c41cfe5284"}, "forked": null, "rollback": 0, "missed": [], "tip": {"height": 265, "id": "f41d004098f2f8d90363101e68d06bf088ea3d6c454c90840c2370c41cfe5284"}}"#;

    /// The oracle's answer to a job of one block refused in its apply after three missed slots
    /// (the synthetic case refused-after-gap on the committed chain).
    const REFUSED_AFTER_GAP: &str = r#"{"results": [{"height": 266, "id": "da4cd6da56c4e1c1c8c4a3cdd8ca5350703b392349d944f258f4439b420717a0", "result": "Rejected", "missed": [{"timestamp": 2184, "height": 266, "username": "genesis_19"}, {"timestamp": 2192, "height": 266, "username": "genesis_42"}, {"timestamp": 2200, "height": 266, "username": "genesis_8"}]}], "committed": 0, "end": "clear", "broadcast": false, "clearQueue": false, "failed": false, "lastDownloaded": {"height": 265, "id": "f41d004098f2f8d90363101e68d06bf088ea3d6c454c90840c2370c41cfe5284"}, "forked": null, "rollback": 0, "missed": [{"timestamp": 2184, "height": 266, "username": "genesis_19"}, {"timestamp": 2192, "height": 266, "username": "genesis_42"}, {"timestamp": 2200, "height": 266, "username": "genesis_8"}], "tip": {"height": 265, "id": "f41d004098f2f8d90363101e68d06bf088ea3d6c454c90840c2370c41cfe5284"}}"#;

    /// Heartwood's answer to a job of one block at 266 that the processor refused, with the tip
    /// at 265 and the last downloaded block reset there.
    fn refused_value(oracle: &Value) -> (Value, Seen) {
        let corpus = corpus();
        let id = |v: &Value| BlockId::from_hex(v.as_str().unwrap()).unwrap();
        let block_id = id(&oracle["results"][0]["id"]);
        let tip = id(&oracle["tip"]["id"]);
        let seen = Seen {
            height: 266,
            id: block_id,
            processed: Processed {
                result: BlockProcessorResult::Rejected,
                applied: None,
                unchained: None,
            },
            line: None,
        };
        let outcome = JobOutcome {
            results: vec![(block_id, BlockProcessorResult::Rejected)],
            committed: 0,
            clear_queue: false,
            end: JobEnd::Clear,
        };
        let chain = ChainState {
            last_downloaded: Some(ChainLink {
                id: tip,
                previous_block: tip,
                height: 265,
                timestamp: 0,
            }),
            ..ChainState::default()
        };
        let blocks = [JobBlock::of(corpus.blocks[0].bytes.clone())];
        let value = job_value(&JobParts {
            blocks: &blocks,
            inputs: &JobInputs::at(0),
            outcome: &outcome,
            seen: std::slice::from_ref(&seen),
            chain: &chain,
            tip: (265, tip),
            milestones: &corpus.milestones,
        });
        (value, seen)
    }

    #[test]
    fn a_job_answer_has_the_shape_of_the_oracles() {
        let oracle: Value = serde_json::from_str(TWO_VOTES).unwrap();
        let (value, _) = refused_value(&oracle);
        assert_eq!(compare::diff(&oracle, &value), Vec::new());
        let blocks = [JobBlock::of(corpus().blocks[0].bytes.clone())];
        let ran = |value: Value, seen: Seen| RigJob::Ran {
            outcome: JobOutcome {
                results: Vec::new(),
                committed: 0,
                clear_queue: false,
                end: JobEnd::Clear,
            },
            seen: vec![seen],
            value,
        };
        let (value, seen) = refused_value(&oracle);
        assert_eq!(
            judge_job(&blocks, &oracle, &ran(value.clone(), seen.clone())),
            Outcome::Pass
        );
        let mut other = value;
        other["end"] = json!("broadcast");
        assert!(judge_job(&blocks, &oracle, &ran(other, seen)).is_failure());
    }

    #[test]
    fn the_rows_of_a_block_refused_in_its_apply_are_the_documented_difference() {
        let oracle: Value = serde_json::from_str(REFUSED_AFTER_GAP).unwrap();
        let (value, seen) = refused_value(&oracle);
        let blocks = [JobBlock::of(corpus().blocks[0].bytes.clone())];
        let ran = |value: Value| RigJob::Ran {
            outcome: JobOutcome {
                results: Vec::new(),
                committed: 0,
                clear_queue: false,
                end: JobEnd::Clear,
            },
            seen: vec![seen.clone()],
            value,
        };
        assert!(refused_rows_only(
            &oracle,
            &value,
            std::slice::from_ref(&seen)
        ));
        assert_eq!(
            judge_job(&blocks, &oracle, &ran(value.clone())),
            Outcome::Documented(Documented::MissedRowsOfRefusedBlock.reason().to_owned())
        );
        // Another difference beside the rows is a mismatch.
        let mut other = value.clone();
        other["committed"] = json!(1);
        assert!(judge_job(&blocks, &oracle, &ran(other)).is_failure());
        // Rows Heartwood keeps for the refused block are a mismatch too.
        let mut kept = value;
        kept["missed"] = oracle["missed"].clone();
        assert!(judge_job(&blocks, &oracle, &ran(kept)).is_failure());
        // A block the processor accepted is not the documented form.
        let mut accepted = seen.clone();
        accepted.processed.result = BlockProcessorResult::Accepted;
        assert!(!refused_rows_only(
            &oracle,
            &refused_value(&oracle).0,
            &[accepted]
        ));
    }

    #[test]
    fn a_download_refused_for_a_removed_type_is_the_documented_difference() {
        let corpus = corpus();
        let oracle: Value = serde_json::from_str(TWO_VOTES).unwrap();
        let (header, transactions) = JobBlock::of(
            corpus
                .blocks
                .iter()
                .find(|b| JobBlock::of(b.bytes.clone()).parts().unwrap().1.len() == 1)
                .unwrap()
                .bytes
                .clone(),
        )
        .parts()
        .unwrap();
        let mut raw = transactions[0].clone();
        raw[7..9].copy_from_slice(&5u16.to_le_bytes());
        let mut bytes = serialiser::serialise_header(&header, true);
        bytes.extend_from_slice(&u32::try_from(raw.len()).unwrap().to_le_bytes());
        bytes.extend_from_slice(&raw);
        let removed = [JobBlock::of(bytes)];
        let refused = RigJob::Refused("the download: a removed type".into());
        assert_eq!(
            judge_job(&removed, &oracle, &refused),
            Outcome::Documented(Documented::RemovedType.reason().to_owned())
        );
        let plain = [JobBlock::of(corpus.blocks[0].bytes.clone())];
        assert!(judge_job(&plain, &oracle, &refused).is_failure());
    }

    #[test]
    fn job_blocks_and_inputs_read_the_oracles_form() {
        let blocks =
            job_blocks(&json!(["0a0b", {"hex": "0c", "set": {"numberOfTransactions": 3}}]))
                .unwrap();
        assert_eq!(
            blocks,
            vec![
                JobBlock {
                    bytes: vec![0x0a, 0x0b],
                    count: None
                },
                JobBlock {
                    bytes: vec![0x0c],
                    count: Some(3)
                }
            ]
        );
        assert!(job_blocks(&json!([{"hex": "0c", "set": {"version": 1}}])).is_err());
        assert!(job_blocks(&json!("0c")).is_err());
        let inputs = JobInputs::from_json(&json!({"now": 5})).unwrap();
        assert_eq!(inputs, JobInputs::at(5));
        let mut given = JobInputs::at(7);
        given.machine_state = "syncing".into();
        given.started = false;
        given.downloaded = false;
        given.last_downloaded = Some(vec![1, 2]);
        assert_eq!(JobInputs::from_json(&given.to_json()).unwrap(), given);
        assert!(JobInputs::from_json(&json!({})).is_err());
    }

    #[test]
    fn the_names_are_the_references() {
        assert_eq!(
            result_name(BlockProcessorResult::DiscardedButCanBeBroadcasted),
            "DiscardedButCanBeBroadcasted"
        );
        assert_eq!(
            unchained_name(UnchainedStatus::NotReadyToAcceptNewHeight),
            "NotReadyToAcceptNewHeight"
        );
        assert_eq!(Chunks::Reference.name(), "reference");
        assert_eq!(Chunks::One.name(), "1");
    }
}
