//! The version floor: the node's own version against the `p2p.minimumVersions` of the milestones.
//!
//! The floor in force at the stored tip (height 1 at a first start) must admit the node's version,
//! or the start stops and names the height. A later milestone whose floor would refuse the node is
//! a warning, with its height: the node is cut off there, as the network intends.

use heartwood_crypto::{Milestones, Params};
use thiserror::Error;

/// A later milestone whose floor refuses the node's version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FloorWarning {
    /// The milestone's height.
    pub height: u32,
    /// Its minimum versions.
    pub ranges: Vec<String>,
}

/// The floor in force refuses the node's version.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error(
    "this node's version {version} is below the minimum versions {} in force at height {height}",
    ranges.join(", ")
)]
pub struct FloorError {
    /// The height whose milestone refuses the node.
    pub height: u32,
    /// The node's version.
    pub version: String,
    /// The milestone's minimum versions.
    pub ranges: Vec<String>,
}

/// Check the node's version `own_version` on the network named `network_name` against the floor
/// in force at `height` and the later ones; `gate` is the peers' version gate (whether a version
/// passes a milestone's minimum versions on the network).
pub fn check(
    milestones: &Milestones,
    own_version: &str,
    network_name: &str,
    height: u32,
    gate: &dyn Fn(&str, &Params, &str) -> bool,
) -> Result<Vec<FloorWarning>, FloorError> {
    let _ = (milestones, own_version, network_name, height, gate);
    todo!()
}
