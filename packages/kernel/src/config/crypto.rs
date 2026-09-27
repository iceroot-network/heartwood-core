//! The crypto files of a network: `network.json`, `milestones.json`, `genesisBlock.json` and
//! `exceptions.json`, from a network's `crypto` directory.
//!
//! All four must be there: a missing file refuses the start and is named, where the reference
//! falls back to its built-in preset; there are no built-in networks. The network and milestone
//! files pass the strict schema ([`crate::config::schema`]) before the crypto crate's loaders
//! read them; `exceptions.json` must be exactly `{}`; the genesis block's JSON is checked by the
//! genesis path that builds the first state.

use std::path::Path;

use heartwood_crypto::errors::ConfigError;
use heartwood_crypto::{Milestones, Network};
use serde_json::Value;
use thiserror::Error;

use crate::config::schema::SchemaError;

/// The file names, in the order the reference's loader reads them.
pub const FILES: [&str; 4] = [
    "network.json",
    "milestones.json",
    "genesisBlock.json",
    "exceptions.json",
];

/// The texts of the four files, each `None` when its file is missing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CryptoTexts {
    /// `network.json`.
    pub network: Option<String>,
    /// `milestones.json`.
    pub milestones: Option<String>,
    /// `genesisBlock.json`.
    pub genesis: Option<String>,
    /// `exceptions.json`.
    pub exceptions: Option<String>,
}

/// A network's crypto configuration, loaded and checked.
#[derive(Debug, Clone, PartialEq)]
pub struct CryptoFiles {
    /// The network description.
    pub network: Network,
    /// The milestones, merged and checked.
    pub milestones: Milestones,
    /// The text of `milestones.json`, whose entries the store's applied-entry digests cover.
    pub milestones_text: String,
    /// The genesis block's JSON.
    pub genesis: Value,
}

/// The crypto files were refused; the node does not start. Every refusal names the file.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CryptoFilesError {
    /// One or more of the four files is missing.
    #[error("the crypto configuration lacks {}; all four files are required", files.join(", "))]
    Missing {
        /// The missing files, in [`FILES`] order.
        files: Vec<&'static str>,
    },
    /// A file could not be read.
    #[error("cannot read {file}: {detail}")]
    Read {
        /// The file.
        file: &'static str,
        /// Why.
        detail: String,
    },
    /// A file is not JSON.
    #[error("{file} is not JSON: {detail}")]
    Json {
        /// The file.
        file: &'static str,
        /// Why.
        detail: String,
    },
    /// A file breaks the strict schema.
    #[error(transparent)]
    Schema(#[from] SchemaError),
    /// The crypto crate's loader refused the network or milestone file.
    #[error("{file}: {source}")]
    Config {
        /// The file.
        file: &'static str,
        /// The loader's refusal.
        source: ConfigError,
    },
}

/// Read and check the four files in `dir`.
pub fn load(dir: &Path) -> Result<CryptoFiles, CryptoFilesError> {
    let _ = dir;
    todo!("read the four files")
}

/// Check the four files' `texts` and load them: every file present, the strict schema,
/// `exceptions.json` exactly `{}`, then the crypto crate's loaders.
pub fn parse(texts: &CryptoTexts) -> Result<CryptoFiles, CryptoFilesError> {
    let _ = texts;
    todo!("check and load the four files")
}
