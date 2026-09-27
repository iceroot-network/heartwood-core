//! The blockchain service: the reference's `Blockchain`, which drives the machine, the job queue
//! and the network around the node thread.
//!
//! - Incoming blocks ([`Blockchain::handle_incoming_block`]): a block in a future slot is
//!   discarded; the block is pinged; when started, `NEWBLOCK`, the block enqueued and
//!   `block.received`; before that, "Block disregarded because blockchain is not ready".
//! - Enqueueing ([`Blockchain::enqueue_blocks`]): jobs cut after 150 transactions, 100 blocks, or
//!   at the first remaining milestone height at or above the last downloaded height (a list that
//!   shifts only when such a cut happens, reproduced); the remaining blocks form a last job, pushed
//!   even when empty; each push resumes the queue unless a fork recovery runs.
//! - Removal ([`Blockchain::remove_blocks`]), the fork block and the job's fork check, `isSynced`,
//!   the wake-up timer (60 s without an accepted block: back to syncing), and the job outcomes and
//!   the queue's drain ([`crate::queue::QueueEvents`]).
//! - The machine ([`Blockchain::dispatch`]) and its actions ([`crate::state_machine`]).

use std::sync::Arc;

use heartwood_crypto::{Block, Milestones};
use heartwood_kernel::errors::BootError;
use heartwood_kernel::{ChainLink, Clock};
use tokio::sync::watch;

use crate::chain_state::SharedChainState;
use crate::errors::NodeError;
use crate::fork_recovery::RollbackDepth;
use crate::network::Network;
use crate::options::BlockchainOptions;
use crate::state_machine::{MachineEvent, MachineState};
use crate::work::{IncomingBlock, NodeHandle};

/// What the service publishes of the machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

/// The blockchain service.
pub struct Blockchain<N> {
    parts: BlockchainParts<N>,
    status: watch::Sender<MachineStatus>,
}

impl<N: Network> Blockchain<N> {
    /// A service of `parts`, its machine uninitialised.
    pub fn new(parts: BlockchainParts<N>) -> Arc<Blockchain<N>> {
        let _ = parts;
        todo!()
    }

    /// Start the machine (`START`) and wait until it is synced or idle; a failed initialise is
    /// the boot's error.
    pub async fn boot(self: &Arc<Self>) -> Result<(), BootError> {
        let _ = (&self.parts, &self.status);
        todo!()
    }

    /// A block from a peer or the node's own forger (see the module documentation);
    /// `fire_event` is false for the idle poll's blocks.
    pub async fn handle_incoming_block(self: &Arc<Self>, block: IncomingBlock, fire_event: bool) {
        let _ = (block, fire_event);
        todo!()
    }

    /// Cut `blocks` into jobs and push them.
    pub fn enqueue_blocks(self: &Arc<Self>, blocks: Vec<IncomingBlock>) {
        let _ = blocks;
        todo!()
    }

    /// Remove the last `count` blocks (at most the tip minus one): clear and stop the queue, then
    /// the node's removal, then the last downloaded block at the new tip. A failed revert stops the
    /// node.
    pub async fn remove_blocks(self: &Arc<Self>, count: u32) -> Result<(), NodeError> {
        let _ = count;
        todo!()
    }

    /// Mark `block` as forked, clear and stop the queue, set the rollback count when given, and
    /// dispatch `FORK`.
    pub fn fork_block(self: &Arc<Self>, block: Arc<Block>, rollback: Option<u32>) {
        let _ = (block, rollback);
        todo!()
    }

    /// The fork check of a job whose first block did not chain: only when no recovery runs and
    /// more than one block time has passed since the last check; whether it rolled back.
    pub async fn check_for_fork(self: &Arc<Self>, blocks: &[IncomingBlock]) -> bool {
        let _ = blocks;
        todo!()
    }

    /// Whether `block` (the tip when `None`) is less than three block times old; true without
    /// peers.
    pub fn is_synced(&self, block: Option<&ChainLink>) -> bool {
        let _ = block;
        todo!()
    }

    /// Dispatch `event` to the machine; its transition's actions run in the background, in order.
    pub fn dispatch(self: &Arc<Self>, event: MachineEvent) {
        let _ = event;
        todo!()
    }

    /// The machine's published status.
    pub fn status(&self) -> watch::Receiver<MachineStatus> {
        self.status.subscribe()
    }

    /// Stop: `STOP`, the idle loops and the wake-up timer ended, the queue stopped after its
    /// running job.
    pub async fn dispose(self: &Arc<Self>) {
        todo!()
    }
}
