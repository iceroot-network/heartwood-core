//! The instance index: one file per started instance, `<data-root>/instances/<name>.json`, where
//! `<data-root>` is the token's data root (the parent of every network's data directory) and
//! `<name>` is [`Instance::name`]. The supervisor writes an instance's entry at its start and
//! removes it at its stop, through the writer that refuses a link in its place; `top` lists the
//! token's entries. An entry carries no pid: the supervisor's pid and its liveness come from its
//! held lock, so an entry whose supervisor is gone is ignored, never trusted.

use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::seams::Instance;

/// The directory of the index under the token's data root.
pub const INDEX_DIR: &str = "instances";

/// One entry of the index: an instance a supervisor started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexEntry {
    /// The instance's name, `<token>-<network>-<role>`.
    pub name: String,
    /// The instance's run directory, where its locks and state are.
    pub run_dir: PathBuf,
    /// The instance's log directory.
    pub log_dir: PathBuf,
}

/// The index could not be read or written.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum IndexError {
    /// An entry or the index's directory could not be read or written, or a link is in its place.
    #[error("{} cannot be read or written: {message}", path.display())]
    Io {
        /// The file or directory.
        path: PathBuf,
        /// Why.
        message: String,
    },
    /// An entry is not valid.
    #[error("{} is not a valid index entry: {reason}", path.display())]
    Entry {
        /// The entry.
        path: PathBuf,
        /// Why.
        reason: String,
    },
    /// The index is not built yet.
    #[error("the instance index is not built yet")]
    NotBuilt,
}

/// The index's directory under `data_root`.
pub fn index_dir(data_root: &Path) -> PathBuf {
    data_root.join(INDEX_DIR)
}

/// The entry of `instance` under `data_root`.
pub fn entry_path(data_root: &Path, instance: &Instance) -> PathBuf {
    index_dir(data_root).join(format!("{}.json", instance.name()))
}

/// The data root of `token`: the directory that holds the data directory of each of its
/// networks, with `env` giving the environment and `home` the home directory.
pub fn data_root(
    token: &str,
    env: &dyn Fn(&str) -> Option<String>,
    home: &Path,
) -> Result<PathBuf, IndexError> {
    let _ = (token, env, home);
    Err(IndexError::NotBuilt)
}

/// Write the entry of `instance` under `data_root`, at the supervisor's start.
pub fn write(data_root: &Path, instance: &Instance) -> Result<(), IndexError> {
    let _ = (data_root, instance);
    Err(IndexError::NotBuilt)
}

/// Remove the entry of `instance` under `data_root`, at the supervisor's stop; none is no error.
pub fn remove(data_root: &Path, instance: &Instance) -> Result<(), IndexError> {
    let _ = (data_root, instance);
    Err(IndexError::NotBuilt)
}

/// The entries of `token`'s instances under `data_root`, across its networks, in name order.
pub fn list(data_root: &Path, token: &str) -> Result<Vec<IndexEntry>, IndexError> {
    let _ = (data_root, token);
    Err(IndexError::NotBuilt)
}
