//! The crypto files of a network: `network.json`, `milestones.json`, `genesisBlock.json` and
//! `exceptions.json`, from a network's `crypto` directory.
//!
//! All four must be there: a missing file refuses the start and is named, where the reference
//! falls back to its built-in preset; there are no built-in networks. The network and milestone
//! files pass the strict schema ([`crate::config::schema`]) before the crypto crate's loaders
//! read them; `exceptions.json` must be exactly `{}`; the genesis block's JSON is checked by the
//! genesis path that builds the first state.

use std::fs;
use std::io::ErrorKind;
use std::path::Path;

use heartwood_crypto::errors::ConfigError;
use heartwood_crypto::{Milestones, Network};
use serde_json::Value;
use thiserror::Error;

use crate::config::schema::{self, SchemaError};

/// The file names, in the order the reference's loader reads them.
#[rustfmt::skip]
pub const FILES: [&str; 4] =
    ["network.json", "milestones.json", "genesisBlock.json", "exceptions.json"];

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

/// Read and check the four files in `dir` (see [`parse`]).
pub fn load(dir: &Path) -> Result<CryptoFiles, CryptoFilesError> {
    let read = |file: &'static str| match fs::read_to_string(dir.join(file)) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(CryptoFilesError::Read {
            file,
            detail: error.to_string(),
        }),
    };
    let [network, milestones, genesis, exceptions] = FILES.map(read);
    parse(&CryptoTexts {
        network: network?,
        milestones: milestones?,
        genesis: genesis?,
        exceptions: exceptions?,
    })
}

/// The JSON of `file`, whose text is `text`.
fn json(file: &'static str, text: &str) -> Result<Value, CryptoFilesError> {
    serde_json::from_str(text).map_err(|error| CryptoFilesError::Json {
        file,
        detail: error.to_string(),
    })
}

/// Check the four files' `texts` and load them: every file present, each one JSON, the strict
/// schema of the network, milestone and exceptions files, then the crypto crate's loaders of the
/// network and milestone files. The first refusal, in [`FILES`] order within each step, is
/// returned.
pub fn parse(texts: &CryptoTexts) -> Result<CryptoFiles, CryptoFilesError> {
    let all = [
        &texts.network,
        &texts.milestones,
        &texts.genesis,
        &texts.exceptions,
    ];
    let missing: Vec<&'static str> = (FILES.into_iter().zip(all))
        .filter_map(|(file, text)| text.is_none().then_some(file))
        .collect();
    let [
        Some(network),
        Some(milestones),
        Some(genesis),
        Some(exceptions),
    ] = all
    else {
        return Err(CryptoFilesError::Missing { files: missing });
    };
    let [network_file, milestones_file, genesis_file, exceptions_file] = FILES;
    let network_json = json(network_file, network)?;
    let milestones_json = json(milestones_file, milestones)?;
    let genesis = json(genesis_file, genesis)?;
    schema::check_network(&network_json)?;
    schema::check_milestones(&milestones_json)?;
    schema::check_exceptions(&json(exceptions_file, exceptions)?)?;
    let config = |file: &'static str| move |source| CryptoFilesError::Config { file, source };
    let network = Network::load(network).map_err(config(network_file))?;
    Ok(CryptoFiles {
        milestones: Milestones::load(milestones, &network).map_err(config(milestones_file))?,
        milestones_text: milestones.clone(),
        network,
        genesis,
    })
}
