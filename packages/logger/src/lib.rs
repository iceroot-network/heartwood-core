//! `heartwood-logger`: the log output of a Heartwood Core process.
//!
//! It installs the `tracing` subscriber that receives the events of the kernel's log macros and
//! writes them as the reference does:
//!
//! - [`format`](mod@format): the line format of the reference's pretty printer, with its level colours and
//!   emoji codes. Console lines are coloured; file lines are not.
//! - [`rotation`]: the log files: `<stem>-current.log`, rotated daily into
//!   `<stem>-<YYYY-MM-DD>[.<n>].log.gz`, keeping at most 10 rotated files and 100 MiB.
//! - [`driver`]: [`LineLayer`], the `tracing` layer that writes the lines, and [`init`], which
//!   installs it with the console and file outputs for the levels of [`LoggerOptions`].
//! - [`errors`]: the crate's error type.
//!
//! The levels are the kernel's, [`Level`].

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

pub mod driver;
pub mod errors;
pub mod format;
pub mod rotation;

pub use heartwood_kernel::log::Level;

pub use crate::driver::{LineLayer, LoggerGuard, LoggerOptions, init};
pub use crate::errors::LoggerError;
