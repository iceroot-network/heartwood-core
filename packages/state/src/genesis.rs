//! The state after the genesis block, as the reference builds it at its first start.
//!
//! 1. The block must be at height 1 and verify, except for the rule on future timestamps.
//! 2. What it issues: the items of every transfer whose sender key equals the block's generator
//!    key byte for byte; at most `2^63 − 1` in total.
//! 3. Every transaction is bootstrapped through [`crate::TransactionHandlers::bootstrap`]; then
//!    each sender's nonce is the number of its transactions, and it pays the amounts, the fees and
//!    the items of its non-issuing transfers. All credits come before all debits, so a debit fails
//!    exactly when the built balance would be negative, which refuses the block.
//! 4. The delegates' vote balances are built from the voters; none may be negative.
//! 5. Round 1 starts as after the genesis block; the genesis block is not one of its blocks.

use heartwood_crypto::{Block, Milestones};

use crate::errors::GenesisError;
use crate::handlers::TransactionHandlers;
use crate::machine::State;

/// What `genesis` issues: the sum of the items of the transfers sent with the generator's key.
pub fn issued(genesis: &Block) -> Result<u64, GenesisError> {
    let _ = genesis;
    todo!()
}

/// Check `genesis` as the reference does at start: height 1, and every rule of block
/// verification except the one on future timestamps.
pub fn validate(genesis: &Block, milestones: &Milestones) -> Result<(), GenesisError> {
    let _ = (genesis, milestones);
    todo!()
}

/// The state after `genesis` (steps 1 to 5 of the module documentation).
pub fn build<H: TransactionHandlers>(
    genesis: &Block,
    milestones: &Milestones,
    handlers: &H,
) -> Result<State, GenesisError> {
    let _ = (genesis, milestones, handlers);
    todo!()
}
