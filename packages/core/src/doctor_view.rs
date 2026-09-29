//! [`CoreNodeView`]: the doctor's view of the node through the node's own code: the relay's and
//! the forger's configuration as their loaders compute it, the version floor with the P2P crate's
//! version gate, and the relay's store read through a read-only connection while the relay runs.
//! Only typed facts and variable names cross the seam, never a `.env` value, and nothing here
//! reads the key file.

use std::path::PathBuf;

use heartwood_cli::Instance;
use heartwood_cli::doctor::seams::{
    BuildFacts, FloorFacts, ForgerFacts, NodeView, NodeViewError, RelayFacts, StoreWindow,
};

/// The node's view for the doctor, with the environment, home directory and working directory the
/// node's loaders read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CoreNodeView {
    /// The process's variables, as a role's start reads them.
    pub process: Vec<(String, String)>,
    /// The home directory.
    pub home: PathBuf,
    /// The working directory.
    pub cwd: PathBuf,
}

impl CoreNodeView {
    /// The view with this process's environment, home and working directory.
    pub fn from_process() -> CoreNodeView {
        CoreNodeView {
            process: std::env::vars().collect(),
            home: std::env::home_dir().unwrap_or_default(),
            cwd: std::env::current_dir().unwrap_or_default(),
        }
    }
}

impl NodeView for CoreNodeView {
    fn build(&self) -> BuildFacts {
        let features = [
            ("devnet", cfg!(feature = "devnet")),
            ("difftest", cfg!(feature = "difftest")),
        ];
        BuildFacts {
            version: crate::VERSION,
            features: features
                .into_iter()
                .filter(|(_, on)| *on)
                .map(|(name, _)| name)
                .collect(),
        }
    }

    fn relay(&self, relay: &Instance) -> Result<RelayFacts, NodeViewError> {
        let _ = relay;
        Err(NodeViewError::NotBuilt)
    }

    fn forger(&self, forger: &Instance) -> Result<ForgerFacts, NodeViewError> {
        let _ = forger;
        Err(NodeViewError::NotBuilt)
    }

    fn floor(
        &self,
        relay: &Instance,
        version: &str,
        height: u32,
    ) -> Result<FloorFacts, NodeViewError> {
        let _ = (relay, version, height);
        Err(NodeViewError::NotBuilt)
    }

    fn store_window(
        &self,
        relay: &Instance,
        usernames: &[String],
        from_height: u32,
    ) -> Result<StoreWindow, NodeViewError> {
        let _ = (relay, usernames, from_height);
        Err(NodeViewError::NotBuilt)
    }
}
