//! `config publish` and `config forger`: an instance's configuration files from the built-in
//! template (for any network name; an existing directory refused without `--force`), and the
//! forger's key written into the existing `delegates.json` (its `keys` replaced by the one key,
//! every other key kept, four-space indentation and a final newline, 0600 through a temporary
//! file, fsync and rename; an absent file refused, as the reference's `require` fails).

use std::path::PathBuf;

use thiserror::Error;

use crate::seams::Instance;

/// What `config publish` wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Published {
    /// The configuration directory.
    pub dir: PathBuf,
    /// The files written, in the order written.
    pub files: Vec<PathBuf>,
}

/// A configuration command failed.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ConfigError {
    /// The configuration directory exists and `--force` was not given.
    #[error("{} already exists; pass --force to replace it", dir.display())]
    Exists {
        /// The directory.
        dir: PathBuf,
    },
    /// The key is not 64 hexadecimal characters.
    #[error("the private key is not 64 hexadecimal characters")]
    Key,
    /// A file could not be read or written, or a link is in its place.
    #[error("{} cannot be read or written: {message}", path.display())]
    Io {
        /// The file.
        path: PathBuf,
        /// Why.
        message: String,
    },
    /// The command is not built yet.
    #[error("config {0} is not built yet")]
    NotBuilt(&'static str),
}

/// Write `instance`'s configuration files from the template; with `force`, over an existing
/// directory.
pub fn publish(instance: &Instance, force: bool) -> Result<Published, ConfigError> {
    let _ = (instance, force);
    Err(ConfigError::NotBuilt("publish"))
}

/// Write the forger's private key `key` (64 hex characters, lower-cased) into `instance`'s
/// `delegates.json`.
pub fn forger_private_key(instance: &Instance, key: &str) -> Result<(), ConfigError> {
    let _ = (instance, key);
    Err(ConfigError::NotBuilt("forger"))
}
