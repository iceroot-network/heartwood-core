//! Instance resolution: which token, network and role a command is about, and its directories.
//!
//! - `--token` and `--network` name the instance; a missing one is filled from the CLI's
//!   `config.json`, which never overrides a flag that was given.
//! - The five directories come from the kernel's `paths` for `<token>-core/<network>`, each
//!   replaced by its `CORE_PATH_DATA`, `CORE_PATH_CONFIG`, `CORE_PATH_CACHE`, `CORE_PATH_LOG` or
//!   `CORE_PATH_TEMP` variable when set.
//! - The instance's name is `<token>-<network>-<role>`.

use std::path::{Path, PathBuf};

use crate::errors::InstanceError;
pub use crate::seams::{Instance, Role};

/// The flags that name an instance, which every role command takes.
#[derive(Debug, Clone, Default, PartialEq, Eq, clap::Args)]
pub struct InstanceFlags {
    /// The token's name, the first part of the instance's directories and name.
    #[arg(long)]
    pub token: Option<String>,
    /// The network's name.
    #[arg(long)]
    pub network: Option<String>,
}

/// The CLI's `config.json` for `home`, the user's home directory.
pub fn config_path(home: &Path) -> PathBuf {
    let _ = home;
    todo!("the CLI's config.json")
}

/// The instance of `role` that `flags` name, with `env` giving the environment, `home` the
/// user's home directory and `cwd` the working directory.
pub fn resolve(
    role: Role,
    flags: &InstanceFlags,
    env: &dyn Fn(&str) -> Option<String>,
    home: &Path,
    cwd: &Path,
) -> Result<Instance, InstanceError> {
    let _ = (role, flags, env, home, cwd);
    todo!("instance resolution")
}
