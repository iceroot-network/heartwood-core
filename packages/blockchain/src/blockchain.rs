//! The blockchain service: the reference's `Blockchain`, which drives the machine, the job queue
//! and the network around the node thread.
//!
//! - Incoming blocks ([`Blockchain::handle_incoming_block`]): a block in a future slot is
//!   discarded; the block is pinged; when started, `NEWBLOCK` and the block enqueued; before that,
//!   "Block disregarded because blockchain is not ready".
//! - Enqueueing ([`Blockchain::enqueue_blocks`], [`cut_jobs`]): jobs cut after 150 transactions,
//!   100 blocks, or at the first remaining milestone height at or above the last downloaded height
//!   (a list that shifts only when such a cut happens, reproduced); the remaining blocks form a
//!   last job, pushed even when empty; each push resumes the queue unless a fork recovery runs.
//! - Removal ([`Blockchain::remove_blocks`]), the fork block and the job's fork check, `isSynced`,
//!   the wake-up timer (60 s without an accepted block: back to syncing), and the job outcomes and
//!   the queue's drain ([`crate::queue::QueueEvents`]): after the queue had the node re-add the
//!   pool and save the online record, `PROCESSFINISHED`.
//! - The machine ([`Blockchain::dispatch`]) and its actions ([`crate::state_machine`]).
//!
//! The service runs its actions and loops on the runtime of [`BlockchainParts::runtime`]. It keeps
//! the job queue's rules: the queue calls [`crate::queue::QueueEvents::finished`] before it starts
//! the next job, and without holding a lock of its own, since the service may clear or pause it
//! from there, as the reference's handlers do during a job. The events the reference also puts on
//! its bus (`block.received`, `block.disregarded`, `state.started`) have no listener in a relay;
//! `blockchain.synced` is [`Network::synced`].

use std::future::Future;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::Duration;

use heartwood_crypto::crypto::slots;
use heartwood_crypto::{Block, BlockId, Milestones};
use heartwood_kernel::errors::BootError;
use heartwood_kernel::utils::pluralise::pluralise;
use heartwood_kernel::{ChainLink, Clock, debug, info, is_block_chained, warning};
use heartwood_p2p::DownloadedBlock;
use heartwood_p2p::monitor::CleanseOptions;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::chain_state::{self, ChainState, MAX_LAST_BLOCKS, SharedChainState};
use crate::errors::NodeError;
use crate::fork_recovery::RollbackDepth;
use crate::network::Network;
use crate::options::BlockchainOptions;
use crate::queue::{JobQueue, QueueEvents};
use crate::state_machine::{MachineEvent, MachineState, transition};
use crate::work::{IncomingBlock, Job, JobEnd, JobOutcome, NodeHandle};

/// How long the machine waits in idle without an accepted block before it syncs again.
const WAKE_UP_MS: i64 = 60_000;
/// How often the wake-up timer looks at the chain while it is not set.
const WAKE_UP_POLL: Duration = Duration::from_secs(1);
/// The sync's download timeout: the reference's default `getBlocksTimeout`.
pub(crate) const GET_BLOCKS_TIMEOUT: Duration = Duration::from_secs(30);

/// The job queue as the service drives it: [`JobQueue`] in a node; a test may give its own.
pub trait Queue: Send + Sync + 'static {
    /// Push `job`; it runs after the jobs before it, once the queue runs.
    fn push(&self, job: Job);
    /// Hold the jobs not started; the running job finishes.
    fn pause(&self);
    /// Run the held jobs again.
    fn resume(&self);
    /// Drop the jobs not started.
    fn clear(&self);
    /// Hold every job, drop the jobs not started, and wait for the running one to finish.
    fn stop(&self) -> impl Future<Output = ()> + Send;
    /// How many jobs wait.
    fn size(&self) -> usize;
    /// Whether a job runs now.
    fn is_running(&self) -> bool;
}

impl Queue for JobQueue {
    fn push(&self, job: Job) {
        JobQueue::push(self, job);
    }
    fn pause(&self) {
        JobQueue::pause(self);
    }
    fn resume(&self) {
        JobQueue::resume(self);
    }
    fn clear(&self) {
        JobQueue::clear(self);
    }
    fn stop(&self) -> impl Future<Output = ()> + Send {
        JobQueue::stop(self)
    }
    fn size(&self) -> usize {
        JobQueue::size(self)
    }
    fn is_running(&self) -> bool {
        JobQueue::is_running(self)
    }
}

/// What the service publishes of the machine.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MachineStatus {
    /// The machine's state.
    pub state: MachineState,
    /// Whether the blockchain has started.
    pub started: bool,
}

/// What the blockchain service is made of.
pub struct BlockchainParts<N> {
    /// The node thread.
    pub node: NodeHandle,
    /// The chain state, shared with the node thread.
    pub chain: SharedChainState,
    /// The network.
    pub network: N,
    /// The blockchain options.
    pub options: Arc<BlockchainOptions>,
    /// The milestones.
    pub milestones: Arc<Milestones>,
    /// The clock.
    pub clock: Arc<dyn Clock>,
    /// How many blocks a recovery without a set count removes.
    pub rollback: RollbackDepth,
    /// The runtime of the service's tasks: the idle loops, the wake-up timer, the queue.
    pub runtime: tokio::runtime::Handle,
}

/// The service's own flags, besides the chain state.
#[derive(Debug, Default)]
pub(crate) struct Flags {
    /// `dispose` ran.
    pub(crate) stopped: bool,
    /// The machine reached `exit`.
    pub(crate) failed: bool,
    /// A fork recovery or a job's fork check removes blocks.
    pub(crate) forking: bool,
    /// A job's fork check runs.
    pub(crate) fork_check: bool,
    /// When the last fork check started, in milliseconds.
    pub(crate) last_fork_check_ms: i64,
    /// The idle loops run.
    pub(crate) polled: bool,
    /// When the wake-up timer was last started, while it is set.
    pub(crate) wake_up: Option<i64>,
    /// The last accepted block's time that the timer has taken.
    pub(crate) seen_accept: Option<i64>,
    /// The single block the idle poll last handled, and the tip it was on.
    pub(crate) last_polled: Option<(BlockId, BlockId)>,
}

/// The blockchain service.
pub struct Blockchain<N, Q = JobQueue> {
    pub(crate) parts: BlockchainParts<N>,
    status: watch::Sender<MachineStatus>,
    pub(crate) queue: Q,
    flags: Mutex<Flags>,
    depth: Mutex<RollbackDepth>,
    tasks: Mutex<Vec<JoinHandle<()>>>,
}

/// The service as the queue's listener.
struct Hooks<N, Q>(Weak<Blockchain<N, Q>>);

impl<N: Network, Q: Queue> QueueEvents for Hooks<N, Q> {
    fn finished(&self, outcome: JobOutcome) {
        if let Some(service) = self.0.upgrade() {
            service.job_finished(outcome);
        }
    }

    fn drained(&self) {
        if let Some(service) = self.0.upgrade() {
            service.dispatch(MachineEvent::ProcessFinished);
        }
    }
}

impl<N: Network> Blockchain<N> {
    /// A service of `parts` with its job queue, its machine uninitialised.
    pub fn new(parts: BlockchainParts<N>) -> Arc<Blockchain<N>> {
        Blockchain::with_queue(parts, JobQueue::new)
    }
}

impl<N: Network, Q: Queue> Blockchain<N, Q> {
    /// A service of `parts` whose job queue `queue` makes from the node, the service's listener
    /// and the runtime.
    pub fn with_queue<F>(parts: BlockchainParts<N>, queue: F) -> Arc<Blockchain<N, Q>>
    where
        F: FnOnce(NodeHandle, Arc<dyn QueueEvents>, tokio::runtime::Handle) -> Q,
    {
        if parts.options.network_start {
            warning!(
                "Heartwood Core is launched in Genesis Start mode. This is usually for starting the first node on the blockchain. Unless you know what you are doing, this is likely wrong :warning:"
            );
        }
        chain_state::lock(&parts.chain).network_start = parts.options.network_start;
        Arc::new_cyclic(|service| {
            let hooks: Arc<dyn QueueEvents> = Arc::new(Hooks(service.clone()));
            Blockchain {
                queue: queue(parts.node.clone(), hooks, parts.runtime.clone()),
                depth: Mutex::new(parts.rollback.clone()),
                parts,
                status: watch::channel(MachineStatus::default()).0,
                flags: Mutex::default(),
                tasks: Mutex::default(),
            }
        })
    }

    /// Start the machine (`START`) and wait until it is synced or idle, then ping 10 random peers;
    /// a failed start is the boot's error.
    pub async fn boot(self: &Arc<Self>) -> Result<(), BootError> {
        info!("Starting Blockchain Manager :chains:");
        self.spawn_wake_up_timer();
        self.dispatch(MachineEvent::Start);
        while !self.flags().stopped && !self.chain().started {
            if self.flags().failed {
                let reason = "the blockchain could not start".into();
                return Err(BootError {
                    package: BlockchainOptions::PACKAGE,
                    reason,
                });
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        let ten = CleanseOptions {
            force_ping: true,
            peer_count: Some(10),
            ..CleanseOptions::default()
        };
        self.parts.network.cleanse_peers(ten).await;
        Ok(())
    }

    /// A block from a peer or the node's own forger (see the module documentation);
    /// `fire_event` is false for the idle poll's blocks.
    pub async fn handle_incoming_block(self: &Arc<Self>, block: IncomingBlock, fire_event: bool) {
        let _ = fire_event;
        let block_time = self.parts.milestones.at(1).block_time();
        let timestamp = i64::from(block.block.header.header.timestamp.unwrap_or(0));
        if slots::slot_number(timestamp, block_time) > slots::slot_number(self.time(), block_time) {
            let height = locale(block.height());
            info!("Discarded block {height} because it takes a future slot :exclamation:");
            return;
        }
        if let Some(link) = link(&block.block) {
            let (now, forger) = (self.parts.clock.now_ms(), block.from_forger);
            self.chain()
                .push_ping_block(link.id, link.height, forger, now);
        }
        if self.chain().started {
            self.dispatch(MachineEvent::NewBlock);
            self.enqueue_blocks(vec![block]);
        } else {
            info!("Block disregarded because blockchain is not ready :exclamation:");
        }
    }

    /// Cut `blocks` into jobs ([`cut_jobs`]) from the last downloaded height, and push them.
    pub fn enqueue_blocks(self: &Arc<Self>, blocks: Vec<IncomingBlock>) {
        if blocks.is_empty() {
            return;
        }
        let chain = self.chain();
        let from = chain
            .last_downloaded
            .map_or(chain.last_height(), |link| link.height);
        drop(chain);
        for job in cut_jobs(blocks, from, &self.parts.milestones) {
            self.queue.push(job);
            if !self.flags().forking {
                self.queue.resume();
            }
        }
    }

    /// Remove the last `count` blocks (at most the tip minus one): clear and stop the queue, then
    /// the node's removal (which moves the chain state's tip), then the last downloaded block at the
    /// new tip. A failed revert stops the node, and is the error.
    pub async fn remove_blocks(self: &Arc<Self>, count: u32) -> Result<(), NodeError> {
        let tip = self.chain().last_height();
        let count = count.min(tip.saturating_sub(1));
        if count < 1 {
            return Ok(());
        }
        self.clear_and_stop_queue();
        // The node logs "Removing <n> blocks. Reset to height <h>" and each "Undoing block <h>".
        match self.parts.node.remove_blocks(count).await {
            Ok(_) => {
                self.chain().reset_last_downloaded();
                Ok(())
            }
            Err(error) => {
                heartwood_kernel::error!("{error}");
                warning!("Shutting down app, because state might be corrupted :boom:");
                Err(error)
            }
        }
    }

    /// Mark `block` as forked, clear and stop the queue, set the rollback count when given, and
    /// dispatch `FORK`.
    pub fn fork_block(self: &Arc<Self>, block: Arc<Block>, rollback: Option<u32>) {
        self.chain().forked_block = Some(block);
        self.clear_and_stop_queue();
        if let Some(count) = rollback.filter(|count| *count > 0) {
            self.chain().blocks_to_rollback = count;
        }
        self.dispatch(MachineEvent::Fork);
    }

    /// The fork check of blocks that did not chain: only when no recovery runs and more than one
    /// block time has passed since the last check; whether it rolled back. After a rollback, the
    /// block below the first one is downloaded from its peer and enqueued with them.
    pub async fn check_for_fork(self: &Arc<Self>, blocks: &[IncomingBlock]) -> bool {
        let now = self.parts.clock.now_ms();
        if !self.flags_with(|flags| {
            let due =
                !flags.forking && now.saturating_sub(flags.last_fork_check_ms) > self.block_ms();
            if due {
                (flags.last_fork_check_ms, flags.fork_check) = (now, true);
            }
            due
        }) {
            return false;
        }
        let rollback = self.parts.network.check_for_fork().await;
        if rollback > 0 {
            self.flags().forking = true;
            self.queue.stop().await;
            let removed = self.remove_blocks(rollback).await;
            self.queue.clear();
            self.rolled_back(rollback);
            self.queue.resume();
            if let (Ok(()), Some(first)) = (removed, blocks.first()) {
                let (ip, below) = (first.block.ip, first.height().saturating_sub(1));
                let download = self.parts.network.download_block_at_height(ip, below);
                if let Ok(Some(forked)) = download.await {
                    let mut blocks = blocks.to_vec();
                    blocks.insert(0, IncomingBlock::downloaded(forked));
                    self.chain().last_downloaded = blocks.last().and_then(|b| link(&b.block));
                    self.dispatch(MachineEvent::NewBlock);
                    self.enqueue_blocks(blocks);
                }
            }
        }
        self.flags().fork_check = false;
        rollback > 0
    }

    /// Whether `block` (the tip when `None`) is less than three block times old; true without
    /// peers.
    pub fn is_synced(&self, block: Option<&ChainLink>) -> bool {
        if self.parts.network.peer_count() == 0 {
            return true;
        }
        let Some(block) = block.copied().or_else(|| self.tip_link()) else {
            return true;
        };
        let block_time = self.parts.milestones.at(block.height).block_time();
        let age = self.time().saturating_sub(block.timestamp.into());
        age < i64::try_from(block_time.saturating_mul(3)).unwrap_or(i64::MAX)
    }

    /// Dispatch `event` to the machine; its transition's actions run in the background, in order.
    pub fn dispatch(self: &Arc<Self>, event: MachineEvent) {
        let mut actions = Vec::new();
        self.status.send_if_modified(|status| {
            let Some(next) = transition(status.state, event) else {
                return false;
            };
            let (from, to) = (status.state.name(), next.next.name());
            debug!("event '{}': {from} -> {to}", event.name());
            status.state = next.next;
            actions = next.actions;
            true
        });
        for action in actions {
            let service = Arc::clone(self);
            self.spawn(service.run(action));
        }
    }

    /// The machine's published status.
    pub fn status(&self) -> watch::Receiver<MachineStatus> {
        self.status.subscribe()
    }

    /// Stop: `STOP`, the idle loops and the wake-up timer ended, the queue stopped after its
    /// running job.
    pub async fn dispose(self: &Arc<Self>) {
        let stopped = self.flags_with(|flags| {
            flags.wake_up = None;
            std::mem::replace(&mut flags.stopped, true)
        });
        if !stopped {
            info!("Stopping Blockchain Manager :chains:");
            self.dispatch(MachineEvent::Stop);
            self.queue.stop().await;
            std::mem::take(&mut *self.tasks())
                .iter()
                .for_each(JoinHandle::abort);
        }
    }

    /// A job's end (see [`JobEnd`]), and the queue cleared when a handler asked for it.
    fn job_finished(self: &Arc<Self>, outcome: JobOutcome) {
        if outcome.clear_queue {
            self.queue.clear();
        }
        match outcome.end {
            JobEnd::Broadcast(block) => {
                if self.chain().started && self.state() == MachineState::NewBlock {
                    let service = Arc::clone(self);
                    self.spawn(async move { service.parts.network.broadcast_block(&block).await });
                }
            }
            JobEnd::Fork(block) => self.fork_block(block, None),
            JobEnd::Clear => {
                self.queue.clear();
                self.chain().reset_last_downloaded();
            }
            JobEnd::ForkCheck(blocks) => {
                self.queue.clear();
                self.chain().reset_last_downloaded();
                let (tip, service) = (self.tip_link(), Arc::clone(self));
                self.spawn(async move {
                    if !service.flags().fork_check && !service.check_for_fork(&blocks).await {
                        let first = blocks.first().and_then(|block| link(&block.block));
                        if let (Some(tip), Some(first)) = (tip, first)
                            && (tip.id, tip.height) != (first.id, first.height)
                        {
                            warning!("{}", not_chained(&tip, &first, &service.parts.milestones));
                        }
                    }
                });
            }
            JobEnd::Dropped => {}
        }
    }

    /// The reference's `clearAndStopQueue`: the last downloaded block back at the tip, the queue
    /// paused and cleared.
    pub(crate) fn clear_and_stop_queue(&self) {
        self.chain().reset_last_downloaded();
        self.queue.pause();
        self.queue.clear();
    }

    /// After a removal of `count` blocks: the last downloaded block at the stored tip, no set
    /// count, and the recovery over.
    pub(crate) fn rolled_back(&self, count: u32) {
        let mut chain = self.chain();
        chain.reset_last_downloaded();
        chain.blocks_to_rollback = 0;
        drop(chain);
        info!(
            "Removed {} :wastebasket:",
            pluralise("block", count.into(), true)
        );
        self.flags().forking = false;
    }

    /// The wake-up timer: `WAKEUP` 60 s after it was set or after the last block accepted while
    /// started (the reference restarts it at each), unless cleared first.
    fn spawn_wake_up_timer(self: &Arc<Self>) {
        let service = Arc::downgrade(self);
        let task = self.parts.runtime.spawn(async move {
            while let Some(pause) = service.upgrade().and_then(|service| service.wake_up_step()) {
                tokio::time::sleep(pause).await;
            }
        });
        self.tasks().push(task);
    }

    /// One look of the wake-up timer: `WAKEUP` when it is due; how long to wait for the next
    /// look, or `None` once stopped.
    fn wake_up_step(self: &Arc<Self>) -> Option<Duration> {
        let accepted = {
            let chain = self.chain();
            chain.last_accepted_ms.filter(|_| chain.started)
        };
        let now = self.parts.clock.now_ms();
        let mut flags = self.flags();
        if flags.stopped {
            return None;
        }
        if accepted.is_some() && accepted != flags.seen_accept {
            flags.seen_accept = accepted;
            flags.wake_up = accepted;
        }
        let Some(due) = flags.wake_up.map(|since| since.saturating_add(WAKE_UP_MS)) else {
            return Some(WAKE_UP_POLL);
        };
        if now < due {
            let wait = u64::try_from(due.saturating_sub(now)).unwrap_or(1);
            return Some(Duration::from_millis(wait));
        }
        flags.wake_up = None;
        drop(flags);
        self.dispatch(MachineEvent::WakeUp);
        Some(WAKE_UP_POLL)
    }

    /// The machine's state.
    pub(crate) fn state(&self) -> MachineState {
        self.status.borrow().state
    }

    /// Mark the blockchain as started.
    pub(crate) fn set_started(&self) {
        self.status.send_modify(|status| status.started = true);
    }

    /// The block time, in milliseconds.
    pub(crate) fn block_ms(&self) -> i64 {
        let seconds = self.parts.milestones.at(1).block_time();
        i64::try_from(seconds)
            .unwrap_or(i64::MAX)
            .saturating_mul(1000)
    }

    /// Seconds since the network epoch, as the reference's `Slots.getTime` gives them.
    pub(crate) fn time(&self) -> i64 {
        slots::epoch_time(self.parts.clock.now_ms(), self.epoch_ms())
    }

    /// The network's epoch, in milliseconds since the Unix epoch (the milestones' loader checked
    /// that it reads).
    pub(crate) fn epoch_ms(&self) -> i64 {
        slots::epoch_unix_ms(self.parts.milestones.at(1).epoch()).unwrap_or(0)
    }

    /// The tip's link, once a block is in memory.
    pub(crate) fn tip_link(&self) -> Option<ChainLink> {
        self.chain().last_block().map(|block| ChainLink::of(block))
    }

    /// Draw the depth of a recovery without a set count.
    pub(crate) fn draw_depth(&self) -> u32 {
        self.depth
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .draw()
    }

    pub(crate) fn chain(&self) -> MutexGuard<'_, ChainState> {
        chain_state::lock(&self.parts.chain)
    }

    pub(crate) fn flags(&self) -> MutexGuard<'_, Flags> {
        self.flags.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// `change` on the flags, under their lock.
    pub(crate) fn flags_with<T>(&self, change: impl FnOnce(&mut Flags) -> T) -> T {
        change(&mut self.flags())
    }

    pub(crate) fn tasks(&self) -> MutexGuard<'_, Vec<JoinHandle<()>>> {
        self.tasks.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn spawn<F: Future<Output = ()> + Send + 'static>(&self, future: F) {
        self.parts.runtime.spawn(future);
    }
}

/// How the reference's `enqueueBlocks` cuts `blocks` into jobs when the last downloaded block is
/// at `last_downloaded`: a job ends after the block that brings it to 150 transactions or to 100
/// blocks, or whose height is the first of the sorted milestone heights at or above
/// `last_downloaded`; that list shifts only at such a cut, so a milestone at `last_downloaded`
/// itself stays first and no later milestone cuts. A block without a stated number of
/// transactions makes the count unknown until the next cut, which then comes only from the block
/// count or a milestone, as the reference's `NaN` does. The remaining blocks are the last job,
/// even when there are none.
pub fn cut_jobs(
    blocks: Vec<IncomingBlock>,
    last_downloaded: u32,
    milestones: &Milestones,
) -> Vec<Job> {
    let mut heights: Vec<u32> = milestones.all().iter().map(|m| m.height()).collect();
    heights.retain(|height| *height >= last_downloaded);
    heights.sort_unstable();
    let mut heights = heights.into_iter().peekable();
    let (mut jobs, mut job, mut transactions) = (Vec::new(), Job::default(), Some(0_u64));
    for block in blocks {
        let stated = block.block.header.header.number_of_transactions;
        transactions = transactions
            .zip(stated)
            .map(|(sum, n)| sum.saturating_add(n.into()));
        let milestone = heights.next_if_eq(&block.height()).is_some();
        job.blocks.push(block);
        let full =
            transactions.is_some_and(|sum| sum >= 150) || job.blocks.len() >= MAX_LAST_BLOCKS;
        if full || milestone {
            jobs.push(std::mem::take(&mut job));
            transactions = Some(0);
        }
    }
    jobs.push(job);
    jobs
}

/// The chain link of a downloaded block, from its header's stated ids.
pub(crate) fn link(block: &DownloadedBlock) -> Option<ChainLink> {
    let header = &block.header.header;
    Some(ChainLink {
        id: BlockId::from_hex(header.id.as_deref()?).ok()?,
        previous_block: BlockId::from_hex(header.previous_block.as_deref()?).ok()?,
        height: header.height?,
        timestamp: header.timestamp?,
    })
}

/// A count as JavaScript's `toLocaleString()` writes it.
pub(crate) fn locale(count: u32) -> String {
    heartwood_crypto::utils::js::to_locale_integer(count.into())
}

/// The reference's message for a `next` block that does not chain to `previous`.
fn not_chained(previous: &ChainLink, next: &ChainLink, milestones: &Milestones) -> String {
    let detail = is_block_chained(previous, next, milestones).err();
    let detail = detail.map_or("Block had no chain error".into(), |why| why.to_string());
    format!(
        "Block {{ height: {}, id: {}, previousBlock: {} }} is not chained to the previous block {{ height: {}, id: {} }}: {detail}",
        next.height, next.id, next.previous_block, previous.height, previous.id
    )
}
