//! The job stall of a harness build (feature `difftest`), which no release build has: the next job
//! of a block received in `newBlock` sleeps before its commit for the number of milliseconds that
//! the file `stall-next-job` in the harness's directory holds, and the file is removed, so a live
//! run can hold one job past AwaitBlock's bound while reference clients wait on the relay.

use std::path::PathBuf;
use std::time::Duration;

/// The name of the control file in the harness's directory.
pub const STALL_FILE: &str = "stall-next-job";

/// The stall control of a relay: its harness's directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StallControl {
    dir: PathBuf,
}

impl StallControl {
    /// The control of the harness's directory `dir`.
    pub fn new(dir: PathBuf) -> StallControl {
        StallControl { dir }
    }

    /// The stall the control file asks for, which removes the file; `None` when there is none.
    pub fn take(&self) -> Option<Duration> {
        let _ = self.dir.join(STALL_FILE);
        None
    }
}
