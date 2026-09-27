//! The kernel's utilities.
//!
//! Three are consensus rules: [`round_calculator`], [`calculate_forging_info`] and
//! [`is_block_chained`]. [`stringify`](mod@stringify) writes the bytes a relay signs for its
//! status. They hold no I/O and no clock, and their arithmetic is checked.
//!
//! The consensus rules read the round length (`activeDelegates`) and the block time from the
//! milestone at height 1: the milestone loader refuses a file that changes either later, so there
//! is a single span of rounds and slots.

#![deny(
    clippy::arithmetic_side_effects,
    clippy::float_arithmetic,
    clippy::cast_possible_truncation
)]

pub mod calculate_forging_info;
pub mod is_block_chained;
pub mod round_calculator;
pub mod stringify;
