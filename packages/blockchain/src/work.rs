//! The node thread's work and its handle, and the jobs' blocks and outcomes: the types between the
//! node thread, the job queue and the blockchain service.
//!
//! The service and the queue send the node thread [`Work`] through a [`NodeHandle`], in order, and
//! read what it publishes after each commit through the handle's `watch` receivers. A test of the
//! service fakes the node thread with a loop that answers the work from a script.

use std::sync::Arc;

use heartwood_crypto::identities::PublicKeyBytes;
use heartwood_crypto::{Block, BlockId, TransactionId};
use heartwood_p2p::DownloadedBlock;
use heartwood_p2p::node_services::Collation;
use heartwood_pool::{Entry, PoolView, ProcessorResult};
use heartwood_state::{StateView, WalletRepository};
use tokio::sync::{mpsc, oneshot, watch};

use crate::errors::{NodeError, NodeStopped};
use crate::processor::BlockProcessorResult;

/// The delegates on this node: the keys the forger's attestation session verified.
pub use heartwood_p2p::node_services::AttestedKeys as OwnKeys;

/// A block for a job: downloaded from a peer (whose address it keeps), or from the node's own
/// forger.
#[derive(Debug, Clone, PartialEq)]
pub struct IncomingBlock {
    /// The block as the download read it: its header's fields and its transactions.
    pub block: DownloadedBlock,
    /// Whether the block came from the node's own forger.
    pub from_forger: bool,
    /// The delegate name the block got when it was enqueued ([`IncomingBlock::set_username`]),
    /// which the processor reads; `None` before, and when its key named no delegate then.
    pub name: Option<String>,
}

impl IncomingBlock {
    /// A downloaded block, not named yet.
    pub fn downloaded(block: DownloadedBlock) -> IncomingBlock {
        IncomingBlock {
            block,
            from_forger: false,
            name: None,
        }
    }

    /// Name the block as the reference's `setBlockUsername` does: a block of version 0 without a
    /// name gets the name of the delegate whose key is its generator key in `wallets`, if any.
    pub fn set_username(&mut self, wallets: &WalletRepository) {
        let header = &self.block.header.header;
        if self.name.is_some() || header.version != Some(0) {
            return;
        }
        let key = header.generator_public_key.as_deref();
        let key = key.and_then(|key| PublicKeyBytes::from_hex(key).ok());
        let delegate = key.and_then(|key| wallets.find_by_public_key(&key)?.delegate());
        self.name = delegate.map(|delegate| delegate.username().to_owned());
    }

    /// The height its header states.
    pub fn height(&self) -> u32 {
        self.block.height()
    }
}

/// The blocks of one job, in height order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Job {
    /// The blocks.
    pub blocks: Vec<IncomingBlock>,
}

/// How a job ended, for the blockchain service.
#[derive(Debug, Clone, PartialEq)]
pub enum JobEnd {
    /// The last result allows a broadcast of this block, the last processed one, which came from
    /// the node's own forger when the flag is set: the service broadcasts it when the machine is
    /// in `newBlock`, after the processing lateness check of a forger's block.
    Broadcast(Arc<Block>, bool),
    /// A block was marked as forked: the service calls `forkBlock` with it.
    Fork(Arc<Block>),
    /// Neither: the service clears the queue and resets the last downloaded block.
    Clear,
    /// The first block did not chain to the tip: the service clears the queue, resets the last
    /// downloaded block and checks for a fork in the background with the job's blocks.
    ForkCheck(Vec<IncomingBlock>),
    /// The first block was at or below the tip: nothing happened.
    Dropped,
}

/// What a job did.
#[derive(Debug, Clone, PartialEq)]
pub struct JobOutcome {
    /// Each processed block's id and result, in order.
    pub results: Vec<(BlockId, BlockProcessorResult)>,
    /// How many blocks were committed.
    pub committed: u32,
    /// Whether a handler asked for the queue to be cleared (an unchained block).
    pub clear_queue: bool,
    /// How the job ended.
    pub end: JobEnd,
    /// The peer that served a block no honest peer serves: one the rebuild refused, one the
    /// processor refused for a fault ([`crate::processor::Processed::fault`]), or one discarded
    /// for its future slot that no delegate forged ([`crate::processor::no_delegate_forged`]).
    pub faulty: Option<std::net::IpAddr>,
}

/// Where pool entries come from; each source has its own processor, and so its own cooldown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    /// `POST /transactions`.
    Api,
    /// The idle loop's download of unconfirmed transactions.
    Download,
    /// The peer-to-peer server's postTransactions.
    P2p,
}

/// A change of the online record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OnlineWork {
    /// A status attestation proved that the delegate with `key` runs `version` in `round`.
    Set {
        /// The delegate's public key.
        key: PublicKeyBytes,
        /// The round.
        round: u64,
        /// The version.
        version: String,
    },
    /// Expire the entries older than `round` minus 5 (the sync's end).
    Expire {
        /// The round.
        round: u64,
    },
}

/// The node thread's work, in arrival order.
#[derive(Debug)]
pub enum Work {
    /// Run a job and answer its outcome. A fatal failure stops the node without an answer.
    Job(Job, oneshot::Sender<JobOutcome>),
    /// Process pool entries with the channel's processor, broadcast what it admits for
    /// broadcast, and answer the processor's result.
    PoolEntries(Channel, Vec<Entry>, oneshot::Sender<ProcessorResult>),
    /// A drain: save the online record, then re-add every pool transaction with its checks;
    /// answered after the re-add when the flag is set (the start's re-add, which the reference
    /// waits for), else right after the save (the queue's drain, whose re-add the reference does
    /// not wait for before `PROCESSFINISHED`; the node's next work waits for it).
    Drain(oneshot::Sender<Result<(), NodeError>>, bool),
    /// Remove the last blocks (at most the tip minus one), re-add their transactions to the pool
    /// in chain order, and answer the new tip's height.
    RemoveBlocks(u32, oneshot::Sender<Result<u32, NodeError>>),
    /// Change the online record.
    Online(OnlineWork),
    /// Collate the pool's candidates at the tip plus one, validated when `validate`, without the
    /// transactions of `exclude`, and answer them with the pool's size.
    Collate {
        /// Validate the candidates (a forger's collation).
        validate: bool,
        /// The ids to leave out.
        exclude: Vec<TransactionId>,
        /// The answer.
        reply: oneshot::Sender<Collation>,
    },
    /// Stop after the work before this one: flush the pool storage, save the online record,
    /// checkpoint and close the store, then answer.
    Stop(oneshot::Sender<()>),
}

/// What the node publishes of its chain besides the state.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChainStatus {
    /// The last blocks in memory, at most 100, in height order; the last is the tip.
    pub last_blocks: Vec<Arc<Block>>,
    /// The height of the last stored block.
    pub stored_height: u32,
}

impl ChainStatus {
    /// The tip, once a block is in memory.
    pub fn tip(&self) -> Option<&Arc<Block>> {
        self.last_blocks.last()
    }
}

/// A handle on the node thread: its work queue and its snapshots. Cheap to clone.
#[derive(Debug, Clone)]
pub struct NodeHandle {
    work: mpsc::UnboundedSender<Work>,
    state: watch::Receiver<Arc<StateView>>,
    pool: watch::Receiver<Arc<PoolView>>,
    chain: watch::Receiver<Arc<ChainStatus>>,
}

impl NodeHandle {
    /// A handle of these channels: the node thread's, or a test's fake node loop.
    pub fn from_channels(
        work: mpsc::UnboundedSender<Work>,
        state: watch::Receiver<Arc<StateView>>,
        pool: watch::Receiver<Arc<PoolView>>,
        chain: watch::Receiver<Arc<ChainStatus>>,
    ) -> NodeHandle {
        NodeHandle {
            work,
            state,
            pool,
            chain,
        }
    }

    /// Queue `work`.
    pub fn submit(&self, work: Work) -> Result<(), NodeStopped> {
        self.work.send(work).map_err(|_| NodeStopped)
    }

    /// The published state.
    pub fn state(&self) -> watch::Receiver<Arc<StateView>> {
        self.state.clone()
    }

    /// The published pool.
    pub fn pool(&self) -> watch::Receiver<Arc<PoolView>> {
        self.pool.clone()
    }

    /// The published chain status.
    pub fn chain(&self) -> watch::Receiver<Arc<ChainStatus>> {
        self.chain.clone()
    }

    /// Run `job` and wait for its outcome.
    pub async fn job(&self, job: Job) -> Result<JobOutcome, NodeStopped> {
        let (answer, outcome) = oneshot::channel();
        self.submit(Work::Job(job, answer))?;
        outcome.await.map_err(|_| NodeStopped)
    }

    /// Process `entries` on `channel`'s processor and wait for the result.
    pub async fn pool_entries(
        &self,
        channel: Channel,
        entries: Vec<Entry>,
    ) -> Result<ProcessorResult, NodeStopped> {
        let (answer, result) = oneshot::channel();
        self.submit(Work::PoolEntries(channel, entries, answer))?;
        result.await.map_err(|_| NodeStopped)
    }

    /// Run a drain and wait for it, the pool's re-add included.
    pub async fn drain(&self) -> Result<(), NodeError> {
        let (answer, result) = oneshot::channel();
        self.submit(Work::Drain(answer, true))?;
        result.await.map_err(|_| NodeError::Stopped)?
    }

    /// Remove the last `count` blocks and wait for the new tip's height.
    pub async fn remove_blocks(&self, count: u32) -> Result<u32, NodeError> {
        let (answer, result) = oneshot::channel();
        self.submit(Work::RemoveBlocks(count, answer))?;
        result.await.map_err(|_| NodeError::Stopped)?
    }

    /// Collate the pool's candidates ([`Work::Collate`]) and wait for them.
    pub async fn collate(
        &self,
        validate: bool,
        exclude: Vec<TransactionId>,
    ) -> Result<Collation, NodeStopped> {
        let (reply, collation) = oneshot::channel();
        self.submit(Work::Collate {
            validate,
            exclude,
            reply,
        })?;
        collation.await.map_err(|_| NodeStopped)
    }

    /// Change the online record.
    pub fn online(&self, work: OnlineWork) -> Result<(), NodeStopped> {
        self.submit(Work::Online(work))
    }

    /// Stop the node thread after its queued work, and wait for it.
    pub async fn stop(&self) {
        let (answer, done) = oneshot::channel();
        if self.submit(Work::Stop(answer)).is_ok() {
            let _ = done.await;
        }
    }
}
