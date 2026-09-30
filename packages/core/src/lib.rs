//! `heartwood-core`: the `heartwood` binary's library, a relay's and a forger's process.
//!
//! - [`commands`]: the command tree, one module per family (`relay`, `forger`, `network`,
//!   `devnet`, the hidden `supervise`).
//! - [`config`]: both roles' configuration, read before anything starts; [`options`]: the
//!   components' options the relay reads itself.
//! - [`app`]: the relay's composition root with its P2P server ([`app::relay`]), and the
//!   forger's ([`app::forger`]); [`views`]: the node as the peer-to-peer client and the API read
//!   it; [`node_services`]: the node as the peer-to-peer server reads it; [`network`]: the network
//!   monitor as the blockchain service calls it.
//! - [`doctor_view`]: the node as `heartwood doctor` reads it, through the node's own loaders.
//! - [`lock`]: the process lock of each role.
//! - [`signals`]: the signals and the graceful shutdown.
//! - `harness` (feature `difftest`): the harness's digest and block files, its network-state
//!   record and its seed.
//!
//! Features: `devnet` (on by default) adds the `devnet` commands and a devnet node's addressing.
//! `difftest` adds the harness's flags of `relay run` (a digest line per state change, the applied
//! blocks' bytes, and one seed that skips the address rules) and its directory variable, test
//! seams that only the harness's builds enable; a release build never has them.

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
pub mod commands;
pub mod config;
pub mod doctor_view;
#[cfg(feature = "difftest")]
pub mod harness;
pub mod lock;
pub mod markers;
pub mod network;
pub mod node_services;
pub mod options;
pub mod signals;
pub mod views;

use std::process::ExitCode;

/// The version a Heartwood node reports: a prerelease of the reference's version.
pub const VERSION: &str = concat!("4.3.1-heartwood.", env!("CARGO_PKG_VERSION"));

/// The exit status of a command's `result`: 0, or 1 with the error printed.
pub fn exit<E: std::fmt::Display>(result: Result<(), E>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(1)
        }
    }
}
