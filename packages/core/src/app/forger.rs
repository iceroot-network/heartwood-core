//! The forger's composition root: the forger role's configuration (its profile in `app.json`, no
//! `peers.json`), the process lock `<data>/run/forger.lock` taken as the commands probe it, the
//! logger, the key file, the forger's options and its relays, then the forger until a signal or a
//! fatal error. The version floor is checked at the height the relay reports at the first round.

use thiserror::Error;

use crate::commands::forger::ForgerRun;

/// The forger could not start, or stopped on a fatal error.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ForgerError {
    /// The configuration was refused.
    #[error("{0}")]
    Config(String),
    /// Another forger of the instance holds the process lock.
    #[error("{0}")]
    Locked(String),
    /// The forger component failed.
    #[error("{0}")]
    Component(String),
}

/// Run a forger with the flags `run` until a signal or a fatal error.
pub fn run_forger(run: &ForgerRun) -> Result<(), ForgerError> {
    let _ = run;
    todo!("the forger's process")
}
