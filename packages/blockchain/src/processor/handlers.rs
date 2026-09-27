//! The handlers of the processor's outcomes other than the unchained one: the refusals, each with
//! its reset of the last downloaded block and its log lines, and the accept step.

use heartwood_crypto::Block;

use crate::processor::{BlockProcessorResult, ProcessContext, Processed};

/// Why the processor refused a block, by the reference's handler.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// The block's verification failed.
    VerificationFailed,
    /// A sender's nonces are not consecutive.
    NonceOutOfOrder,
    /// The generator is not the slot's delegate.
    InvalidGenerator,
    /// The reward is not the round's reward for the block.
    InvalidReward,
    /// Two transactions of one unique type from one sender.
    ConflictingTransactions,
    /// A transaction id already stored or pending in the job.
    AlreadyForged,
}

/// Refuse `block` for `refusal`: the handler's log lines and the reset of the last downloaded
/// block; `Rejected`.
pub fn refuse(
    ctx: &mut ProcessContext<'_>,
    block: &Block,
    refusal: Refusal,
) -> BlockProcessorResult {
    let _ = (ctx, block, refusal);
    todo!()
}

/// Apply `block`: the state applies it with its missed slots; on success the forked block is
/// cleared when this is its height ("Successfully recovered from fork"), the pool removes the
/// forged transactions, the accept time is recorded for the wake-up timer, and the last
/// downloaded block moves up to the block; on a refused apply, "Refused new block with id ..."
/// with the bad transaction or block, and the reset of the last downloaded block.
pub fn accept(ctx: &mut ProcessContext<'_>, block: &Block) -> Processed {
    let _ = (ctx, block);
    todo!()
}
