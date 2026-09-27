//! The capture of a node's standard output and error into `<name>-out.log` and
//! `<name>-error.log`, each rotated when it reaches 500 MB, keeping 7 rotated files.

use std::path::{Path, PathBuf};

use crate::errors::ProcessError;

/// The size at which a capture file is rotated.
pub const ROTATE_AT: u64 = 500 * 1000 * 1000;
/// How many rotated files are kept.
pub const KEEP: usize = 7;

/// A capture file that rotates itself.
#[derive(Debug)]
pub struct Capture {
    #[expect(dead_code, reason = "the capture's methods read it")]
    path: PathBuf,
    #[expect(dead_code, reason = "the capture's methods read it")]
    limit: u64,
}

impl Capture {
    /// A capture into `path`, rotated at `limit` bytes ([`ROTATE_AT`] in production).
    pub fn open(path: &Path, limit: u64) -> Result<Capture, ProcessError> {
        let _ = (path, limit);
        todo!("a capture file")
    }

    /// Append `bytes`, rotating first when they would pass the limit.
    pub fn write(&mut self, bytes: &[u8]) -> Result<(), ProcessError> {
        let _ = bytes;
        todo!("a capture's write")
    }
}
