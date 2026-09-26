//! The log files and their rotation.
//!
//! The live file is `<stem>-current.log`. At the first write of a new UTC day, and when the file
//! reaches its size limit, it is compressed into `<stem>-<YYYY-MM-DD>.log.gz`, or
//! `<stem>-<YYYY-MM-DD>.<n>.log.gz` for the n-th file of a day (n from 2). Then the oldest
//! rotated files are removed until at most `max_files` remain and they take at most `max_bytes`.

use std::path::Path;

use crate::errors::LoggerError;

/// The name of the live file.
pub fn current_name(stem: &str) -> String {
    let _ = stem;
    todo!()
}

/// The name of the `index`-th rotated file of `date` (`YYYY-MM-DD`); the first has no index.
pub fn rotated_name(stem: &str, date: &str, index: u32) -> String {
    let _ = (stem, date, index);
    todo!()
}

/// Remove the oldest rotated files of `stem` in `dir` until at most `max_files` remain and they
/// take at most `max_bytes`.
pub fn prune(dir: &Path, stem: &str, max_files: usize, max_bytes: u64) -> Result<(), LoggerError> {
    let _ = (dir, stem, max_files, max_bytes);
    todo!()
}
