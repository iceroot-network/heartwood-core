//! Installing the logger: the console and file outputs and their levels.

use std::path::Path;

use heartwood_kernel::log::Level;

use crate::errors::LoggerError;

/// Which output shows which levels, and how many rotated files are kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoggerOptions {
    /// The least severe level the console shows; `None` turns the console output off.
    pub console: Option<Level>,
    /// The least severe level the file shows; `None` turns the file output off.
    pub file: Option<Level>,
    /// The most rotated files kept.
    pub max_files: usize,
    /// The most bytes the rotated files may take together.
    pub max_bytes: u64,
    /// Whether emoji codes such as `:dove_of_peace:` become emoji; when off they are removed.
    pub emojify: bool,
}

impl Default for LoggerOptions {
    /// The reference's defaults: `info` on the console, `debug` in the file, 10 rotated files of
    /// at most 100 MiB together, emoji on.
    fn default() -> LoggerOptions {
        LoggerOptions {
            console: Some(Level::Info),
            file: Some(Level::Debug),
            max_files: 10,
            max_bytes: 100 * 1024 * 1024,
            emojify: true,
        }
    }
}

/// Keeps the outputs running; dropping it flushes and closes the log file.
#[derive(Debug)]
pub struct LoggerGuard {
    _private: (),
}

/// Install the logger for this process, with its files in `log_dir` named after `file_stem`
/// (the reference's `<token>-<network>-<process>`).
pub fn init(
    options: &LoggerOptions,
    log_dir: &Path,
    file_stem: &str,
) -> Result<LoggerGuard, LoggerError> {
    let _ = (options, log_dir, file_stem);
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_are_the_reference_defaults() {
        let options = LoggerOptions::default();
        assert_eq!(options.console, Some(Level::Info));
        assert_eq!(options.file, Some(Level::Debug));
        assert_eq!((options.max_files, options.max_bytes), (10, 104_857_600));
    }
}
