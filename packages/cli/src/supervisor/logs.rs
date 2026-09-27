//! The capture of a node's standard output and error into `<name>-out.log` and
//! `<name>-error.log`, each rotated when it reaches 500 MB, keeping 7 rotated files: the
//! pm2-logrotate settings the reference's installer set. A rotated file is renamed `<file>.1`, and
//! the older ones move up to `<file>.7`; the oldest beyond that is dropped.

use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

use super::at;
use crate::errors::ProcessError;

/// The size at which a capture file is rotated.
pub const ROTATE_AT: u64 = 500 * 1000 * 1000;
/// How many rotated files are kept.
pub const KEEP: usize = 7;

/// A capture file that rotates itself.
#[derive(Debug)]
pub struct Capture {
    path: PathBuf,
    limit: u64,
    file: File,
    size: u64,
}

impl Capture {
    /// A capture into `path`, rotated at `limit` bytes ([`ROTATE_AT`] in production).
    pub fn open(path: &Path, limit: u64) -> Result<Capture, ProcessError> {
        if let Some(dir) = path.parent() {
            at(dir, fs::create_dir_all(dir))?;
        }
        let file = append(path)?;
        let size = at(path, file.metadata())?.len();
        let path = path.to_path_buf();
        Ok(Capture {
            path,
            limit,
            file,
            size,
        })
    }

    /// Append `bytes`, rotating first when they would pass the limit.
    pub fn write(&mut self, bytes: &[u8]) -> Result<(), ProcessError> {
        let len = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        if self.size > 0 && self.size.saturating_add(len) > self.limit {
            for k in (1..=KEEP).rev() {
                let from = match k {
                    1 => self.path.clone(),
                    _ => Capture::rotated(&self.path, k - 1),
                };
                match fs::rename(&from, Capture::rotated(&self.path, k)) {
                    Err(e) if e.kind() != ErrorKind::NotFound => return at(&from, Err(e)),
                    _ => {}
                }
            }
            self.file = append(&self.path)?;
            self.size = 0;
        }
        at(&self.path, self.file.write_all(bytes))?;
        self.size = self.size.saturating_add(len);
        Ok(())
    }

    /// The name of the `k`-th rotated file of the capture at `path`: `<path>.<k>`.
    pub fn rotated(path: &Path, k: usize) -> PathBuf {
        let mut name = path.as_os_str().to_owned();
        name.push(format!(".{k}"));
        PathBuf::from(name)
    }
}

fn append(path: &Path) -> Result<File, ProcessError> {
    at(
        path,
        OpenOptions::new().create(true).append(true).open(path),
    )
}
