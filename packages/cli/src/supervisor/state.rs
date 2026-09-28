//! The supervisor's state file, `<role>.state.json`: what `status` and `restart` read.
//!
//! A JSON object: `name`, `status`, `supervisorPid`, `pid`, `argv` (the node's command line, the
//! program first), `startedAt` (Unix milliseconds), `restarts`, `unstable`, `lastExit` and
//! `version`. The supervisor rewrites it whole, through a temporary file and a rename, at every
//! change; it is trusted only while the supervisor's lock is held, except for `errored`.

use std::fs;
use std::io::ErrorKind;
use std::path::Path;

use serde_json::{Value, json};

use super::at;
use crate::errors::ProcessError;
use crate::instance::pick;
use crate::seams::SupervisedStatus;

/// Every status, to read one back by its name.
const STATUSES: [SupervisedStatus; 6] = [
    SupervisedStatus::Launching,
    SupervisedStatus::Online,
    SupervisedStatus::OnlineUnsupervised,
    SupervisedStatus::Stopping,
    SupervisedStatus::Stopped,
    SupervisedStatus::Errored,
];

/// The state a supervisor writes at every change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupervisorState {
    /// The instance's name.
    pub name: String,
    /// The role's status.
    pub status: SupervisedStatus,
    /// The supervisor's pid.
    pub supervisor_pid: u32,
    /// The node's pid, while it runs.
    pub pid: Option<u32>,
    /// The node's command line, the program first, as `restart` runs it again.
    pub argv: Vec<String>,
    /// When the node last started, in Unix milliseconds.
    pub started_at_ms: Option<i64>,
    /// Restarts since the supervisor started.
    pub restarts: u32,
    /// Unstable runs in the first seconds.
    pub unstable: u32,
    /// How the node last exited (`exit status: 3`, `signal: 9 (SIGKILL)`), once it has.
    pub last_exit: Option<String>,
    /// The node's version.
    pub version: String,
}

impl SupervisorState {
    /// Read the state file at `path`; `None` when there is none.
    pub fn read(path: &Path) -> Result<Option<SupervisorState>, ProcessError> {
        let text = match fs::read_to_string(path) {
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
            read => at(path, read)?,
        };
        let invalid = |reason: String| ProcessError::State {
            path: path.to_path_buf(),
            reason,
        };
        let value: Value = serde_json::from_str(&text).map_err(|e| invalid(e.to_string()))?;
        let status: String = pick!(value, "status").map_err(invalid)?;
        let status = STATUSES.into_iter().find(|s| s.name() == status);
        Ok(Some(SupervisorState {
            name: pick!(value, "name").map_err(invalid)?,
            status: status.ok_or_else(|| invalid("an unknown status".into()))?,
            supervisor_pid: pick!(value, "supervisorPid").map_err(invalid)?,
            pid: pick!(value, "pid").map_err(invalid)?,
            argv: pick!(value, "argv").map_err(invalid)?,
            started_at_ms: pick!(value, "startedAt").map_err(invalid)?,
            restarts: pick!(value, "restarts").map_err(invalid)?,
            unstable: pick!(value, "unstable").map_err(invalid)?,
            last_exit: pick!(value, "lastExit").map_err(invalid)?,
            version: pick!(value, "version").map_err(invalid)?,
        }))
    }

    /// Write the state file at `path`, atomically.
    pub fn write(&self, path: &Path) -> Result<(), ProcessError> {
        let value = json!({
            "name": self.name,
            "status": self.status.name(),
            "supervisorPid": self.supervisor_pid,
            "pid": self.pid,
            "argv": self.argv,
            "startedAt": self.started_at_ms,
            "restarts": self.restarts,
            "unstable": self.unstable,
            "lastExit": self.last_exit,
            "version": self.version,
        });
        let temp = path.with_extension("json.tmp");
        at(&temp, fs::write(&temp, format!("{value:#}\n")))?;
        at(path, fs::rename(&temp, path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_state_reads_back_as_written() {
        let dir = std::env::temp_dir().join(format!("hw-cli-state-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("relay.state.json");
        assert_eq!(SupervisorState::read(&path).unwrap(), None);
        for status in STATUSES {
            let state = SupervisorState {
                name: "node1-devnet-relay".into(),
                status,
                supervisor_pid: 41,
                pid: Some(42),
                argv: vec!["heartwood".into(), "relay".into(), "run".into()],
                started_at_ms: Some(1_790_000_000_000),
                restarts: 3,
                unstable: 1,
                last_exit: Some("exit status: 3".into()),
                version: "4.3.1-heartwood.0.1.0".into(),
            };
            state.write(&path).unwrap();
            assert_eq!(SupervisorState::read(&path).unwrap(), Some(state));
        }
        for bad in [
            "{\"status\": \"sleeping\", \"argv\": []}",
            "{\"status\": \"online\", \"argv\": 3}",
            "[]",
        ] {
            fs::write(&path, bad).unwrap();
            assert!(matches!(
                SupervisorState::read(&path),
                Err(ProcessError::State { .. })
            ));
        }
        fs::remove_dir_all(dir).unwrap();
    }
}
