//! The log files and their rotation.
//!
//! As the reference's rotating file stream is configured: the live file is `<stem>-current.log`,
//! opened for appending. At each midnight after the file was opened (UTC here; the reference uses
//! the machine's time zone, UTC on a server), the next write first rotates it: the file is
//! compressed with gzip into `<stem>-<YYYY-MM-DD>.log.gz`, named after the date of the rotation,
//! or `<stem>-<YYYY-MM-DD>.<n>.log.gz` with the first free `n` from 2, and a new live file is
//! started. A start never rotates, and nothing rotates on size.
//!
//! The rotated files are listed, one path per line, in the history file
//! `<stem>-current.log.txt`. After each rotation the files of the list that still exist are
//! ordered by their status change time, and the oldest are removed until at most `max_files`
//! remain and they take at most `max_bytes` together (a limit of 0 is no limit).

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use flate2::Compression;
use flate2::write::GzEncoder;
use heartwood_kernel::clock::Clock;
use heartwood_kernel::timestamp::UtcDateTime;

use crate::errors::LoggerError;

/// The most rotated files of one date.
const MAX_INDEX: u32 = 999;
const DAY_MS: i64 = 86_400_000;

/// The name of the live file.
pub fn current_name(stem: &str) -> String {
    format!("{stem}-current.log")
}

/// The name of the history file, which lists the rotated files.
pub fn history_name(stem: &str) -> String {
    format!("{}.txt", current_name(stem))
}

/// The name of the `index`-th rotated file of `date` (`YYYY-MM-DD`); the first has no index.
pub fn rotated_name(stem: &str, date: &str, index: u32) -> String {
    if index > 1 {
        format!("{stem}-{date}.{index}.log.gz")
    } else {
        format!("{stem}-{date}.log.gz")
    }
}

/// Add `rotated` to the history of `stem` in `dir`, then remove the oldest listed files until at
/// most `max_files` remain and they take at most `max_bytes` together. Returns the removed files.
pub fn prune(
    dir: &Path,
    stem: &str,
    rotated: &Path,
    max_files: usize,
    max_bytes: u64,
) -> Result<Vec<PathBuf>, LoggerError> {
    let history = dir.join(history_name(stem));
    let listed = match fs::read_to_string(&history) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
        Err(source) => return Err(io_error(&history, source)),
    };
    let mut files = Vec::new();
    for name in listed
        .split('\n')
        .map(PathBuf::from)
        .chain([rotated.to_path_buf()])
    {
        match fs::metadata(&name) {
            Ok(meta) if meta.is_file() => files.push((changed_ms(&meta), meta.len(), name)),
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(source) => return Err(io_error(&name, source)),
        }
    }
    // A stable sort, so files changed in the same millisecond keep their history order.
    files.sort_by_key(|(changed, _, _)| *changed);
    let mut removed = Vec::new();
    let mut total: u64 = files.iter().map(|(_, size, _)| *size).sum();
    while !files.is_empty()
        && ((max_files > 0 && files.len() > max_files) || (max_bytes > 0 && total > max_bytes))
    {
        let (_, size, name) = files.remove(0);
        fs::remove_file(&name).map_err(|source| io_error(&name, source))?;
        total = total.saturating_sub(size);
        removed.push(name);
    }
    let mut text: String = files
        .iter()
        .map(|(_, _, name)| name.to_string_lossy())
        .collect::<Vec<_>>()
        .join("\n");
    text.push('\n');
    fs::write(&history, text).map_err(|source| io_error(&history, source))?;
    Ok(removed)
}

/// The live log file, rotated daily.
pub struct RotatingFile {
    dir: PathBuf,
    stem: String,
    max_files: usize,
    max_bytes: u64,
    clock: Arc<dyn Clock>,
    file: File,
    next_rotation_ms: i64,
}

impl RotatingFile {
    /// Open (or create) the live file of `stem` in `dir`, creating `dir` if needed.
    pub fn open(
        dir: &Path,
        stem: &str,
        max_files: usize,
        max_bytes: u64,
        clock: Arc<dyn Clock>,
    ) -> Result<RotatingFile, LoggerError> {
        fs::create_dir_all(dir).map_err(|source| io_error(dir, source))?;
        let file = append(&dir.join(current_name(stem)))?;
        let next_rotation_ms = next_midnight(clock.now_ms());
        Ok(RotatingFile {
            dir: dir.to_path_buf(),
            stem: stem.to_owned(),
            max_files,
            max_bytes,
            clock,
            file,
            next_rotation_ms,
        })
    }

    /// The path of the live file.
    pub fn path(&self) -> PathBuf {
        self.dir.join(current_name(&self.stem))
    }

    /// Rotate now: compress the live file into the next rotated name of today's date, start a new
    /// live file and prune the history. Returns the rotated file.
    pub fn rotate(&mut self) -> Result<PathBuf, LoggerError> {
        let now = self.clock.now_ms();
        let current = self.path();
        self.file
            .flush()
            .map_err(|source| io_error(&current, source))?;
        let date = UtcDateTime::from_unix_ms(now)
            .map(|time| format!("{:04}-{:02}-{:02}", time.year, time.month, time.day))
            .unwrap_or_default();
        let rotated = (1..=MAX_INDEX)
            .map(|index| self.dir.join(rotated_name(&self.stem, &date, index)))
            .find(|path| !path.exists())
            .ok_or_else(|| {
                io_error(
                    &self.dir,
                    io::Error::other("no free name for a rotated file"),
                )
            })?;
        gzip(&current, &rotated)?;
        fs::remove_file(&current).map_err(|source| io_error(&current, source))?;
        prune(
            &self.dir,
            &self.stem,
            &rotated,
            self.max_files,
            self.max_bytes,
        )?;
        self.file = append(&current)?;
        self.next_rotation_ms = next_midnight(now);
        Ok(rotated)
    }
}

impl Write for RotatingFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.clock.now_ms() >= self.next_rotation_ms {
            self.rotate().map_err(io::Error::other)?;
        } else if !self.path().exists() {
            // The live file was removed under us: start it again, as the reference does.
            self.file = append(&self.path()).map_err(io::Error::other)?;
        }
        self.file.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

/// The first UTC midnight after `now_ms`.
fn next_midnight(now_ms: i64) -> i64 {
    now_ms
        .div_euclid(DAY_MS)
        .saturating_add(1)
        .saturating_mul(DAY_MS)
}

fn append(path: &Path) -> Result<File, LoggerError> {
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|source| io_error(path, source))
}

fn gzip(from: &Path, to: &Path) -> Result<(), LoggerError> {
    let mut input = File::open(from).map_err(|source| io_error(from, source))?;
    let output = File::create(to).map_err(|source| io_error(to, source))?;
    let mut encoder = GzEncoder::new(output, Compression::default());
    io::copy(&mut input, &mut encoder).map_err(|source| io_error(to, source))?;
    encoder
        .finish()
        .and_then(|mut file| file.flush())
        .map_err(|source| io_error(to, source))
}

/// The status change time of a file in milliseconds, as the reference orders rotated files.
fn changed_ms(meta: &fs::Metadata) -> i64 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        meta.ctime()
            .saturating_mul(1_000)
            .saturating_add(meta.ctime_nsec().div_euclid(1_000_000))
    }
    #[cfg(not(unix))]
    {
        // No status change time: every file ties, and the history order decides.
        let _ = meta;
        0
    }
}

fn io_error(path: &Path, source: io::Error) -> LoggerError {
    LoggerError::Io {
        path: path.to_path_buf(),
        source,
    }
}
