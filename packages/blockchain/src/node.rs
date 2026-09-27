//! The node thread: it alone holds the state, the store, the pool and the online record.
//!
//! Every change runs here, in the order the work arrives ([`crate::work::Work`]): jobs, pool
//! entries, the queue's drain, block removals, the online record's changes, and the stop. After
//! each commit the thread publishes the new snapshots (the state, the pool, the chain status)
//! through `watch` channels and tells its listeners, never between two blocks of a job. Network
//! and API tasks send it work through a [`NodeHandle`] and read the snapshots; nothing else reads
//! or writes the state or the store.

use std::sync::Arc;
use std::thread::JoinHandle;

use heartwood_crypto::{Milestones, Network};
use heartwood_kernel::Clock;
use heartwood_state::State;
use heartwood_transactions::Handlers;

use crate::chain_state::SharedChainState;
use crate::commit::ChainStore;
use crate::errors::NodeError;
use crate::events::{CommitObserver, NodeListener};
use crate::options::BlockchainOptions;
use crate::pool_task::{PoolTask, TransactionBroadcast};
use crate::processor::BlockProcess;
use crate::work::NodeHandle;

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
    /// the harness), if anyone.
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
    pub fn spawn(parts: NodeParts) -> Result<(Node, NodeHandle), NodeError> {
        let _ = parts;
        todo!()
    }

    /// Wait for the thread to end: `Ok` after a stop, the fatal error otherwise.
    pub fn join(self) -> Result<(), NodeError> {
        let _ = self.thread;
        todo!()
    }
}
