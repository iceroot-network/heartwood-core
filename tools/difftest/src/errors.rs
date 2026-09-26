//! Error type of the harness.

use std::path::PathBuf;

use thiserror::Error;

/// Everything that can stop a harness command. Differences between the oracle and Heartwood are
/// not errors: they are reported as results (see [`crate::report`]).
#[derive(Debug, Error)]
pub enum Error {
    /// A file or directory could not be read or written.
    #[error("{path}: {source}")]
    Io {
        /// The path.
        path: PathBuf,
        /// The error.
        #[source]
        source: std::io::Error,
    },

    /// A line of a file is not valid JSON.
    #[error("{path}:{line}: {message}")]
    Parse {
        /// The file.
        path: PathBuf,
        /// The line, from 1.
        line: usize,
        /// What is wrong.
        message: String,
    },

    /// A vector file or directory breaks the vector format.
    #[error("{path}: {message}")]
    Vectors {
        /// The file or directory.
        path: PathBuf,
        /// What is wrong.
        message: String,
    },

    /// The files of a vector directory do not match its manifest.
    #[error("manifest check failed in {}:\n  {}", dir.display(), problems.join("\n  "))]
    Manifest {
        /// The vector directory.
        dir: PathBuf,
        /// One line per problem.
        problems: Vec<String>,
    },

    /// A devnet archive or a corpus directory breaks its format, or does not link up.
    #[error("{path}: {message}")]
    Corpus {
        /// The file or directory.
        path: PathBuf,
        /// What is wrong.
        message: String,
    },

    /// The oracle could not be started or answered wrongly.
    #[error("oracle: {0}")]
    Oracle(String),

    /// The command line or a setting is wrong.
    #[error("{0}")]
    Usage(String),

    /// A level or feature that is not built yet.
    #[error("{0}")]
    NotYet(String),

    /// Two subjects were registered for the same operation.
    #[error("two subjects registered for operation {0}")]
    DuplicateSubject(String),

    /// A genesis profile that cannot be read or does not describe a chain definition.
    #[error("{path}: {message}")]
    Profile {
        /// The profile file.
        path: PathBuf,
        /// What is wrong.
        message: String,
    },

    /// The chain definition could not be built.
    #[error("genesis: {0}")]
    Genesis(#[from] heartwood_crypto::errors::GenesisError),
}

impl Error {
    pub(crate) fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Error::Io {
            path: path.into(),
            source,
        }
    }
}

/// A result with the harness's [`enum@Error`].
pub type Result<T> = std::result::Result<T, Error>;
