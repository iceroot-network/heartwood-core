//! `heartwood-transactions`: the transaction rules of Heartwood Core.
//!
//! - [`handlers`]: one handler per transaction type (1/6 transfer, 1/1 second-signature
//!   registration, 1/2 delegate registration, 1/7 delegate resignation, 2/0 burn, 2/2 vote), each
//!   with its checks in the reference's order, its apply, revert and genesis paths; the base
//!   checks and the dynamic fee floor of a decoded transaction ([`minimum_fee`], a wrapper of
//!   `heartwood_crypto::utils::fee_floor`); and [`Handlers`], the closed registry, which is the
//!   one implementation of `heartwood_state::TransactionHandlers`.
//! - [`pool_query`]: the checks a transaction must pass to enter the pool, and the two views of
//!   the pool and the network they read.
//! - [`errors`]: [`TransactionError`], whose [`TransactionError::class`] is the reference's error
//!   class and [`TransactionError::message`] its message, and [`PoolError`], whose
//!   [`PoolError::code`] is the pool's error code.
//!
//! The registry dispatches on the closed `TransactionType`; there is no registration and no trait
//! object, so no plugin can replace a rule. A handler runs every check before it changes a
//! wallet, so a refused transaction leaves the wallets as they were.

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

pub mod errors;
pub mod handlers;
pub mod pool_query;

pub use crate::errors::{PoolError, TransactionError};
pub use crate::handlers::registry::Handlers;
pub use crate::handlers::transaction::{is_activated, minimum_fee, minimum_fee_with};
pub use crate::pool_query::{OnlineDelegates, PoolQuery, check_pool_entry};
