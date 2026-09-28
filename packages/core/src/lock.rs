//! The process lock of a role: `<data>/run/<role>.lock` (`relay.lock`, `forger.lock`), held for
//! the process's lifetime, so a second process of the role of one instance refuses to start; and
//! `<data>/run/<role>.pid`, the process's id, removed when the lock is released. The lock is taken
//! as the commands take theirs ([`heartwood_cli::supervisor::signals::take_lock`]), which tries
//! again past the short shared locks of the commands' probes, so a node started while `status`
//! looks at its lock does not refuse. The run directory is made with mode 0700, refused when
//! another user owns it, and set to 0700 when it is wider; the pid file is always a new file (a
//! link left in its place is removed, never followed).

use std::fs::{self, DirBuilder, File, OpenOptions, Permissions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use heartwood_cli::supervisor::signals::take_lock;
use thiserror::Error;

/// The lock could not be taken.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum LockError {
    /// Another process of the role of the instance holds it.
    #[error("another {role} of this instance holds {}", path.display())]
    Held {
        /// The role.
        role: &'static str,
        /// The lock file.
        path: PathBuf,
    },
    /// A file of the lock could not be made or read.
    #[error("{}: {message}", path.display())]
    Io {
        /// The file or directory.
        path: PathBuf,
        /// Why.
        message: String,
    },
}

/// The held lock; dropping it releases the lock and removes the pid file.
#[derive(Debug)]
pub struct ProcessLock {
    file: File,
    pid: PathBuf,
}

impl ProcessLock {
    /// Take the lock of the `role` (`relay` or `forger`) of the node whose data directory is
    /// `data`, and write its pid file.
    pub fn acquire(data: &Path, role: &'static str) -> Result<ProcessLock, LockError> {
        let run = data.join("run");
        let fail = |path: &Path, message: String| LockError::Io {
            path: path.to_path_buf(),
            message,
        };
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&run)
            .map_err(|e| fail(&run, e.to_string()))?;
        let lock = run.join(format!("{role}.lock"));
        let file = match take_lock(&lock) {
            Ok(Some(file)) => file,
            Ok(None) => return Err(LockError::Held { role, path: lock }),
            Err(error) => return Err(fail(&lock, error.to_string())),
        };
        // The lock is held, so a pid file that is there names no running process of the role.
        let pid = run.join(format!("{role}.pid"));
        let _ = fs::remove_file(&pid);
        let created = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&pid);
        let mut written = created.map_err(|e| fail(&pid, e.to_string()))?;
        let uid = written
            .metadata()
            .map_err(|e| fail(&pid, e.to_string()))?
            .uid();
        let directory = fs::metadata(&run).map_err(|e| fail(&run, e.to_string()))?;
        if directory.uid() != uid {
            let _ = fs::remove_file(&pid);
            return Err(fail(&run, "it belongs to another user".into()));
        }
        if directory.mode() & 0o077 != 0 {
            let private = Permissions::from_mode(0o700);
            fs::set_permissions(&run, private).map_err(|e| fail(&run, e.to_string()))?;
        }
        writeln!(written, "{}", std::process::id()).map_err(|e| fail(&pid, e.to_string()))?;
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
        // process, since liveness is the lock's.
        let _ = fs::remove_file(&self.pid);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use heartwood_cli::supervisor::signals::is_locked;

    fn scratch(name: &str) -> PathBuf {
        let data = std::env::temp_dir().join(format!("heartwood-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&data);
        data
    }

    #[test]
    fn a_second_lock_of_one_role_is_refused_until_the_first_is_dropped() {
        let data = scratch("lock");
        let first = ProcessLock::acquire(&data, "relay").unwrap();
        let pid = fs::read_to_string(data.join("run/relay.pid")).unwrap();
        assert_eq!(pid.trim(), std::process::id().to_string());
        let mode = fs::metadata(data.join("run")).unwrap().mode() & 0o777;
        assert_eq!(mode, 0o700);
        match ProcessLock::acquire(&data, "relay") {
            Err(LockError::Held {
                role: "relay",
                path,
            }) => {
                assert!(path.ends_with("run/relay.lock"), "{}", path.display())
            }
            other => panic!("{other:?}"),
        }
        // The other role has its own lock and pid file.
        let forger = ProcessLock::acquire(&data, "forger").unwrap();
        assert!(data.join("run/forger.pid").exists());
        drop(first);
        assert!(!data.join("run/relay.pid").exists());
        let again = ProcessLock::acquire(&data, "relay").unwrap();
        assert!(again.file().metadata().is_ok());
        drop((again, forger));
        fs::remove_dir_all(&data).unwrap();
    }

    #[test]
    fn a_left_pid_link_is_replaced_and_a_wide_run_directory_narrowed() {
        let data = scratch("lock-link");
        let run = data.join("run");
        fs::create_dir_all(&run).unwrap();
        fs::set_permissions(&run, Permissions::from_mode(0o755)).unwrap();
        let target = data.join("elsewhere");
        fs::write(&target, "kept").unwrap();
        std::os::unix::fs::symlink(&target, run.join("forger.pid")).unwrap();
        let lock = ProcessLock::acquire(&data, "forger").unwrap();
        assert_eq!(fs::read_to_string(&target).unwrap(), "kept");
        let pid = fs::symlink_metadata(run.join("forger.pid")).unwrap();
        assert!(pid.is_file());
        assert_eq!(fs::metadata(&run).unwrap().mode() & 0o777, 0o700);
        drop(lock);
        fs::remove_dir_all(&data).unwrap();
    }

    #[test]
    fn a_node_starting_while_the_commands_probe_its_lock_takes_it() {
        let data = scratch("lock-probe");
        fs::create_dir_all(data.join("run")).unwrap();
        let path = data.join("run/relay.lock");
        fs::write(&path, "").unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let probing = Arc::clone(&stop);
        let probe_path = path.clone();
        // `status` in a loop: a shared lock taken and released at once, over and over.
        let probe = std::thread::spawn(move || {
            let mut probes = 0_u32;
            while !probing.load(Ordering::Relaxed) {
                is_locked(&probe_path).unwrap();
                probes += 1;
            }
            probes
        });
        for _ in 0..50 {
            let lock = ProcessLock::acquire(&data, "relay").expect("a probe never refuses a start");
            drop(lock);
        }
        stop.store(true, Ordering::Relaxed);
        assert!(probe.join().unwrap() > 0);
        fs::remove_dir_all(&data).unwrap();
    }
}
