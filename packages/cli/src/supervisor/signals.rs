//! Signals: the supervisor stops its node with SIGINT and kills it with SIGKILL 30 s later; a
//! process is signalled only while the lock that names it is held.

use std::path::Path;
use std::time::Duration;

use nix::sys::signal::Signal;

use crate::errors::ProcessError;

/// How long the supervisor waits after SIGINT before it sends SIGKILL.
pub const KILL_AFTER: Duration = Duration::from_secs(30);

/// Send `signal` to `pid`, provided that the lock at `lock` is held (by that process or its
/// supervisor), so that a pid reused by another process is never signalled.
pub fn signal_if_locked(pid: u32, signal: Signal, lock: &Path) -> Result<bool, ProcessError> {
    let _ = (pid, signal, lock);
    todo!("a signal to a locked process")
}
