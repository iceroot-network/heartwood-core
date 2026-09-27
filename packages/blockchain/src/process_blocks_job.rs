//! A job: the reference's `ProcessBlocksJob`, run on the node thread.
//!
//! 1. A job whose first block is at or below the tip does nothing
//!    ([`crate::work::JobEnd::Dropped`]).
//! 2. A first block that does not chain to the tip ends the job with a fork check
//!    ([`crate::work::JobEnd::ForkCheck`]): the service clears the queue, resets the last
//!    downloaded block and checks for a fork in the background.
//! 3. Each block in turn: a block in a future slot (against a slot lookup that prefers the
//!    timestamps of blocks accepted in this job) ends the loop; otherwise it is rebuilt as the
//!    reference's block factory rebuilds it ([`heartwood_p2p::to_block`]) and processed. An
//!    accepted block joins the job's accepted blocks; `Corrupted` stops the node; any other result
//!    ends the loop, and `Rollback` also marks the block as the forked block. A rebuild or
//!    processing error ends the loop with no result for the block (`Failed to process chunk of
//!    block chunk of blocks [<from>, <to>] on top of <h>`), and the job goes on with the previous
//!    block's result.
//! 4. The accepted blocks are committed in one SQLite transaction (the store's chunk), each
//!    block's rows written right after its apply.
//! 5. The end: `Broadcast` when the last result was accepted or discarded but can be broadcast,
//!    `Fork` when a block was marked as forked, else `Clear` ([`crate::work::JobEnd`]).
//!
//! After the commit the node publishes the new snapshot and its events; never between blocks.

use heartwood_crypto::Milestones;
use heartwood_kernel::Clock;
use heartwood_state::State;
use heartwood_transactions::Handlers;

use crate::chain_state::ChainState;
use crate::commit::ChainStore;
use crate::errors::NodeError;
use crate::events::CommitObserver;
use crate::pool_task::PoolTask;
use crate::processor::BlockProcess;
use crate::work::{Job, JobOutcome};

/// What a job reads and changes on the node thread.
pub struct JobContext<'a> {
    /// The state.
    pub state: &'a mut State,
    /// The store.
    pub store: &'a mut ChainStore,
    /// The chain state.
    pub chain: &'a mut ChainState,
    /// The pool and the online record.
    pub pool: &'a mut PoolTask,
    /// The block processor.
    pub processor: &'a dyn BlockProcess,
    /// The transaction handlers.
    pub handlers: &'a Handlers,
    /// The milestones.
    pub milestones: &'a Milestones,
    /// The clock.
    pub clock: &'a dyn Clock,
    /// Who is told of each applied block, on the node thread.
    pub observer: &'a mut dyn CommitObserver,
}

/// Run `job` (see the module documentation). A failed commit or a failed revert is fatal: the
/// node stops, and its next start restores the committed tip.
pub fn run_job(ctx: &mut JobContext<'_>, job: Job) -> Result<JobOutcome, NodeError> {
    let _ = (ctx, job);
    todo!()
}
