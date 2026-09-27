//! A job: the reference's `ProcessBlocksJob`, run on the node thread.
//!
//! 1. A job without blocks, or whose first block is at or below the tip, does nothing
//!    ([`crate::work::JobEnd::Dropped`]).
//! 2. A first block that does not chain to the tip ends the job with a fork check
//!    ([`crate::work::JobEnd::ForkCheck`]): the service clears the queue, resets the last
//!    downloaded block and checks for a fork in the background.
//! 3. Each block in turn: a block in a future slot ends the loop (the slots have one block time
//!    from height 1, so the reference's lookup, which prefers the timestamps of the blocks accepted
//!    in this job, gives the same slots); otherwise it is rebuilt as the reference's block factory
//!    rebuilds it ([`heartwood_p2p::to_block`]) and processed. An accepted block joins the job's
//!    accepted blocks; `Corrupted` stops the node; any other result ends the loop, and `Rollback`
//!    also marks the block as the forked block and as the last downloaded block. A rebuild error
//!    ends the loop with no result for the block (`Failed to process chunk of block chunk of
//!    blocks [<from>, <to>] on top of <h>`), and the job goes on with the previous block's result.
//! 4. The accepted blocks are committed in one SQLite transaction (the store's chunk), each
//!    block's rows written right after its apply. The stored transaction ids that the processor
//!    checks are read once, for all the job's transactions, before the chunk opens.
//! 5. The end: `Broadcast` when the last result was accepted or discarded but can be broadcast,
//!    `Fork` when a block was marked as forked, else `Clear` ([`crate::work::JobEnd`]).
//!
//! After the commit the node publishes the new snapshot and its events; never between blocks. A
//! failed write or commit is fatal (the node stops; its next start restores the committed tip),
//! where the reference reverts the job's blocks and goes on.

use std::collections::BTreeSet;
use std::sync::Arc;

use heartwood_crypto::crypto::slots;
use heartwood_crypto::utils::js::to_locale_integer;
use heartwood_crypto::{Block, BlockId, Milestones, TransactionId};
use heartwood_kernel::{ChainLink, Clock, debug, error, is_block_chained};
use heartwood_p2p::download::node_hex;
use heartwood_p2p::to_block;
use heartwood_state::State;
use heartwood_transactions::Handlers;

use crate::chain_state::ChainState;
use crate::commit::ChainStore;
use crate::errors::{NodeError, StoreError};
use crate::events::CommitObserver;
use crate::pool_task::PoolTask;
use crate::processor::{BlockProcess, BlockProcessorResult, ForgedIds, ProcessContext};
use crate::work::{IncomingBlock, Job, JobEnd, JobOutcome};

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

impl ForgedIds for BTreeSet<TransactionId> {
    fn is_stored(&self, id: &TransactionId) -> bool {
        self.contains(id)
    }
}

/// Run `job` (see the module documentation). A failed write, commit or pool step, and a
/// corrupted state, are fatal: the node stops, and its next start restores the committed tip.
pub fn run_job(ctx: &mut JobContext<'_>, job: Job) -> Result<JobOutcome, NodeError> {
    use BlockProcessorResult::{Accepted, Corrupted, DiscardedButCanBeBroadcasted, Rollback};
    let outcome = |results, committed, clear_queue, end| JobOutcome {
        results,
        committed,
        clear_queue,
        end,
    };
    let dropped = Ok(outcome(Vec::new(), 0, false, JobEnd::Dropped));
    let (Some(first), Some(last)) = (job.blocks.first(), job.blocks.last()) else {
        return dropped;
    };
    let on = ctx.state.height();
    if first.height() <= on {
        return dropped;
    }
    let locale = |height: u32| to_locale_integer(height.into());
    let (from, to) = (locale(first.height()), locale(last.height()));
    let span = format!("[{from}, {to}] on top of {}", locale(on));
    debug!("Processing chunk of blocks {span}");
    let tip = ChainLink::of(ctx.state.last_block());
    if is_block_chained(&tip, &link(first), ctx.milestones).is_err() {
        return Ok(outcome(Vec::new(), 0, false, JobEnd::ForkCheck(job.blocks)));
    }
    let ids = job
        .blocks
        .iter()
        .flat_map(|incoming| incoming.block.transactions.iter().flatten());
    let ids: Vec<TransactionId> = ids.map(|transaction| *transaction.id()).collect();
    let forged = ctx.store.database().forged_transaction_ids(&ids);
    let forged: BTreeSet<TransactionId> = forged.map_err(StoreError::from)?.into_iter().collect();
    let mut chunk = ctx.store.chunk()?;
    let (mut accepted, mut results, mut clear_queue) = (Vec::new(), Vec::new(), false);
    let (mut last_result, mut last_processed, mut fork) = (None, None, None);
    for incoming in &job.blocks {
        let timestamp = incoming.block.header.header.timestamp.unwrap_or(0);
        if slot(ctx.milestones, timestamp.into()) > current_slot(ctx.milestones, ctx.clock) {
            let height = locale(incoming.height());
            error!("Discarded block {height} because it takes a future slot :crystal_ball:");
            break;
        }
        let block = match to_block(&incoming.block, ctx.milestones) {
            Ok(block) => Arc::new(block),
            Err(why) => {
                error!("Failed to process chunk of block chunk of blocks {span}");
                error!("{why}");
                break;
            }
        };
        let processed = ctx.processor.process(
            &mut ProcessContext {
                state: ctx.state,
                handlers: ctx.handlers,
                milestones: ctx.milestones,
                chain: ctx.chain,
                forged: &forged,
                pending: &accepted,
                pool: ctx.pool,
                clock: ctx.clock,
            },
            &block,
        );
        clear_queue |= processed.unchained.is_some();
        results.push((*block.id(), processed.result));
        last_result = Some(processed.result);
        last_processed = Some(Arc::clone(&block));
        match (processed.result, processed.applied) {
            (Accepted, Some(applied)) => {
                chunk.applied(ctx.state, &block, &applied, ctx.milestones)?;
                ctx.observer.applied_with(ctx.state, &block, &applied);
                accepted.push(Block::clone(&block));
                ctx.chain.set_last_block(block);
            }
            // A block accepted without its apply, or a failed revert: the state and the store
            // may no longer agree.
            (Accepted | Corrupted, _) => {
                error!("Shutting down app, because state is corrupted :boom:");
                return Err(NodeError::Corrupted {
                    height: block.height(),
                });
            }
            (result, _) => {
                if result == Rollback {
                    ctx.chain.last_downloaded = Some(ChainLink::of(&block));
                    fork = Some(block);
                }
                break;
            }
        }
    }
    if let Some(last) = accepted.last() {
        chunk.commit()?;
        ctx.chain.last_stored_height = last.height();
    }
    if let Some(failure) = ctx.pool.take_failure() {
        return Err(NodeError::Pool(failure));
    }
    let broadcast = matches!(last_result, Some(Accepted | DiscardedButCanBeBroadcasted));
    let end = match (last_processed, fork) {
        (Some(block), _) if broadcast => JobEnd::Broadcast(block),
        (_, Some(block)) => JobEnd::Fork(block),
        _ => JobEnd::Clear,
    };
    let committed = u32::try_from(accepted.len()).unwrap_or(u32::MAX);
    Ok(outcome(results, committed, clear_queue, end))
}

/// The chain link of a downloaded block's header, for the chaining test: its ids as Node reads
/// the hex (the reply check has already refused a malformed one), height and timestamp.
fn link(incoming: &IncomingBlock) -> ChainLink {
    let header = &incoming.block.header.header;
    let id = |hex: &Option<String>| {
        let bytes = node_hex(hex.as_deref().unwrap_or_default()).try_into();
        BlockId::from_array(bytes.unwrap_or([0; 32]))
    };
    let (id, previous_block) = (id(&header.id), id(&header.previous_block));
    let (height, timestamp) = (incoming.height(), header.timestamp.unwrap_or(0));
    ChainLink {
        id,
        previous_block,
        height,
        timestamp,
    }
}

/// The slot of `timestamp`; the slots have one block time from height 1.
fn slot(milestones: &Milestones, timestamp: i64) -> i64 {
    slots::slot_number(timestamp, milestones.at(1).block_time())
}

/// The current slot at `clock`. The milestone loader refuses an epoch it cannot read.
fn current_slot(milestones: &Milestones, clock: &dyn Clock) -> i64 {
    let epoch = slots::epoch_unix_ms(milestones.at(1).epoch()).unwrap_or(0);
    slot(milestones, slots::epoch_time(clock.now_ms(), epoch))
}
