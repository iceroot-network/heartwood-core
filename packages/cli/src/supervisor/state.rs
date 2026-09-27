//! The supervisor's state file, `<role>.state.json`: what `status` reads.

use std::path::Path;

use crate::errors::ProcessError;
use crate::seams::SupervisedStatus;

/// The state a supervisor writes at every change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupervisorState {
    /// The role's status.
    pub status: SupervisedStatus,
    /// The supervisor's pid.
    pub supervisor_pid: u32,
    /// The node's pid, while it runs.
    pub pid: Option<u32>,
    /// When the node last started, in Unix milliseconds.
    pub started_at_ms: Option<i64>,
    /// Restarts since the supervisor started.
    pub restarts: u32,
    /// The node's version.
    pub version: String,
}

impl SupervisorState {
    /// Read the state file at `path`; `None` when there is none.
    pub fn read(path: &Path) -> Result<Option<SupervisorState>, ProcessError> {
        let _ = path;
        todo!("the state file's read")
    }

    /// Write the state file at `path`, atomically.
    pub fn write(&self, path: &Path) -> Result<(), ProcessError> {
        let _ = path;
        todo!("the state file's write")
    }
}
