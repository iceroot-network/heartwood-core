//! `heartwood-kernel`: the slim kernel of Heartwood Core.
//!
//! What the other crates share, without a service container or runtime plugins:
//!
//! - [`utils`]: the consensus utilities (the round of a height, the forger index of a slot, block
//!   chaining) and the shared helpers (the status canonical JSON, epoch timestamps, IP matching).
//! - [`clock`]: the time seam, [`Clock`], with [`SystemClock`] for nodes and `ManualClock` for
//!   tests.
//! - [`events`]: the event names, [`Event`].
//! - [`lifecycle`]: the component lifecycle, [`lifecycle::Component`].
//! - [`log`]: the eight log levels of the reference and their macros over `tracing`.
//! - [`paths`]: the config, data, cache, log and temp directories of a node.
//! - [`config`]: the `.env` parser and the option merge.
//! - [`errors`]: the crate's error types.
//!
//! Rules: the consensus utilities read the round length and the block time from the milestone at
//! height 1, because neither may change after it; they take every input explicitly and read no
//! clock. Nothing in this crate holds global mutable state.
//!
//! Features: `difftest` adds `ManualClock`, a test seam that only test code enables; a release
//! build never has it.

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

pub mod clock;
pub mod config;
pub mod errors;
pub mod events;
pub mod lifecycle;
pub mod log;
pub mod paths;
pub mod utils;

#[cfg(feature = "difftest")]
pub use crate::clock::ManualClock;
pub use crate::clock::{Clock, SystemClock};
pub use crate::events::Event;
pub use crate::utils::calculate_forging_info::{ForgingInfo, calculate_forging_info};
pub use crate::utils::is_block_chained::{ChainLink, is_block_chained};
pub use crate::utils::round_calculator::{RoundInfo, calculate_round, is_new_round};
