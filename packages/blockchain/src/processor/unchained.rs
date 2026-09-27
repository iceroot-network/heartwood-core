//! The reference's unchained handler: why a block that does not chain to the tip does not, and
//! its outcome. It resets the last downloaded block and asks for the queue to be cleared, which
//! the job does when a result carries an unchained status.

use heartwood_crypto::Block;

use crate::processor::{
    BlockProcessorResult, ProcessContext, UnchainedStatus, locale, reset_last_downloaded,
    round_forgers,
};

/// The unchained status of `block` against the tip, given its delegate name and whether its
/// generator is valid, and its outcome: `DoubleForging` is `Rollback` when the block's delegate is
/// among the round's delegates and `Rejected` otherwise; `AlreadyInBlockchain` and `EqualToLastBlock` are
/// `DiscardedButCanBeBroadcasted`; the others are `Rejected`.
pub fn handle(
    ctx: &mut ProcessContext<'_>,
    block: &Block,
    name: Option<&str>,
    generator_valid: bool,
) -> (BlockProcessorResult, UnchainedStatus) {
    reset_last_downloaded(ctx.state, ctx.chain);
    let status = status(ctx, block, name, generator_valid);
    let result = match status {
        UnchainedStatus::DoubleForging => {
            let forgers = round_forgers(ctx.state, block.height(), ctx.milestones);
            if forgers
                .iter()
                .any(|forger| Some(forger.username.as_str()) == name)
            {
                BlockProcessorResult::Rollback
            } else {
                BlockProcessorResult::Rejected
            }
        }
        UnchainedStatus::NotReadyToAcceptNewHeight
        | UnchainedStatus::GeneratorMismatch
        | UnchainedStatus::InvalidTimestamp => BlockProcessorResult::Rejected,
        UnchainedStatus::AlreadyInBlockchain | UnchainedStatus::EqualToLastBlock => {
            BlockProcessorResult::DiscardedButCanBeBroadcasted
        }
    };
    (result, status)
}

/// Why `block` does not chain to the tip, in the reference's order of checks.
fn status(
    ctx: &ProcessContext<'_>,
    block: &Block,
    name: Option<&str>,
    generator_valid: bool,
) -> UnchainedStatus {
    let last = ctx.state.last_block();
    let (height, last_height) = (block.height(), last.height());
    let shown = locale(height);
    if u64::from(height) > u64::from(last_height).saturating_add(1) {
        heartwood_kernel::debug!(
            "Blockchain not ready to accept new block at height {shown}. Last block: {} :warning:",
            locale(last_height)
        );
        UnchainedStatus::NotReadyToAcceptNewHeight
    } else if height < last_height {
        heartwood_kernel::debug!(
            "Block {shown} disregarded because already in blockchain :warning:"
        );
        UnchainedStatus::AlreadyInBlockchain
    } else if height == last_height && block.id() == last.id() {
        heartwood_kernel::debug!("Block {shown} just received :chains:");
        UnchainedStatus::EqualToLastBlock
    } else if block.header().timestamp < last.header().timestamp {
        heartwood_kernel::debug!(
            "Block {shown} disregarded, because the timestamp is lower than the previous timestamp :stopwatch:"
        );
        UnchainedStatus::InvalidTimestamp
    } else if generator_valid {
        heartwood_kernel::warning!(
            "Detected double forging by {} :chains:",
            name.unwrap_or_default()
        );
        UnchainedStatus::DoubleForging
    } else {
        heartwood_kernel::info!(
            "Forked block disregarded because it is not allowed to be forged. Caused by delegate {} :bangbang:",
            name.unwrap_or("undefined")
        );
        UnchainedStatus::GeneratorMismatch
    }
}
