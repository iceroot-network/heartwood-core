//! The process lock: `<data>/run/relay.lock`, held for the process's lifetime, so a second relay of
//! the instance refuses to start; and `<data>/run/relay.pid`, the process's id, removed when the
//! lock is released. The run directory is made with mode 0700, refused when another user owns it,
//! and set to 0700 when it is wider; the pid file is always a new file (a link left in its place
//! is removed, never followed).

use std::fs::{self, DirBuilder, File, OpenOptions, Permissions, TryLockError};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use crate::app::RelayError;

/// The held lock; dropping it releases the lock and removes the pid file.
#[derive(Debug)]
pub struct ProcessLock {
    file: File,
    pid: PathBuf,
}

impl ProcessLock {
    /// Take the lock of the node whose data directory is `data`.
    pub fn acquire(data: &Path) -> Result<ProcessLock, RelayError> {
        let run = data.join("run");
        let fail = |path: &Path, error: std::io::Error| {
            RelayError::Locked(format!("{}: {error}", path.display()))
        };
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&run)
            .map_err(|e| fail(&run, e))?;
        let lock = run.join("relay.lock");
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&lock);
        let file = file.map_err(|e| fail(&lock, e))?;
        match file.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => {
                return Err(RelayError::Locked(format!(
                    "another relay of this instance holds {}",
                    lock.display()
                )));
            }
            Err(TryLockError::Error(error)) => return Err(fail(&lock, error)),
        }
        // The lock is held, so a pid file that is there names no running relay.
        let pid = run.join("relay.pid");
        let _ = fs::remove_file(&pid);
        let created = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&pid);
        let mut written = created.map_err(|e| fail(&pid, e))?;
        let uid = written.metadata().map_err(|e| fail(&pid, e))?.uid();
        let directory = fs::metadata(&run).map_err(|e| fail(&run, e))?;
        if directory.uid() != uid {
            let _ = fs::remove_file(&pid);
            return Err(RelayError::Locked(format!(
                "{} belongs to another user",
                run.display()
            )));
        }
        if directory.mode() & 0o077 != 0 {
            let private = Permissions::from_mode(0o700);
            fs::set_permissions(&run, private).map_err(|e| fail(&run, e))?;
        }
        writeln!(written, "{}", std::process::id()).map_err(|e| fail(&pid, e))?;
        Ok(ProcessLock { file, pid })
    }

    /// The lock file, held open.
    pub fn file(&self) -> &File {
        &self.file
    }
}

impl Drop for ProcessLock {
    fn drop(&mut self) {
        // The lock itself goes with the file; a pid file that cannot be removed names no running
        // relay, since liveness is the lock's.
        let _ = fs::remove_file(&self.pid);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_lock_of_one_instance_is_refused_until_the_first_is_dropped() {
        let data = std::env::temp_dir().join(format!("heartwood-lock-{}", std::process::id()));
        let _ = fs::remove_dir_all(&data);
        let first = ProcessLock::acquire(&data).unwrap();
        let pid = fs::read_to_string(data.join("run/relay.pid")).unwrap();
        assert_eq!(pid.trim(), std::process::id().to_string());
        let mode = fs::metadata(data.join("run")).unwrap().mode() & 0o777;
        assert_eq!(mode, 0o700);
        match ProcessLock::acquire(&data) {
            Err(RelayError::Locked(message)) => {
                assert!(message.contains("relay.lock"), "{message}")
            }
            other => panic!("{other:?}"),
        }
        drop(first);
        assert!(!data.join("run/relay.pid").exists());
        let again = ProcessLock::acquire(&data).unwrap();
        assert!(again.file().metadata().is_ok());
        drop(again);
        fs::remove_dir_all(&data).unwrap();
    }

    #[test]
    fn a_left_pid_link_is_replaced_and_a_wide_run_directory_narrowed() {
        let data = std::env::temp_dir().join(format!("heartwood-lock-link-{}", std::process::id()));
        let _ = fs::remove_dir_all(&data);
        let run = data.join("run");
        fs::create_dir_all(&run).unwrap();
        fs::set_permissions(&run, Permissions::from_mode(0o755)).unwrap();
        let target = data.join("elsewhere");
        fs::write(&target, "kept").unwrap();
        std::os::unix::fs::symlink(&target, run.join("relay.pid")).unwrap();
        let lock = ProcessLock::acquire(&data).unwrap();
        assert_eq!(fs::read_to_string(&target).unwrap(), "kept");
        let pid = fs::symlink_metadata(run.join("relay.pid")).unwrap();
        assert!(pid.is_file());
        assert_eq!(fs::metadata(&run).unwrap().mode() & 0o777, 0o700);
        drop(lock);
        fs::remove_dir_all(&data).unwrap();
    }
}
