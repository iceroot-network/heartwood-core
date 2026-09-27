//! `heartwood-core`: the `heartwood` binary's library, a relay's process.
//!
//! - [`cli`]: the command line.
//! - [`app`]: the relay's composition root.
//! - [`lock`]: the process lock.
//! - [`signals`]: the signals and the graceful shutdown.
//!
//! Features: `difftest` adds the harness's flags of `relay run` (a digest line per state change,
//! the applied blocks' bytes, and one seed that skips the address rules), test seams that only the
//! harness's builds enable; a release build never has them.

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

pub mod app;
pub mod cli;
pub mod lock;
pub mod signals;

use std::process::ExitCode;

/// The version a Heartwood node reports: a prerelease of the reference's version.
pub const VERSION: &str = concat!("4.3.1-heartwood.", env!("CARGO_PKG_VERSION"));

/// Run `command`; the process's exit status.
pub fn run(command: cli::Command) -> ExitCode {
    match app::run_relay(command.relay_run()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(1)
        }
    }
}
