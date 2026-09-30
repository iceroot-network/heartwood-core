//! What the scenario runner drives: a [`Backend`], one network of nodes. The `heartwood` backend
//! drives `heartwood devnet` and the relays' APIs; the `external` backend calls the executable that
//! `DIFFTEST_SCENARIO_BACKEND` names with one verb per call (the devnet tooling's, for reference
//! devnets and for the rootless networks); the `lockstep` backend runs the scenario through the
//! lockstep producer, where `revert` and `fork` exist too. Nodes are numbered from 1.

use std::path::Path;

use thiserror::Error;

use super::format::{CompiledTx, Part, RestartModes, Via};

/// A backend could not do what the runner asked.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum BackendError {
    /// The backend has no such action (a `revert` on a devnet, a `partition` on the lockstep).
    #[error("the {backend} backend cannot {action}")]
    Unsupported {
        /// The backend.
        backend: &'static str,
        /// The action.
        action: &'static str,
    },
    /// The network has no node of that number.
    #[error("the network has no node {0}")]
    NoNode(u32),
    /// The action failed.
    #[error("{action} failed: {message}")]
    Failed {
        /// The action.
        action: &'static str,
        /// Why.
        message: String,
    },
    /// The backend is not built yet.
    #[error("the {0} backend is not built yet")]
    NotBuilt(&'static str),
}

/// A node's tip as the runner observes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tip {
    /// Its height.
    pub height: u32,
    /// Its block's id, in hex.
    pub id: String,
}

/// A node's answer to one submitted transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Submitted {
    /// The transaction's name in the scenario.
    pub reference: String,
    /// Whether the node's pool admitted it.
    pub accepted: bool,
    /// The refusal's code (`ERR_OFFLINE`, ...), when it was refused.
    pub code: Option<String>,
}

/// One network the runner drives.
pub trait Backend {
    /// The backend's name: `heartwood`, `external` or `lockstep`.
    fn name(&self) -> &'static str;

    /// The number of nodes.
    fn nodes(&self) -> u32;

    /// Start node `node`, or its `part`.
    fn start(&mut self, node: u32, part: Part) -> Result<(), BackendError>;

    /// Stop node `node`, or its `part`; with `kill`, by SIGKILL.
    fn stop(&mut self, node: u32, part: Part, kill: bool) -> Result<(), BackendError>;

    /// Restart node `node`, or its `part`, by the restart `modes` give its implementation.
    fn restart(&mut self, node: u32, part: Part, modes: RestartModes) -> Result<(), BackendError>;

    /// Split the nodes into `groups`: a node reaches the nodes of its own group only.
    fn partition(&mut self, groups: &[Vec<u32>]) -> Result<(), BackendError>;

    /// Join every group again.
    fn heal(&mut self) -> Result<(), BackendError>;

    /// The base URL of node `node`'s public API, such as `http://10.85.0.11:6003`.
    fn api_url(&self, node: u32) -> Result<String, BackendError>;

    /// Archive every node's chain and logs into `dir`, as the run's record.
    fn archive(&mut self, dir: &Path) -> Result<(), BackendError>;

    /// Submit `transactions` to node `node` over `via`; each one's answer, in order.
    fn submit(
        &mut self,
        node: u32,
        via: Via,
        transactions: &[CompiledTx],
    ) -> Result<Vec<Submitted>, BackendError>;

    /// Node `node`'s tip now.
    fn tip(&self, node: u32) -> Result<Tip, BackendError>;

    /// Take back `blocks` blocks of the chain (the lockstep producer only).
    fn revert(&mut self, blocks: u32) -> Result<(), BackendError> {
        let _ = blocks;
        Err(BackendError::Unsupported {
            backend: self.name(),
            action: "revert",
        })
    }

    /// Switch to the branch `branch` (the lockstep producer only).
    fn fork(&mut self, branch: &str) -> Result<(), BackendError> {
        let _ = branch;
        Err(BackendError::Unsupported {
            backend: self.name(),
            action: "fork",
        })
    }
}
