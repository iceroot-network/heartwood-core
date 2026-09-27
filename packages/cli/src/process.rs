//! The role commands: `start`, `stop`, `restart`, `status` and `log` of one instance.
//!
//! - `start` refuses a running role; it spawns `heartwood supervise` in a new session with an
//!   explicit argv and `/dev/null` as its input, in the foreground when not a daemon.
//! - `stop` refuses a role that does not run; it sends SIGTERM to the supervisor and returns when
//!   the node has exited.
//! - `status` prints the process table row: ID, Name, Version, Status, Uptime, Restarts, CPU, RAM.
//! - `log` prints the last lines of the node's log (15 by default), or of the error capture, and
//!   follows it when asked.

use std::ffi::OsString;
use std::path::PathBuf;

use crate::errors::ProcessError;
use crate::seams::{Env, Instance, ProcessOps, StatusRow};

/// The lines `log` prints by default.
pub const LOG_LINES: usize = 15;

/// Start `instance` under a supervisor, running `argv` (the node's command line, the program
/// first) with `env`; as a daemon, or in the foreground until it stops.
pub fn start(
    instance: &Instance,
    argv: &[OsString],
    env: &Env,
    daemon: bool,
) -> Result<(), ProcessError> {
    let _ = (instance, argv, env, daemon);
    todo!("start")
}

/// Stop `instance` and wait until its node has exited.
pub fn stop(instance: &Instance) -> Result<(), ProcessError> {
    let _ = instance;
    todo!("stop")
}

/// Stop `instance`, then start it again with the argv its supervisor ran and `env`.
pub fn restart(instance: &Instance, env: &Env) -> Result<(), ProcessError> {
    let _ = (instance, env);
    todo!("restart")
}

/// The process table row of `instance`.
pub fn status(instance: &Instance) -> Result<StatusRow, ProcessError> {
    let _ = instance;
    todo!("status")
}

/// Print the last `lines` lines of `instance`'s log, or of its error capture, and follow it.
pub fn log(
    instance: &Instance,
    lines: usize,
    error: bool,
    follow: bool,
) -> Result<(), ProcessError> {
    let _ = (instance, lines, error, follow);
    todo!("log")
}

/// The process operations over the role commands, running `binary` as the supervisor and the node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Supervisors {
    /// The `heartwood` binary.
    pub binary: PathBuf,
}

impl ProcessOps for Supervisors {
    fn start(&self, instance: &Instance, env: &Env) -> Result<(), ProcessError> {
        let _ = (instance, env, &self.binary);
        todo!("a devnet node's start")
    }

    fn stop(&self, instance: &Instance) -> Result<(), ProcessError> {
        let _ = instance;
        todo!("a devnet node's stop")
    }

    fn kill(&self, instance: &Instance) -> Result<(), ProcessError> {
        let _ = instance;
        todo!("a devnet node's kill")
    }

    fn status(&self, instance: &Instance) -> Result<StatusRow, ProcessError> {
        let _ = instance;
        todo!("a devnet node's status")
    }
}
