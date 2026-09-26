//! `heartwood-state`: the consensus state of Heartwood Core, without storage.
//!
//! - [`wallets`]: [`Wallet`] and its [`Delegate`] record, the [`WalletRepository`] with its
//!   public-key and username indexes, and the vote balances.
//! - [`dpos`]: the delegate ranking and the round's delegates.
//! - [`round_state`]: the round, its shuffled forging list, missed slots and the reward of a block
//!   in its round.
//! - [`handlers`]: [`TransactionHandlers`], the seam through which the transaction handlers of
//!   `heartwood-transactions` change wallets.
//! - [`block_state`]: a block's transactions and its forger's payout, applied and reverted.
//! - [`undo`]: what reverting a transaction, a block or a round start needs.
//! - [`machine`]: [`State`], which applies and reverts whole blocks in the reference's order.
//! - [`genesis`]: the state after the genesis block, with its explicit issuance.
//! - [`stores`]: the last block, the genesis block and the supply totals.
//! - [`supply`]: the supply check.
//! - [`digest`]: the canonical state digest, per height and over a replay.
//! - [`errors`]: the crate's error types.
//!
//! Rules: no I/O, no clock and no async. Amounts are `u64` with checked arithmetic; a result the
//! reference would reach only by throwing is a typed error with the same verdict. Every map whose
//! order can be observed is an `IndexMap` or a `BTreeMap`. The state of block `H` is changed by
//! the rules of the milestone at `H`.

#![forbid(unsafe_code)]
#![warn(missing_docs)]
#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )
)]
#![deny(
    clippy::arithmetic_side_effects,
    clippy::float_arithmetic,
    clippy::cast_possible_truncation
)]

pub mod block_state;
pub mod digest;
pub mod dpos;
pub mod errors;
pub mod genesis;
pub mod handlers;
pub mod machine;
pub mod round_state;
pub mod stores;
pub mod supply;
pub mod undo;
pub mod wallets;

pub use crate::handlers::{ApplyContext, TransactionHandlers, TransactionRejected};
pub use crate::machine::State;
pub use crate::round_state::RoundState;
pub use crate::wallets::{Delegate, Resignation, Wallet, WalletRepository};
