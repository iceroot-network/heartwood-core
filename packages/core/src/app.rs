//! The relay's composition root: the paths (`CORE_PATH_*`), the environment, `app.json` (the
//! relay's profile), `peers.json`, the crypto files, the version floor, the logger, then the
//! components in rank order (database, state, transactions, pool, p2p, blockchain, api). The
//! `webhooks` entry warns and is not started; an unknown package, or a missing required one, stops
//! the start. It implements the peer-to-peer client's views of the node and the API's submission
//! and status over the node's snapshots and the store's readers.

use thiserror::Error;

use crate::cli::RelayRun;

/// The relay could not start, or stopped on a fatal error.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RelayError {
    /// The configuration was refused.
    #[error("{0}")]
    Config(String),
    /// Another relay of the instance holds the process lock.
    #[error("{0}")]
    Locked(String),
    /// A component failed.
    #[error("{0}")]
    Component(String),
    /// The node stopped on a fatal error.
    #[error("{0}")]
    Fatal(String),
}

/// Run a relay with the flags `run` until a signal or a fatal error.
pub fn run_relay(run: &RelayRun) -> Result<(), RelayError> {
    let _ = run;
    todo!()
}
