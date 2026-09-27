//! The block processor: a block's checks in the reference's order, and each handler's outcome
//! and side effects.
//!
//! 1. The block's verification at the current time (its transactions strictly decoded by the
//!    rebuild): `Rejected`.
//! 2. A sender's nonces not consecutive from its wallet's nonce: `Rejected`.
//! 3. The generator: the delegate of the block's key, registered, ranked, and the delegate of the
//!    slot's index in the round's list.
//! 4. Not chained to the tip: the unchained statuses ([`unchained`]).
//! 5. An invalid generator: `Rejected`.
//! 6. A reward other than the round's reward for the block: `Rejected`.
//! 7. Two transactions of one unique type from one sender: `Rejected`.
//! 8. A transaction id already stored, or in a block of this job not stored yet: `Rejected`.
//! 9. The apply ([`handlers::accept`]): the state applies the block with its missed slots; the
//!    pool removes the forged transactions; `Accepted`. A refused apply leaves the state as it was:
//!    `Rejected`.
//!
//! Every refusing handler resets the last downloaded block; the unchained handler also asks for
//! the queue to be cleared. The processor runs on the node thread and never reads the wall clock
//! except through [`ProcessContext::clock`].

pub mod handlers;
pub mod unchained;

use heartwood_crypto::{Block, Milestones, Params, Transaction, TransactionId};
use heartwood_kernel::Clock;
use heartwood_state::State;
use heartwood_state::machine::Applied;
use heartwood_transactions::Handlers;

use crate::chain_state::ChainState;

/// What the processor made of a block, by the reference's names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockProcessorResult {
    /// Applied.
    Accepted,
    /// Not applied, and may still be broadcast: a block the node already has.
    DiscardedButCanBeBroadcasted,
    /// Refused.
    Rejected,
    /// A double-forged block of a round delegate: the node rolls back to recover.
    Rollback,
    /// Reverted after a partial apply; the reference's processor never returns it.
    Reverted,
    /// A revert that failed after a partial apply; unreachable with an atomic apply.
    Corrupted,
}

/// Why a block does not chain to the tip, as the reference's unchained handler names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnchainedStatus {
    /// Higher than the tip plus one.
    NotReadyToAcceptNewHeight,
    /// Lower than the tip.
    AlreadyInBlockchain,
    /// The tip itself.
    EqualToLastBlock,
    /// At the tip's height plus one or the tip's, by a generator that is not the slot's delegate.
    GeneratorMismatch,
    /// A second block for the tip's slot by its scheduled delegate.
    DoubleForging,
    /// An older timestamp than the tip's.
    InvalidTimestamp,
}

/// The stored transaction ids the processor checks against: read once for all the job's
/// transactions before the job's chunk opens (the node thread is the only writer).
pub trait ForgedIds {
    /// Whether a stored block holds the transaction `id`.
    fn is_stored(&self, id: &TransactionId) -> bool;
}

/// The pool steps of an accepted block.
pub trait PoolSteps {
    /// Remove the forged `transaction` (with its sender's earlier transactions), under the rules
    /// `params`. A storage failure is recorded by the pool's side and stops the node after the
    /// job.
    fn remove_forged(&mut self, transaction: &Transaction, params: &Params);
}

/// What the processor reads and changes.
pub struct ProcessContext<'a> {
    /// The state at the tip.
    pub state: &'a mut State,
    /// The transaction handlers.
    pub handlers: &'a Handlers,
    /// The milestones.
    pub milestones: &'a Milestones,
    /// The chain state.
    pub chain: &'a mut ChainState,
    /// The stored transaction ids.
    pub forged: &'a dyn ForgedIds,
    /// The blocks accepted earlier in this job, not committed yet, in height order.
    pub pending: &'a [Block],
    /// The pool.
    pub pool: &'a mut dyn PoolSteps,
    /// The clock of the block's verification and of the accept step.
    pub clock: &'a dyn Clock,
}

/// What processing a block did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Processed {
    /// The result.
    pub result: BlockProcessorResult,
    /// What the apply returned, when the block was applied.
    pub applied: Option<Applied>,
    /// The unchained status, when the block did not chain to the tip.
    pub unchained: Option<UnchainedStatus>,
}

/// Process `block` (see the module documentation for the order of the checks).
pub fn process(ctx: &mut ProcessContext<'_>, block: &Block) -> Processed {
    let _ = (ctx, block);
    todo!()
}

/// The processor as the job calls it: [`BlockProcessor`] in a node, a scripted fake in the
/// runtime's tests.
pub trait BlockProcess: Send + Sync {
    /// Process `block`.
    fn process(&self, ctx: &mut ProcessContext<'_>, block: &Block) -> Processed;
}

/// The block processor.
#[derive(Debug, Clone, Copy, Default)]
pub struct BlockProcessor;

impl BlockProcess for BlockProcessor {
    fn process(&self, ctx: &mut ProcessContext<'_>, block: &Block) -> Processed {
        process(ctx, block)
    }
}
