//! The directories of a node: config, data, cache, log and temp.
//!
//! As the reference derives them: the platform's application directories for the name
//! `<token>-core` (XDG directories on Linux, `~/Library` on macOS), each followed by the network
//! name, and each replaced by its `CORE_PATH_CONFIG`, `CORE_PATH_DATA`, `CORE_PATH_CACHE`,
//! `CORE_PATH_LOG` or `CORE_PATH_TEMP` variable when that is set.

use std::path::{Path, PathBuf};

/// The directories of a node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    /// Configuration files.
    pub config: PathBuf,
    /// Databases and other data.
    pub data: PathBuf,
    /// Caches.
    pub cache: PathBuf,
    /// Log files.
    pub log: PathBuf,
    /// Temporary files.
    pub temp: PathBuf,
}

/// The directories for `token` and `network`, reading environment variables through `env` and
/// with `home` as the user's home directory.
pub fn resolve_paths(
    token: &str,
    network: &str,
    env: &dyn Fn(&str) -> Option<String>,
    home: &Path,
) -> Paths {
    let _ = (token, network, env, home);
    todo!()
}
