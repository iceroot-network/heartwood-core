//! `peers.json`: the seed peers and the peer sources, with the reference's schema (unknown keys
//! refused). A source is a local file (a path starting with `/`) or a plain-HTTP URL; an `https`
//! source is refused at start, naming it.

use std::path::{Path, PathBuf};

use thiserror::Error;

/// One seed peer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerData {
    /// The address, as the file gives it.
    pub ip: String,
    /// The P2P port.
    pub port: u16,
}

/// `peers.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PeersFile {
    /// The seed peers, in file order.
    pub list: Vec<PeerData>,
    /// The peer sources, in file order.
    pub sources: Vec<String>,
}

/// `peers.json` could not be read.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PeersError {
    /// The file could not be read.
    #[error("cannot read {}: {detail}", path.display())]
    Read {
        /// The file.
        path: PathBuf,
        /// Why.
        detail: String,
    },
    /// The file is not JSON, or its schema refuses it.
    #[error("peers.json is not valid: {detail}")]
    Schema {
        /// What is wrong.
        detail: String,
    },
    /// A source that the relay does not fetch.
    #[error("the peer source {source_name} is not a local file or a plain-HTTP URL")]
    Source {
        /// The source, as the file names it.
        source_name: String,
    },
}

/// `<config_dir>/peers.json`.
pub fn load(config_dir: &Path) -> Result<PeersFile, PeersError> {
    let _ = config_dir;
    todo!()
}
