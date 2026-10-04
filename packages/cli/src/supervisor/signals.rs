//! Locks and signals. A lock is an exclusive advisory lock on its file (`File::try_lock`), held by
//! the process that runs as long as it runs; the node takes `<role>.lock` the same way. Whether a
//! lock is held is probed with a shared lock, released at once, so that probes never disturb each
//! other; a process taking a lock refuses at once when another process holds it exclusively, and
//! tries again while only probes hold it, for at most a second, however often they probe. The
//! supervisor stops its node with SIGINT and kills it with SIGKILL 30 s later; any other process is
//! signalled only while the lock that names it is held, so a stale pid is never signalled. A holder
//! of a node's lock that is not the node (`snapshot rollback`) removes the role's pid file as it
//! takes the lock, as the node does, so that no pid a dead node left is read while it holds it. A
//! supervisor writes its pid into its lock file as it takes the lock ([`record_pid`]), and `stop`
//! and `kill` read the supervisor's pid from there while the lock is held ([`held_pid`]), never
//! from the state file, which an earlier supervisor may have left. `kill` also signals process
//! groups ([`send_group`]): a daemon supervisor's, which holds its node, and the group of the node
//! a foreground supervisor it has just killed named last, as long as that node still leads one.
//!
//! A node started by a service manager records it beside its lock ([`record_manager`]).
//!
//! The lock files are opened relative to their run directory's descriptor ([`RunDir`]): the
//! functions of a path open the directory it is in without following a link, and their `_in`
//! variants take a directory that was checked and is held, so that a directory swapped after the
//! check is never where a lock lands.

use std::fs::{File, TryLockError};
use std::io::ErrorKind;
use std::os::unix::fs::FileExt;
use std::path::Path;
use std::thread;
use std::time::Duration;

use nix::errno::Errno;
use nix::fcntl::OFlag;
use nix::sys::signal::{Signal, kill, killpg};
use nix::unistd::{Pid, getpgid, getsid};

use super::RunDir;
use super::manager::ManagerRecord;
use crate::errors::ProcessError;
use crate::seams::Instance;

/// How long the supervisor waits after SIGINT before it sends SIGKILL.
pub const KILL_AFTER: Duration = Duration::from_secs(30);
/// How often a process tries to take a lock that only probes hold before it counts the lock as
/// held.
const TAKE_ATTEMPTS: u32 = 100;
/// The pause between those tries, far longer than a probe holds a lock.
const PROBE_WAIT: Duration = Duration::from_millis(10);
/// How often [`held_pid`] looks for the pid a holder is about to record.
const HELD_ATTEMPTS: u32 = 50;

/// Take the lock at `path`, held until the file returned is closed; `None` when another process
/// holds it. The directory is held for the call ([`RunDir::beside`]): a link in its place is
/// refused.
pub fn take_lock(path: &Path) -> Result<Option<File>, ProcessError> {
    take_lock_in(&RunDir::beside(path)?, path)
}

/// [`take_lock`] in the run directory `dir`, which was checked.
pub fn take_lock_in(dir: &RunDir, path: &Path) -> Result<Option<File>, ProcessError> {
    let flags = OFlag::O_WRONLY | OFlag::O_CREAT;
    let file = dir
        .file(path, flags, 0o666)
        .map_err(|e| lock_error(path, &e))?;
    for _ in 0..TAKE_ATTEMPTS {
        match file.try_lock() {
            Ok(()) => return Ok(Some(file)),
            Err(TryLockError::WouldBlock) if is_locked_in(dir, path)? => return Ok(None),
            Err(TryLockError::WouldBlock) => thread::sleep(PROBE_WAIT),
            Err(TryLockError::Error(e)) => return Err(lock_error(path, &e)),
        }
    }
    Ok(None)
}

/// Write the pid of this process into the lock file `lock`, which it holds, in place of any pid an
/// earlier holder left.
pub fn record_pid(lock: &File, path: &Path) -> Result<(), ProcessError> {
    let written = lock
        .set_len(0)
        .and_then(|()| lock.write_all_at(std::process::id().to_string().as_bytes(), 0));
    written.map_err(|e| lock_error(path, &e))
}

/// Record the service manager that started the node whose lock is held, beside the lock: the
/// record `env` names ([`ManagerRecord::from_env`]), written through a temporary file and a
/// rename in `dir`, or an earlier start's record removed when `env` names none, so that `status`
/// and `stop` trust only the record of the node that runs.
pub fn record_manager(
    dir: &RunDir,
    instance: &Instance,
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<(), ProcessError> {
    let path = instance.manager_path();
    match ManagerRecord::from_env(instance, env) {
        Some(record) => record.write_in(dir, &path),
        None => ManagerRecord::remove(&path),
    }
}

/// The pid the process that holds the lock at `path` recorded there; `None` when no process
/// holds it or the holder has not recorded its pid within a moment (a lock taken without one).
pub fn held_pid(path: &Path) -> Result<Option<u32>, ProcessError> {
    let Some(dir) = RunDir::beside_if_there(path)? else {
        return Ok(None);
    };
    for _ in 0..HELD_ATTEMPTS {
        if !is_locked_in(&dir, path)? {
            return Ok(None);
        }
        let text = dir.read(path).map_err(|e| lock_error(path, &e))?;
        if let Ok(pid) = text.trim().parse() {
            return Ok(Some(pid));
        }
        thread::sleep(PROBE_WAIT);
    }
    Ok(None)
}

/// Whether a process holds the lock at `path`.
pub fn is_locked(path: &Path) -> Result<bool, ProcessError> {
    let dir = RunDir::beside_if_there(path)?;
    dir.map_or(Ok(false), |dir| is_locked_in(&dir, path))
}

/// [`is_locked`] in the run directory `dir`: a shared lock is refused while a process holds the
/// lock exclusively, and granted beside the probes' shared locks.
pub fn is_locked_in(dir: &RunDir, path: &Path) -> Result<bool, ProcessError> {
    let file = match dir.file(path, OFlag::O_RDONLY, 0) {
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
    let target = process_id(pid)?;
    match kill(target, signal) {
        Ok(()) => Ok(true),
        Err(Errno::ESRCH) => Ok(false),
        Err(e) => Err(signal_error(pid, &e)),
    }
}

/// Send `signal` to every process of the process group `group`; gives `false` when no process is
/// in it.
pub fn send_group(group: u32, signal: Signal) -> Result<bool, ProcessError> {
    let target = process_id(group)?;
    match killpg(target, signal) {
        Ok(()) => Ok(true),
        Err(Errno::ESRCH) => Ok(false),
        Err(e) => Err(signal_error(group, &e)),
    }
}

/// Whether `pid` leads a session, as a daemon supervisor does once it has left its caller's: its
/// session's one process group is then its own, and holds its node.
pub fn leads_session(pid: u32) -> bool {
    process_id(pid).is_ok_and(|pid| getsid(Some(pid)) == Ok(pid))
}

/// Whether `pid` leads a process group, as the node of a foreground supervisor does.
pub fn leads_group(pid: u32) -> bool {
    process_id(pid).is_ok_and(|pid| getpgid(Some(pid)) == Ok(pid))
}

/// `pid` as a process id to signal: never 0 or negative, which would name a group.
fn process_id(pid: u32) -> Result<Pid, ProcessError> {
    let target = i32::try_from(pid).ok().filter(|&pid| pid > 0);
    let failed = || ProcessError::Signal {
        pid,
        message: "not a process id".into(),
    };
    target.map(Pid::from_raw).ok_or_else(failed)
}

fn signal_error(pid: u32, error: &Errno) -> ProcessError {
    ProcessError::Signal {
        pid,
        message: error.to_string(),
    }
}

fn lock_error(path: &Path, error: &std::io::Error) -> ProcessError {
    ProcessError::Lock {
        path: path.to_path_buf(),
        message: error.to_string(),
    }
}
