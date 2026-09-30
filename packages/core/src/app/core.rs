//! The core role's composition root: one process holding the relay's and the forger's locks of
//! its instance besides its own ([`heartwood_cli::Role::also_locks`]), the relay's components
//! started as [`crate::app::relay`] starts them, then the forger as [`crate::app::forger`] does, on
//! its own thread and runtime, reaching its relay's peer-to-peer listener on loopback (`hosts`
//! defaults to `127.0.0.1:<CORE_P2P_PORT>`; a `remoteAccess` without a loopback address stops the
//! start, naming the option). Shutdown in the core order: the relay's first two steps, the
//! forger's three, then the relay's last two. Logs go to `<token>-<network>-core-current.log`.

use thiserror::Error;

use crate::app::forger::ForgerError;
use crate::app::relay::RelayError;
use crate::commands::core::CoreRun;

/// The core role could not start, or stopped on a fatal error.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CoreError {
    /// The configuration was refused.
    #[error("{0}")]
    Config(String),
    /// The relay's side failed.
    #[error(transparent)]
    Relay(#[from] RelayError),
    /// The forger's side failed.
    #[error(transparent)]
    Forger(#[from] ForgerError),
    /// The role is not built yet.
    #[error("core run is not built yet")]
    NotBuilt,
}

/// Run the core role with the flags `run` until a signal or a fatal error, with the process's
/// environment and directories, as `relay run` and `forger run` do.
pub fn run_core(run: &CoreRun) -> Result<(), CoreError> {
    let _ = run;
    Err(CoreError::NotBuilt)
}
