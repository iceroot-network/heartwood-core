//! The process table of `status`: a running node's CPU use and resident memory, from `/proc` on
//! Linux (from `/bin/ps`, with an explicit argv, on macOS), and the table's text.

use crate::errors::ProcessError;
use crate::seams::StatusRow;

/// A running process's resource use.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sample {
    /// CPU use since the process started, in percent of one core.
    pub cpu_percent: f64,
    /// Resident memory, in bytes.
    pub rss_bytes: u64,
}

/// The resource use of `pid`.
pub fn sample(pid: u32) -> Result<Sample, ProcessError> {
    let _ = pid;
    todo!("a process's resource use")
}

/// The table of `rows`, with its header: ID, Name, Version, Status, Uptime, Restarts, CPU, RAM.
pub fn render(rows: &[StatusRow]) -> String {
    let _ = rows;
    todo!("the process table")
}
