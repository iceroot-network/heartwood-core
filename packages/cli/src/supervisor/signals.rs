//! Locks and signals. A lock is an exclusive advisory lock on its file (`File::try_lock`), held by
//! the process that runs as long as it runs; the node takes `<role>.lock` the same way. Whether a
//! lock is held is probed with a shared lock, released at once, so that probes never disturb each
//! other; a process taking a lock tries again briefly, in case a probe held it at that moment. The
//! supervisor stops its node with SIGINT and kills it with SIGKILL 30 s later; any other process is
//! signalled only while the lock that names it is held, so a stale pid is never signalled.

use std::fs::{File, OpenOptions, TryLockError};
use std::io::ErrorKind;
use std::path::Path;
use std::thread;
use std::time::Duration;

use nix::errno::Errno;
use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;

use crate::errors::ProcessError;

/// How long the supervisor waits after SIGINT before it sends SIGKILL.
pub const KILL_AFTER: Duration = Duration::from_secs(30);
/// How often a process tries to take a lock before it counts the lock as held.
const TAKE_ATTEMPTS: u32 = 5;
/// The pause between those tries, far longer than a probe holds a lock.
const PROBE_WAIT: Duration = Duration::from_millis(10);

/// Take the lock at `path`, held until the file returned is closed; `None` when another process
/// holds it.
pub fn take_lock(path: &Path) -> Result<Option<File>, ProcessError> {
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path);
    let file = file.map_err(|e| lock_error(path, &e))?;
    for _ in 0..TAKE_ATTEMPTS {
        match file.try_lock() {
            Ok(()) => return Ok(Some(file)),
            Err(TryLockError::WouldBlock) => thread::sleep(PROBE_WAIT),
            Err(TryLockError::Error(e)) => return Err(lock_error(path, &e)),
        }
    }
    Ok(None)
}

/// Whether a process holds the lock at `path`.
pub fn is_locked(path: &Path) -> Result<bool, ProcessError> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(lock_error(path, &e)),
    };
    match file.try_lock_shared() {
        Ok(()) => Ok(false),
        Err(TryLockError::WouldBlock) => Ok(true),
        Err(TryLockError::Error(e)) => Err(lock_error(path, &e)),
    }
}

/// Send `signal` to `pid`, provided that the lock at `lock` is held (by that process or its
/// supervisor), so that a pid reused by another process is never signalled. Gives whether the
/// signal was sent to a live process.
pub fn signal_if_locked(pid: u32, signal: Signal, lock: &Path) -> Result<bool, ProcessError> {
    Ok(is_locked(lock)? && send(pid, signal)?)
}

/// Send `signal` to `pid`; gives `false` when no such process exists.
pub fn send(pid: u32, signal: Signal) -> Result<bool, ProcessError> {
    let failed = |message: String| ProcessError::Signal { pid, message };
    let target = i32::try_from(pid).ok().filter(|&pid| pid > 0);
    let target = target.ok_or_else(|| failed("not a process id".into()))?;
    match kill(Pid::from_raw(target), signal) {
        Ok(()) => Ok(true),
        Err(Errno::ESRCH) => Ok(false),
        Err(e) => Err(failed(e.to_string())),
    }
}

fn lock_error(path: &Path, error: &std::io::Error) -> ProcessError {
    ProcessError::Lock {
        path: path.to_path_buf(),
        message: error.to_string(),
    }
}
