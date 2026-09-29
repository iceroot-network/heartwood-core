//! The handlers of the processor's outcomes other than the unchained one: the refusals, each with
//! its reset of the last downloaded block, and the accept step.

use std::sync::Arc;

use heartwood_crypto::Block;
use heartwood_kernel::ChainLink;
use heartwood_state::errors::ApplyError;
use serde_json::Value;

use crate::processor::{
    BlockProcessorResult, ProcessContext, Processed, locale, reset_last_downloaded,
};

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

impl Refusal {
    /// The class name of the reference's handler of this refusal.
    pub const fn handler_name(self) -> &'static str {
        match self {
            Refusal::VerificationFailed => "VerificationFailedHandler",
            Refusal::NonceOutOfOrder => "NonceOutOfOrderHandler",
            Refusal::InvalidGenerator => "InvalidGeneratorHandler",
            Refusal::InvalidReward => "InvalidRewardHandler",
            Refusal::ConflictingTransactions => "ConflictingTransactionsHandler",
            Refusal::AlreadyForged => "AlreadyForgedHandler",
        }
    }
}

/// Refuse `block` for `refusal`: the reset of the last downloaded block; `Rejected`. The checks
/// that refuse have logged why.
pub fn refuse(
    ctx: &mut ProcessContext<'_>,
    block: &Block,
    refusal: Refusal,
) -> BlockProcessorResult {
    heartwood_kernel::debug!(
        "Block {} refused by the {}",
        locale(block.height()),
        refusal.handler_name()
    );
    reset_last_downloaded(ctx.state, ctx.chain);
    BlockProcessorResult::Rejected
}

/// Apply `block`: the state applies it with its missed slots; on success the forked block is
/// cleared when this is its height ("Successfully recovered from fork"), the pool removes the
/// forged transactions, the accept time is recorded for the wake-up timer, and the last
/// downloaded block moves up to the block; on a refused apply, "Refused new block with id ..."
/// with the bad transaction or block, and the reset of the last downloaded block.
///
/// The apply is atomic: a refused block leaves the state as it was, so the reference's revert of
/// a half-applied block has nothing to do. Only a failure to take back a half-applied block,
/// which no consistent state reaches, is `Corrupted`.
pub fn accept(ctx: &mut ProcessContext<'_>, block: &Block) -> Processed {
    let height = block.height();
    match ctx.state.apply_block(block, ctx.milestones, ctx.handlers) {
        Ok(applied) => {
            ctx.chain.set_last_block(Arc::new(block.clone()));
            if ctx
                .chain
                .forked_block
                .as_ref()
                .is_some_and(|forked| forked.height() == height)
            {
                heartwood_kernel::info!("Successfully recovered from fork :star2:");
                ctx.chain.clear_forked_block();
            }
            let params = ctx.milestones.at(height);
            for transaction in block.transactions() {
                ctx.pool.remove_forged(transaction, params);
            }
            // The wake-up timer restarts at each accepted block once the blockchain has started.
            if ctx.chain.started {
                ctx.chain.last_accepted_ms = Some(ctx.clock.now_ms());
            }
            if ctx
                .chain
                .last_downloaded
                .is_some_and(|downloaded| downloaded.height < height)
            {
                ctx.chain.last_downloaded = Some(ChainLink::of(block));
            }
            Processed {
                result: BlockProcessorResult::Accepted,
                applied: Some(applied),
                unchained: None,
                fault: false,
            }
        }
        Err(ApplyError::RollbackFailed(error)) => {
            heartwood_kernel::error!(
                "Critical error occurs when reverting block at height {}. Possible state corruption. Message: {error}",
                locale(height)
            );
            not_applied(BlockProcessorResult::Corrupted, false)
        }
        // A refused apply of a block that chains, by its slot's delegate, is no honest block.
        Err(error) => {
            refused_apply(ctx, block, &error);
            reset_last_downloaded(ctx.state, ctx.chain);
            not_applied(BlockProcessorResult::Rejected, true)
        }
    }
}

/// A block that was not applied, with `result` and whether it was refused for a `fault`.
fn not_applied(result: BlockProcessorResult, fault: bool) -> Processed {
    Processed {
        result,
        applied: None,
        unchained: None,
        fault,
    }
}

/// The reference's log lines of a refused apply, which the devnet tooling reads: the refused
/// block's id, then the bad transaction's data or the block's header data with its height.
fn refused_apply<E: std::fmt::Display>(
    ctx: &ProcessContext<'_>,
    block: &Block,
    error: &ApplyError<E>,
) {
    heartwood_kernel::warning!(
        "Refused new block with id {} :warning: :warning: :warning:",
        block.id()
    );
    let params = ctx.milestones.at(block.height());
    if let ApplyError::Transaction {
        index, rejected, ..
    } = error
    {
        heartwood_kernel::warning!("Block contains a bad transaction: {rejected} :no_entry:");
        let data = block
            .transactions()
            .get(*index)
            .map_or(Value::Null, |transaction| transaction.to_json(params));
        heartwood_kernel::warning!("Bad transaction data: {data}");
    } else {
        heartwood_kernel::warning!("Block is bad: {error} :no_entry:");
        let mut data = block.to_json(ctx.milestones);
        if let Value::Object(fields) = &mut data {
            fields.shift_remove("transactions");
        }
        heartwood_kernel::warning!("Bad block data: {data}");
    }
}
