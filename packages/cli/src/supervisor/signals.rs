//! Locks and signals. A lock is an exclusive advisory lock on its file (`File::try_lock`), held by
//! the process that runs as long as it runs; the node takes `<role>.lock` the same way. Whether a
//! lock is held is probed with a shared lock, released at once, so that probes never disturb each
//! other; a process taking a lock refuses at once when another process holds it exclusively, and
//! tries again while only probes hold it, for at most a second, however often they probe. The
//! supervisor stops its node with SIGINT and kills it with SIGKILL 30 s later; any other process is
//! signalled only while the lock that names it is held, so a stale pid is never signalled. A
//! supervisor writes its pid into its lock file as it takes the lock ([`record_pid`]), and `stop`
//! and `kill` read the supervisor's pid from there while the lock is held ([`held_pid`]), never
//! from the state file, which an earlier supervisor may have left. `kill` signals a daemon
//! supervisor's process group, which holds its node ([`send_group`]).

use std::fs::{File, OpenOptions, TryLockError};
use std::io::{ErrorKind, Read};
use std::os::unix::fs::{FileExt, OpenOptionsExt};
use std::path::Path;
use std::thread;
use std::time::Duration;

use nix::errno::Errno;
use nix::fcntl::OFlag;
use nix::sys::signal::{Signal, kill, killpg};
use nix::unistd::{Pid, getsid};

use crate::errors::ProcessError;

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
/// holds it.
pub fn take_lock(path: &Path) -> Result<Option<File>, ProcessError> {
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .custom_flags(OFlag::O_NOFOLLOW.bits())
        .open(path);
    let file = file.map_err(|e| lock_error(path, &e))?;
    for _ in 0..TAKE_ATTEMPTS {
        match file.try_lock() {
            Ok(()) => return Ok(Some(file)),
            Err(TryLockError::WouldBlock) if held_exclusively(path)? => return Ok(None),
            Err(TryLockError::WouldBlock) => thread::sleep(PROBE_WAIT),
            Err(TryLockError::Error(e)) => return Err(lock_error(path, &e)),
        }
    }
    Ok(None)
}

/// Whether another process holds the lock at `path` exclusively, as a running process holds it:
/// a shared lock is refused then, and granted beside the probes' shared locks.
fn held_exclusively(path: &Path) -> Result<bool, ProcessError> {
    let file = File::open(path).map_err(|e| lock_error(path, &e))?;
    match file.try_lock_shared() {
        Ok(()) => Ok(false),
        Err(TryLockError::WouldBlock) => Ok(true),
        Err(TryLockError::Error(e)) => Err(lock_error(path, &e)),
    }
}

/// Write the pid of this process into the lock file `lock`, which it holds, in place of any pid an
/// earlier holder left.
pub fn record_pid(lock: &File, path: &Path) -> Result<(), ProcessError> {
    let written = lock
        .set_len(0)
        .and_then(|()| lock.write_all_at(std::process::id().to_string().as_bytes(), 0));
    written.map_err(|e| lock_error(path, &e))
}

/// The pid the process that holds the lock at `path` recorded there; `None` when no process
/// holds it or the holder has not recorded its pid within a moment (a lock taken without one).
pub fn held_pid(path: &Path) -> Result<Option<u32>, ProcessError> {
    for _ in 0..HELD_ATTEMPTS {
        if !is_locked(path)? {
            return Ok(None);
        }
        let mut text = String::new();
        let file = File::open(path).and_then(|mut file| file.read_to_string(&mut text));
        file.map_err(|e| lock_error(path, &e))?;
        if let Ok(pid) = text.trim().parse() {
            return Ok(Some(pid));
        }
        thread::sleep(PROBE_WAIT);
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
