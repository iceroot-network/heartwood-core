//! `service install|uninstall`: a systemd unit for one role of an instance (a user unit by
//! default, a system unit with `--system`), written only: no service manager is called, and the
//! commands to enable the unit are printed. The unit sets the service-manager variables the start
//! records ([`crate::supervisor::manager`]), so `status` names the manager and `stop` refuses.

use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::seams::{Instance, Role};

/// Where a unit goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Manager {
    /// A systemd user unit.
    SystemdUser,
    /// A systemd system unit.
    SystemdSystem,
}

/// What `service install` wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    /// The unit file.
    pub path: PathBuf,
    /// The unit's name.
    pub unit: String,
}

/// A service command failed.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ServiceError {
    /// A file could not be read or written, or a link is in its place.
    #[error("{} cannot be read or written: {message}", path.display())]
    Io {
        /// The file.
        path: PathBuf,
        /// Why.
        message: String,
    },
    /// The command is not built yet.
    #[error("service {0} is not built yet")]
    NotBuilt(&'static str),
}

/// The unit's text for `role` of `instance` under `manager`, running `binary`.
pub fn render(
    instance: &Instance,
    role: Role,
    manager: Manager,
    binary: &Path,
) -> Result<String, ServiceError> {
    let _ = (instance, role, manager, binary);
    Err(ServiceError::NotBuilt("render"))
}

/// Write the unit of `role` of `instance` into `dir`, or the manager's own unit directory.
pub fn install(
    instance: &Instance,
    role: Role,
    manager: Manager,
    binary: &Path,
    dir: Option<&Path>,
) -> Result<Installed, ServiceError> {
    let _ = (instance, role, manager, binary, dir);
    Err(ServiceError::NotBuilt("install"))
}

/// Remove the unit of `role` of `instance` from `dir`, or the manager's own unit directory.
pub fn uninstall(
    instance: &Instance,
    role: Role,
    manager: Manager,
    dir: Option<&Path>,
) -> Result<(), ServiceError> {
    let _ = (instance, role, manager, dir);
    Err(ServiceError::NotBuilt("uninstall"))
}
