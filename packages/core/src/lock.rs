//! The process lock: `<data>/run/relay.lock`, held for the process's lifetime, so a second relay of
//! the instance refuses to start; and `<data>/run/relay.pid`.

use std::path::Path;

use crate::app::RelayError;

/// The held lock; dropping it releases the lock and removes the pid file.
#[derive(Debug)]
pub struct ProcessLock {
    file: std::fs::File,
}

impl ProcessLock {
    /// Take the lock of the node whose data directory is `data`.
    pub fn acquire(data: &Path) -> Result<ProcessLock, RelayError> {
        let _ = data;
        todo!()
    }

    /// The lock file, held open.
    pub fn file(&self) -> &std::fs::File {
        &self.file
    }
}
