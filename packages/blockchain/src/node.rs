//! The node thread: it alone holds the state, the store, the pool and the online record.
//!
//! Every change runs here, in the order the work arrives ([`crate::work::Work`]): jobs, pool
//! entries, the queue's drain, block removals, the online record's changes, and the stop. After
//! each commit the thread runs the pool's steps (the expiry of the online record at each round
//! start, a re-add when a milestone starts, and the clean-up), publishes the new snapshots (the
//! state, the pool, the chain status) through `watch` channels, and tells its listener, never
//! between two blocks of a job. Network and API tasks send it work through a [`NodeHandle`] and
//! read the snapshots; nothing else reads or writes the state or the store.
//!
//! A job locks the shared chain state for one block at a time, so the service's actions and the
//! network's reads come between two blocks, and see the tip as it moves. A fatal error ([`NodeError`]) ends the thread at once: the work it was
//! doing gets no answer and the store keeps its committed tip. A stop ends it after the work
//! before it: the pool's storage keeps its transactions, the online record is saved, and the
//! store is closed (SQLite checkpoints its log when its last connection closes); the running job
//! was committed or rolled back before, since the thread runs one work at a time.

use std::sync::Arc;
use std::thread::JoinHandle;

use heartwood_crypto::utils::js::to_locale_integer;
use heartwood_crypto::{Milestones, Network};
use heartwood_kernel::utils::pluralise::pluralise;
use heartwood_kernel::{Clock, calculate_round, error, info};
use heartwood_pool::PoolView;
use heartwood_state::{State, StateView};
use heartwood_transactions::Handlers;
use tokio::sync::{mpsc, watch};

use crate::chain_state::{SharedChainState, lock};
use crate::commit::ChainStore;
use crate::errors::{NodeError, StoreError};
use crate::events::{CommitObserver, NoObserver, NodeEvent, NodeListener};
use crate::options::BlockchainOptions;
use crate::pool_task::{PoolEnv, PoolTask, TransactionBroadcast};
use crate::process_blocks_job::{JobContext, run_job};
use crate::processor::BlockProcess;
use crate::work::{ChainStatus, Job, JobOutcome, NodeHandle, Work};

/// What the node thread is made of.
pub struct NodeParts {
    /// The restored state.
    pub state: State,
    /// The started store.
    pub store: ChainStore,
    /// The pool with the restored online record.
    pub pool: PoolTask,
    /// The chain state, shared with the blockchain service.
    pub chain: SharedChainState,
    /// The block processor.
    pub processor: Arc<dyn BlockProcess>,
    /// The transaction handlers.
    pub handlers: Handlers,
    /// The milestones.
    pub milestones: Arc<Milestones>,
    /// The network.
    pub network: Arc<Network>,
    /// The blockchain options.
    pub options: Arc<BlockchainOptions>,
    /// The clock.
    pub clock: Arc<dyn Clock>,
    /// Who hears of each commit's events, after the commit.
    pub listener: Arc<dyn NodeListener>,
    /// Who is told of each applied and reverted block on the node thread (the digest writer of
    /// the harness), if anyone. The thread first tells it the restored state
    /// ([`CommitObserver::restarted`]).
    pub observer: Option<Box<dyn CommitObserver>>,
    /// Where admitted transactions are broadcast.
    pub broadcast: Arc<dyn TransactionBroadcast>,
}

/// The node thread.
pub struct Node {
    thread: JoinHandle<Result<(), NodeError>>,
}

impl Node {
    /// Start the node thread with `parts`; it publishes its first snapshots before this returns.
    /// Fails only when the operating system cannot start a thread.
    pub fn spawn(mut parts: NodeParts) -> Result<(Node, NodeHandle), NodeError> {
        let (work, queue) = mpsc::unbounded_channel();
        let (state, state_view) = watch::channel(Arc::new(parts.state.view()));
        let (pool, pool_view) = watch::channel(Arc::new(parts.pool.view()));
        let (chain, chain_status) = watch::channel(Arc::new(status(&parts.chain)));
        let observer = parts.observer.take();
        let observer = observer.unwrap_or_else(|| Box::new(NoObserver));
        let publish = Publish { state, pool, chain };
        let node = NodeThread {
            parts,
            observer,
            publish,
        };
        let builder = std::thread::Builder::new().name("heartwood-node".to_owned());
        let thread = builder
            .spawn(move || node.run(queue))
            .map_err(|_| NodeError::Stopped)?;
        let handle = NodeHandle::from_channels(work, state_view, pool_view, chain_status);
        Ok((Node { thread }, handle))
    }

    /// Wait for the thread to end: `Ok` after a stop, the fatal error otherwise.
    pub fn join(self) -> Result<(), NodeError> {
        self.thread.join().unwrap_or(Err(NodeError::Stopped))
    }
}

/// The chain status of `chain`, to publish.
fn status(chain: &SharedChainState) -> ChainStatus {
    let chain = lock(chain);
    let last_blocks = chain.last_blocks.iter().cloned().collect();
    let stored_height = chain.last_stored_height;
    ChainStatus {
        last_blocks,
        stored_height,
    }
}

/// The pool's environment of node parts, which borrows the state, the rules and the clock, not
/// the pool.
macro_rules! env {
    ($parts:expr) => {
        PoolEnv {
            state: &$parts.state,
            milestones: &$parts.milestones,
            network: &$parts.network,
            clock: &*$parts.clock,
        }
    };
}

/// The senders of the published state, pool and chain status.
struct Publish {
    state: watch::Sender<Arc<StateView>>,
    pool: watch::Sender<Arc<PoolView>>,
    chain: watch::Sender<Arc<ChainStatus>>,
}

/// The thread: its parts (the observer taken out), its observer, and its snapshots' senders.
struct NodeThread {
    parts: NodeParts,
    observer: Box<dyn CommitObserver>,
    publish: Publish,
}

impl NodeThread {
    /// Run the work of `queue` until a stop, the end of every handle, or a fatal error.
    fn run(mut self, mut queue: mpsc::UnboundedReceiver<Work>) -> Result<(), NodeError> {
        self.observer.restarted(&self.parts.state);
        let stop = loop {
            match queue.blocking_recv() {
                Some(Work::Stop(answer)) => break Some(answer),
                None => break None,
                Some(work) => {
                    let done = self.work(work);
                    done.inspect_err(|fatal| error!("The node stops: {fatal}"))?;
                }
            }
        };
        drop(queue);
        let mut parts = self.parts;
        parts.pool.save_online(&mut parts.store)?;
        parts.pool.close();
        drop(parts.store);
        if let Some(answer) = stop {
            let _ = answer.send(());
        }
        Ok(())
    }

    fn work(&mut self, work: Work) -> Result<(), NodeError> {
        let (parts, publish) = (&mut self.parts, &self.publish);
        match work {
            Work::Job(job, answer) => {
                let _ = answer.send(self.job(job)?);
            }
            Work::PoolEntries(channel, entries, answer) => {
                let (result, admitted) = parts.pool.process(channel, &env!(parts), entries)?;
                if !admitted.is_empty() {
                    parts.broadcast.broadcast_transactions(&admitted);
                }
                publish.pool.send_replace(Arc::new(parts.pool.view()));
                let _ = answer.send(result);
            }
            Work::Drain(answer, after_readd) => {
                let (saved, mut answer) = (parts.pool.save_online(&mut parts.store), Some(answer));
                if !after_readd || saved.is_err() {
                    let _ = answer.take().map(|answer| answer.send(saved.clone()));
                }
                saved?;
                let readded = parts.pool.readd(&env!(parts), &[]).map(drop);
                publish.pool.send_replace(Arc::new(parts.pool.view()));
                let readded = readded.map_err(NodeError::from);
                if let Some(answer) = answer {
                    let _ = answer.send(readded.clone());
                }
                readded?;
            }
            Work::RemoveBlocks(count, answer) => {
                let removed = self.remove_blocks(count);
                let _ = answer.send(removed.clone());
                removed?;
            }
            Work::Online(change) => {
                parts.pool.online(change);
                let view = parts.pool.view_online(&publish.pool.borrow());
                publish.pool.send_replace(Arc::new(view));
            }
            // The loop in `run` takes the stop before it gets here.
            Work::Stop(answer) => drop(answer),
        }
        Ok(())
    }

    /// Run `job`, then the steps after its commit.
    fn job(&mut self, job: Job) -> Result<JobOutcome, NodeError> {
        let (parts, observer) = (&mut self.parts, &mut *self.observer);
        let outcome = run_job(
            &mut JobContext {
                state: &mut parts.state,
                store: &mut parts.store,
                chain: &parts.chain,
                pool: &mut parts.pool,
                processor: &*parts.processor,
                handlers: &parts.handlers,
                milestones: &parts.milestones,
                clock: &*parts.clock,
                observer,
            },
            job,
        )?;
        let count = usize::try_from(outcome.committed).unwrap_or(usize::MAX);
        let (mut events, mut rounds, mut milestone) = (Vec::new(), Vec::new(), false);
        for undo in parts.state.journal().iter().rev().take(count).rev() {
            let height = undo.block.height();
            let next_round = calculate_round(height, &parts.milestones).next_round;
            let round_started = undo.round.as_ref().map(|_| next_round);
            rounds.extend(round_started);
            if starts_milestone(&parts.milestones, height) {
                events.push(NodeEvent::MilestoneChanged { height });
                milestone = true;
            }
            let applied = NodeEvent::BlockApplied {
                height,
                round_started,
            };
            events.push(applied);
        }
        if count > 0 {
            parts.pool.committed(&env!(parts), &rounds, milestone)?;
            self.publish(&events);
        }
        Ok(outcome)
    }

    /// Remove the last `count` blocks, at most the tip minus one, in one chunk; re-add their
    /// transactions in chain order; check that the stored tip is the state's. Returns the new
    /// tip's height.
    fn remove_blocks(&mut self, count: u32) -> Result<u32, NodeError> {
        let (parts, observer) = (&mut self.parts, &mut *self.observer);
        let tip = parts.state.height();
        let count = count.min(tip.saturating_sub(1));
        let target = tip.saturating_sub(count);
        if count == 0 {
            return Ok(tip);
        }
        let (blocks, reset) = (pluralise("block", count.into(), true), locale(target));
        info!("Removing {blocks}. Reset to height {reset} :warning:");
        let (mut events, mut removed) = (Vec::new(), Vec::new());
        let mut chunk = parts.store.chunk()?;
        while parts.state.height() > target {
            let height = parts.state.height();
            info!("Undoing block {}", locale(height));
            let reverted = parts.state.revert_block(&parts.milestones, &parts.handlers);
            let reverted = reverted.map_err(|error| {
                let reason = error.to_string();
                NodeError::Revert { height, reason }
            })?;
            chunk.reverted(&parts.state, &reverted, &parts.milestones)?;
            observer.reverted(&parts.state);
            removed.extend(reverted.block.transactions().iter().rev().cloned());
            events.push(NodeEvent::BlockReverted { height });
            let height = parts.state.height();
            if starts_milestone(&parts.milestones, height) {
                events.push(NodeEvent::MilestoneChanged { height });
            }
        }
        chunk.commit()?;
        let mut chain = lock(&parts.chain);
        chain.set_last_block(Arc::new(parts.state.last_block().clone()));
        chain.reset_last_downloaded();
        chain.last_stored_height = target;
        drop(chain);
        removed.reverse();
        parts.pool.readd(&env!(parts), &removed)?;
        let stored = parts.store.database().last_block();
        let stored = stored.map_err(StoreError::from)?.map(|stored| stored.row);
        if stored.as_ref().map(|row| row.id) != Some(*parts.state.last_block().id()) {
            let (stored, state) = (stored.map_or(0, |row| row.height), target);
            return Err(NodeError::TipMismatch { stored, state });
        }
        self.publish(&events);
        Ok(target)
    }

    /// Publish the state, the pool and the chain status, then tell the listener `events`.
    fn publish(&self, events: &[NodeEvent]) {
        let (parts, publish) = (&self.parts, &self.publish);
        let view = parts.state.view_after(&publish.state.borrow());
        publish.state.send_replace(Arc::new(view));
        publish.pool.send_replace(Arc::new(parts.pool.view()));
        publish.chain.send_replace(Arc::new(status(&parts.chain)));
        parts.listener.committed(events);
    }
}

/// Whether a milestone starts at `height`, as the reference's `isNewMilestone` reads it.
fn starts_milestone(milestones: &Milestones, height: u32) -> bool {
    let all = milestones.all();
    all.iter().any(|params| params.height() == height)
}

/// A height as the reference's log lines write it.
fn locale(height: u32) -> String {
    to_locale_integer(height.into())
}
