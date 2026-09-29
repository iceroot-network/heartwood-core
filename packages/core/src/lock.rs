//! The process lock of a role: `<data>/run/<role>.lock` (`relay.lock`, `forger.lock`), held for
//! the process's lifetime, so a second process of the role of one instance refuses to start; and
//! `<data>/run/<role>.pid`, the process's id, removed when the lock is released. The run
//! directory and the lock are made and taken as the commands make and take theirs
//! ([`heartwood_cli::supervisor::run_dir`], mode 0700 and refused when another user owns it;
//! [`take_lock`], which tries again past the short shared locks of the commands' probes, so a node
//! started while `status` looks at its lock does not refuse). The pid file is always a new private
//! file (a link left in its place is removed, never followed), made and removed in the run
//! directory as it was checked and is held ([`RunDir`]), so a directory swapped for a link
//! meanwhile receives none of the files and loses none.

use std::fs::File;
use std::io::Write;
use std::path::PathBuf;

use heartwood_cli::supervisor::RunDir;
use heartwood_cli::supervisor::signals::take_lock_in;
use heartwood_cli::{Instance, Role};
use heartwood_kernel::paths::Paths;
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
    #[error("{0}")]
    Io(String),
}

/// The held lock; dropping it releases the lock and removes the pid file.
#[derive(Debug)]
pub struct ProcessLock {
    file: File,
    dir: RunDir,
    pid: PathBuf,
}

impl ProcessLock {
    /// Take the lock of `instance`'s role, and write its pid file.
    pub fn acquire(instance: &Instance) -> Result<ProcessLock, LockError> {
        let io = |error: &dyn std::fmt::Display| LockError::Io(error.to_string());
        let dir = RunDir::open(instance).map_err(|e| io(&e))?;
        let (path, role) = (instance.lock_path(), instance.role.name());
        let file = take_lock_in(&dir, &path).map_err(|e| io(&e))?;
        let file = file.ok_or(LockError::Held { role, path })?;
        // The lock is held, so a pid file that is there names no running process of the role.
        let pid = instance.pid_path();
        let _ = dir.remove(&pid);
        let failed = |error: std::io::Error| LockError::Io(format!("{}: {error}", pid.display()));
        let mut created = dir.create(&pid, 0o600).map_err(failed)?;
        writeln!(created, "{}", std::process::id()).map_err(failed)?;
        Ok(ProcessLock { file, dir, pid })
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
        let _ = self.dir.remove(&self.pid);
    }
}

/// The instance of `role` of `token` on `network`, whose directories are `paths`.
pub fn instance(role: Role, token: &str, network: &str, paths: &Paths) -> Instance {
    let (token, network, paths) = (token.to_owned(), network.to_owned(), paths.clone());
    Instance {
        token,
        network,
        role,
        paths,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    use heartwood_cli::supervisor::signals::is_locked;

    fn scratch(name: &str) -> PathBuf {
        let data = std::env::temp_dir().join(format!("heartwood-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&data);
        data
    }

    impl ProcessLock {
        fn test(data: &Path, role: &str) -> Result<ProcessLock, LockError> {
            let role = if role == "relay" {
                Role::Relay
            } else {
                Role::Forger
            };
            let dir = data.to_path_buf();
            let paths = Paths {
                config: dir.clone(),
                data: dir.clone(),
                cache: dir.clone(),
                log: dir.clone(),
                temp: dir,
            };
            ProcessLock::acquire(&instance(role, "t", "n", &paths))
        }
    }

    #[test]
    fn a_second_lock_of_one_role_is_refused_until_the_first_is_dropped() {
        let data = scratch("lock");
        let first = ProcessLock::test(&data, "relay").unwrap();
        let pid = fs::read_to_string(data.join("run/relay.pid")).unwrap();
        assert_eq!(pid.trim(), std::process::id().to_string());
        let mode = fs::metadata(data.join("run")).unwrap().mode() & 0o777;
        assert_eq!(mode, 0o700);
        match ProcessLock::test(&data, "relay") {
            Err(LockError::Held {
                role: "relay",
                path,
            }) => {
                assert!(path.ends_with("run/relay.lock"), "{}", path.display())
            }
            other => panic!("{other:?}"),
        }
        // The other role has its own lock and pid file.
        let forger = ProcessLock::test(&data, "forger").unwrap();
        assert!(data.join("run/forger.pid").exists());
        drop(first);
        assert!(!data.join("run/relay.pid").exists());
        let again = ProcessLock::test(&data, "relay").unwrap();
        assert!(again.file().metadata().is_ok());
        drop((again, forger));
        fs::remove_dir_all(&data).unwrap();
    }

    #[test]
    fn a_left_pid_link_is_replaced_and_a_wide_run_directory_narrowed() {
        let data = scratch("lock-link");
        let run = data.join("run");
        fs::create_dir_all(&run).unwrap();
        fs::set_permissions(&run, fs::Permissions::from_mode(0o755)).unwrap();
        let target = data.join("elsewhere");
        fs::write(&target, "kept").unwrap();
        std::os::unix::fs::symlink(&target, run.join("forger.pid")).unwrap();
        let lock = ProcessLock::test(&data, "forger").unwrap();
        assert_eq!(fs::read_to_string(&target).unwrap(), "kept");
        let pid = fs::symlink_metadata(run.join("forger.pid")).unwrap();
        assert!(pid.is_file());
        assert_eq!(fs::metadata(&run).unwrap().mode() & 0o777, 0o700);
        drop(lock);
        fs::remove_dir_all(&data).unwrap();
    }

    #[test]
    fn a_relay_lock_link_is_refused() {
        let data = scratch("lock-file-link");
        let run = data.join("run");
        fs::create_dir_all(&run).unwrap();
        let target = data.join("elsewhere");
        fs::write(&target, "kept").unwrap();
        std::os::unix::fs::symlink(&target, run.join("relay.lock")).unwrap();
        let refused = ProcessLock::test(&data, "relay");
        assert!(matches!(refused, Err(LockError::Io(_))), "{refused:?}");
        assert_eq!(fs::read_to_string(&target).unwrap(), "kept");
        assert!(!run.join("relay.pid").exists());
        fs::remove_dir_all(&data).unwrap();
    }

    #[test]
    fn the_pid_file_is_removed_from_the_directory_that_was_checked() {
        let data = scratch("lock-swap");
        let lock = ProcessLock::test(&data, "relay").unwrap();
        let (run, moved, other) = (data.join("run"), data.join("moved"), data.join("other"));
        fs::create_dir(&other).unwrap();
        fs::write(other.join("relay.pid"), "kept").unwrap();
        // The run directory is swapped for a link to another directory while the lock is held.
        fs::rename(&run, &moved).unwrap();
        std::os::unix::fs::symlink(&other, &run).unwrap();
        drop(lock);
        assert!(!moved.join("relay.pid").exists());
        assert_eq!(fs::read_to_string(other.join("relay.pid")).unwrap(), "kept");
        fs::remove_dir_all(&data).unwrap();
    }

    #[test]
    fn a_node_starting_while_the_commands_probe_its_lock_takes_it() {
        let data = scratch("lock-probe");
        fs::create_dir_all(data.join("run")).unwrap();
        let path = data.join("run/relay.lock");
        fs::write(&path, "").unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let probes = Arc::new(AtomicU32::new(0));
        let (probing, counted, probe_path) = (Arc::clone(&stop), Arc::clone(&probes), path.clone());
        // `status` in a loop: a shared lock taken and released at once, over and over.
        let probe = std::thread::spawn(move || {
            while !probing.load(Ordering::Relaxed) {
                is_locked(&probe_path).unwrap();
                counted.fetch_add(1, Ordering::Relaxed);
            }
        });
        // The starts overlap the probes whatever the number of cores: they begin once the probe
        // runs, and go on until it has probed 50 times among them (the probe's thread may not
        // run at all during 50 quick starts on a small machine).
        while probes.load(Ordering::Relaxed) == 0 && !probe.is_finished() {
            std::thread::yield_now();
        }
        let before = probes.load(Ordering::Relaxed);
        let probed = || probes.load(Ordering::Relaxed) - before;
        let mut starts = 0;
        while starts < 50 || (probed() < 50 && !probe.is_finished()) {
            let lock = ProcessLock::test(&data, "relay").expect("a probe never refuses a start");
            drop(lock);
            starts += 1;
        }
        stop.store(true, Ordering::Relaxed);
        probe.join().unwrap();
        assert!(probed() >= 50);
        fs::remove_dir_all(&data).unwrap();
    }
}
