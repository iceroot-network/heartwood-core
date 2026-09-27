//! The reference's unchained handler: why a block that does not chain to the tip does not, and
//! its outcome. It resets the last downloaded block and asks for the queue to be cleared.

use heartwood_crypto::Block;

use crate::processor::{BlockProcessorResult, ProcessContext, UnchainedStatus};

/// The unchained status of `block` against the tip, given whether its generator is valid, and
/// its outcome: `DoubleForging` is `Rollback` when the block's delegate is among the round's
/// delegates and `Rejected` otherwise; `AlreadyInBlockchain` and `EqualToLastBlock` are
/// `DiscardedButCanBeBroadcasted`; the others are `Rejected`.
pub fn handle(
    ctx: &mut ProcessContext<'_>,
    block: &Block,
    generator_valid: bool,
) -> (BlockProcessorResult, UnchainedStatus) {
    let _ = (ctx, block, generator_valid);
    todo!()
}
