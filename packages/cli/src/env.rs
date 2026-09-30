//! `env get|list|set|paths`: an instance's `<config>/.env`, read with the kernel's parser (the one
//! the node reads it with) and written as the reference's `envfile` writes it
//! (`stringifySync(...).trim()`), through the writer that refuses a link in its place.

use std::path::PathBuf;

use thiserror::Error;

use crate::seams::Instance;

/// The environment file could not be read or written.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum EnvError {
    /// There is no environment file, as the reference says it.
    #[error("No environment file found at {}", path.display())]
    NoFile {
        /// Where it was looked for.
        path: PathBuf,
    },
    /// The file could not be read or written, or a link is in its place.
    #[error("{} cannot be read or written: {message}", path.display())]
    Io {
        /// The file.
        path: PathBuf,
        /// Why.
        message: String,
    },
    /// The command is not built yet.
    #[error("env {0} is not built yet")]
    NotBuilt(&'static str),
}

/// The value of `key` in `instance`'s environment file; `None` when it has none.
pub fn get(instance: &Instance, key: &str) -> Result<Option<String>, EnvError> {
    let _ = (instance, key);
    Err(EnvError::NotBuilt("get"))
}

/// Every variable of `instance`'s environment file, in the file's order.
pub fn list(instance: &Instance) -> Result<Vec<(String, String)>, EnvError> {
    let _ = instance;
    Err(EnvError::NotBuilt("list"))
}

/// Set `key` to `value` in `instance`'s environment file, rewriting it as `envfile` does.
pub fn set(instance: &Instance, key: &str, value: &str) -> Result<(), EnvError> {
    let _ = (instance, key, value);
    Err(EnvError::NotBuilt("set"))
}

/// `instance`'s five directories by name: config, data, cache, log and temp.
pub fn paths(instance: &Instance) -> [(&'static str, PathBuf); 5] {
    let paths = &instance.paths;
    [
        ("config", paths.config.clone()),
        ("data", paths.data.clone()),
        ("cache", paths.cache.clone()),
        ("log", paths.log.clone()),
        ("temp", paths.temp.clone()),
    ]
}
