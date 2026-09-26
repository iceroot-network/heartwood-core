//! Error type of the crate.

use std::path::PathBuf;

use thiserror::Error;

/// The logger could not be set up or could not write.
#[derive(Debug, Error)]
pub enum LoggerError {
    /// A log file or directory could not be opened, written, rotated or removed.
    #[error("{path}: {source}")]
    Io {
        /// The file or directory.
        path: PathBuf,
        /// The error.
        #[source]
        source: std::io::Error,
    },
    /// A subscriber is already installed in this process.
    #[error("a logger is already installed in this process")]
    AlreadyInstalled,
}
