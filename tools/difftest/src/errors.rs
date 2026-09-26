//! Error type of the harness.

use std::path::PathBuf;

use thiserror::Error;

/// Everything that can stop a harness command. Differences between the oracle and Heartwood are
/// not errors: they are reported as results (see [`crate::report`]).
#[derive(Debug, Error)]
pub enum Error {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("{path}:{line}: {message}")]
    Parse {
        path: PathBuf,
        line: usize,
        message: String,
    },

    #[error("{path}: {message}")]
    Vectors { path: PathBuf, message: String },

    #[error("manifest check failed in {}:\n  {}", dir.display(), problems.join("\n  "))]
    Manifest { dir: PathBuf, problems: Vec<String> },

    #[error("oracle: {0}")]
    Oracle(String),

    #[error("{0}")]
    Usage(String),

    /// A level or feature that is not built yet.
    #[error("{0}")]
    NotYet(String),

    #[error("two subjects registered for operation {0}")]
    DuplicateSubject(String),
}

impl Error {
    pub(crate) fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Error::Io {
            path: path.into(),
            source,
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;
