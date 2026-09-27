//! The node's environment, as the reference's local config loader sets it: the process's own
//! variables, then the keys of `<config>/.env` that the process does not have, then every key of
//! `~/.<token>/.env` when that file exists, over both. The reference refuses to start without the
//! home file; a Heartwood relay does not need it.
//!
//! A switch (`CORE_API_DISABLED`, `CORE_API_RATE_LIMIT_DISABLED`, ...) is on when its variable is
//! set to any non-empty value ([`Env::is_set`]), as the reference's `!!process.env.X` reads it.

use std::path::{Path, PathBuf};

use indexmap::IndexMap;
use thiserror::Error;

/// The node's environment: every variable, after the `.env` files.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Env {
    vars: IndexMap<String, String>,
}

/// An `.env` file could not be read.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("cannot read {}: {detail}", path.display())]
pub struct EnvError {
    /// The file.
    pub path: PathBuf,
    /// Why.
    pub detail: String,
}

impl Env {
    /// An environment of exactly `vars`, for tests and for a node without `.env` files.
    pub fn from_vars(vars: impl IntoIterator<Item = (String, String)>) -> Env {
        Env {
            vars: vars.into_iter().collect(),
        }
    }

    /// The value of `name`, if it is set.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.vars.get(name).map(String::as_str)
    }

    /// Whether `name` is set to a non-empty value.
    pub fn is_set(&self, name: &str) -> bool {
        self.get(name).is_some_and(|value| !value.is_empty())
    }

    /// The environment as a lookup function, as the components' defaults read it.
    pub fn lookup(&self) -> impl Fn(&str) -> Option<String> + '_ {
        move |name| self.get(name).map(str::to_owned)
    }
}

/// The environment of a node whose configuration directory is `config_dir`, for the user whose
/// home directory is `home`, with the token `token`, over the process's variables `process`
/// (see the module documentation for the precedence).
pub fn load(
    config_dir: &Path,
    home: &Path,
    token: &str,
    process: impl IntoIterator<Item = (String, String)>,
) -> Result<Env, EnvError> {
    let _ = (config_dir, home, token, process.into_iter());
    todo!()
}
