//! `app.json`: the components of a process's role, in file order, with their options.
//!
//! Each component of the role's profile (`relay`, `forger`, `core`) names its package and gives
//! options, which are merged over the component's defaults (arrays replaced, see
//! [`crate::config::merge`]) and read as the reference's option schemas read them: numbers from
//! strings, booleans from `"true"` and `"false"`, and unknown keys logged, not refused. A profile's
//! `flags` and an `app.js` beside the file are not read; the start warns about them. Each
//! component's options type implements [`Options`].

use std::path::{Path, PathBuf};

use serde_json::Value;
use thiserror::Error;

use crate::config::env::Env;

/// The role of a process, and so its profile in `app.json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// A relay: the chain, the pool, the peers and the API, without forging.
    Relay,
    /// A forger.
    Forger,
    /// A relay and a forger in one process.
    Core,
}

impl Role {
    /// The profile's key in `app.json`.
    pub const fn profile(self) -> &'static str {
        match self {
            Role::Relay => "relay",
            Role::Forger => "forger",
            Role::Core => "core",
        }
    }
}

/// One component of a profile.
#[derive(Debug, Clone, PartialEq)]
pub struct PluginEntry {
    /// The package name, such as `@solar-network/api`.
    pub package: String,
    /// The options the file gives, before the defaults.
    pub options: Value,
}

/// A role's profile of `app.json`.
#[derive(Debug, Clone, PartialEq)]
pub struct AppConfig {
    /// The components, in file order.
    pub plugins: Vec<PluginEntry>,
    /// What the file holds that is not read (a profile's `flags`, an `app.js`), for the start's
    /// warnings.
    pub ignored: Vec<String>,
}

/// `app.json` could not be read as a profile.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum AppConfigError {
    /// The file could not be read.
    #[error("cannot read {}: {detail}", path.display())]
    Read {
        /// The file.
        path: PathBuf,
        /// Why.
        detail: String,
    },
    /// The file is not JSON, or not of the reference's shape.
    #[error("{} is not a valid app.json: {detail}", path.display())]
    Shape {
        /// The file.
        path: PathBuf,
        /// What is wrong.
        detail: String,
    },
    /// The file has no profile for the role.
    #[error("app.json has no {profile} profile")]
    NoProfile {
        /// The role's profile key.
        profile: &'static str,
    },
}

/// A component's options could not be read as the reference's schema reads them.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("the {package} option {key} is not valid: {reason}")]
pub struct OptionsError {
    /// The component's package.
    pub package: &'static str,
    /// The option's key, as a dotted path.
    pub key: String,
    /// Why it is refused.
    pub reason: String,
}

/// A component's options, read from its `app.json` entry.
pub trait Options: Sized {
    /// The component's package name.
    const PACKAGE: &'static str;

    /// The options of `value` (the entry's options merged over the defaults), with the variables
    /// of `env` that the component's defaults read.
    fn from_options(value: &Value, env: &Env) -> Result<Self, OptionsError>;
}

/// The profile of `role` in `<config_dir>/app.json`.
pub fn load(config_dir: &Path, role: Role) -> Result<AppConfig, AppConfigError> {
    let _ = (config_dir, role);
    todo!()
}
