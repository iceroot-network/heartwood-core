//! The local devnet library, mode B: every node of a devnet on one host, each at its own loopback
//! address, as separate relay and forger processes under their supervisors.
//!
//! - **Addresses.** Node k's relay listens at `127.P.(10+k)`, P2P on 6002 and the API on 6003,
//!   where `P` is the address prefix (default `127.0.0`); its forger connects from the same
//!   address, or in a failover layout from `127.P.(110+k)`, which each relay it may use lists in
//!   its `remoteAccess`.
//! - **`init`** checks the counts, test-binds every listener, has the chain generated into
//!   `genesis/`, writes each `node<k>/` (token `node<k>`, network `devnet`, its `CORE_PATH_*`, its
//!   `.env`, `app.json`, the forger's `hosts` and `localAddress`, its share of the delegates' keys
//!   split 11/11/11/10/10 over five validators, `peers.json` of the other nodes, the crypto files)
//!   and the manifest `devnet.json` last.
//! - **`start`** checks the crypto files' hashes, re-stamps the epoch when no block above height 1
//!   exists and the epoch is less than 10 s away or past, starts every relay, waits for their API
//!   ports (30 s at most), then starts the forgers. **`stop`** stops the forgers before the relays;
//!   `--kill` records the stop and sends SIGKILL.
//! - **`status`** reads the supervisors and each relay's node status, last block and peers,
//!   compares the block ids at the lowest common height (`consistent` or `DIVERGED at h`), and
//!   counts the delegates whose forger is online against the quorum's 27.
//! - **`reset`** deletes a node's data, logs, temp and cache; **`wait`** polls a relay's height
//!   every 2 s; **`logs`** merges the nodes' logs with prefixes; **`env`** prints a node's
//!   environment.
//! - **Guards.** A host that is not loopback, a taken address and a network whose name is not
//!   `devnet` are refused.

use std::io::Write;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::time::Duration;

use heartwood_kernel::Clock;

use crate::errors::DevnetError;
use crate::seams::{ChainGenerator, Env, NodeApi, ProcessOps, Role, StatusRow};

/// Every node's peer-to-peer port.
pub const P2P_PORT: u16 = 6002;
/// Every node's API port.
pub const API_PORT: u16 = 6003;
/// The manifest's file name in the devnet's directory.
pub const MANIFEST: &str = "devnet.json";
/// The default address prefix.
pub const DEFAULT_PREFIX: [u8; 3] = [127, 0, 0];
/// The quorum a forging devnet needs: at least 27 of the 53 delegates.
pub const QUORUM: u32 = 27;

/// Node `k`'s address (1-based): `127.P.(10+k)`.
pub fn node_address(prefix: [u8; 3], k: u32) -> Option<IpAddr> {
    let last = u8::try_from(k).ok()?.checked_add(10)?;
    Some(IpAddr::V4(Ipv4Addr::new(
        prefix[0], prefix[1], prefix[2], last,
    )))
}

/// The address of node `k`'s forger in a failover layout: `127.P.(110+k)`.
pub fn forger_address(prefix: [u8; 3], k: u32) -> Option<IpAddr> {
    let last = u8::try_from(k).ok()?.checked_add(110)?;
    Some(IpAddr::V4(Ipv4Addr::new(
        prefix[0], prefix[1], prefix[2], last,
    )))
}

/// What `init` builds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitOptions {
    /// The nodes with a relay and a forger (1 to 5).
    pub validators: u32,
    /// The nodes with a relay only, after the validators.
    pub relays: u32,
    /// The chain's key-derivation seed.
    pub seed: String,
    /// The first three bytes of every node address.
    pub address_prefix: [u8; 3],
    /// The seconds from `init` (or from a re-stamping `start`) to the epoch.
    pub epoch_delay: u32,
    /// The p2p `minimumNetworkReach` of every relay.
    pub min_network_reach: u32,
    /// Every node's log level.
    pub log_level: String,
    /// Replace a devnet the directory already holds.
    pub force: bool,
}

/// One node of a devnet, as the manifest describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestNode {
    /// The node's number, from 1.
    pub index: u32,
    /// The relay's address.
    pub address: IpAddr,
    /// The forger's source address, when the node has a forger.
    pub forger_address: Option<IpAddr>,
    /// The relays the forger uses, in order.
    pub forger_hosts: Vec<SocketAddr>,
    /// How many delegates the node's forger holds.
    pub delegates: u32,
    /// The node's directory.
    pub dir: PathBuf,
}

/// The manifest `devnet.json`, written last by `init`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    /// The chain's seed.
    pub seed: String,
    /// The address prefix.
    pub address_prefix: [u8; 3],
    /// The chain's nethash.
    pub nethash: String,
    /// The chain's epoch.
    pub epoch: String,
    /// The SHA-256 of each crypto file, by file name.
    pub crypto_hashes: Vec<(String, String)>,
    /// The nodes, in order.
    pub nodes: Vec<ManifestNode>,
}

/// The nodes and roles a command acts on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeSelection {
    /// The nodes, by number; every node when empty.
    pub nodes: Vec<u32>,
    /// The role; both when `None`.
    pub role: Option<Role>,
}

/// Whether the relays agree on their chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Consistency {
    /// Every relay has the same block at the lowest common height.
    Consistent,
    /// The relays' blocks differ at this height.
    Diverged {
        /// The lowest common height.
        height: u32,
    },
}

/// What `status` reports.
#[derive(Debug, Clone, PartialEq)]
pub struct DevnetStatus {
    /// Each node's supervised roles.
    pub processes: Vec<(u32, Vec<StatusRow>)>,
    /// Each relay's height, when its API answered.
    pub heights: Vec<(u32, Option<u32>)>,
    /// The chain's consistency across the relays that answered.
    pub consistency: Consistency,
    /// The delegates whose forger is online.
    pub online_delegates: u32,
}

/// Create the devnet of `options` in `root`, its chain from `generator`, its epoch from `clock`.
pub fn init(
    root: &Path,
    options: &InitOptions,
    generator: &dyn ChainGenerator,
    clock: &dyn Clock,
) -> Result<Manifest, DevnetError> {
    let _ = (root, options, generator, clock);
    todo!("devnet init")
}

/// Start the selected nodes: the relays, then (once their APIs answer) the forgers.
pub fn start(
    root: &Path,
    nodes: &NodeSelection,
    ops: &dyn ProcessOps,
    api: &dyn NodeApi,
    clock: &dyn Clock,
) -> Result<(), DevnetError> {
    let _ = (root, nodes, ops, api, clock);
    todo!("devnet start")
}

/// Stop the selected nodes, forgers before relays; with `kill`, by SIGKILL after recording the
/// stop.
pub fn stop(
    root: &Path,
    nodes: &NodeSelection,
    kill: bool,
    ops: &dyn ProcessOps,
) -> Result<(), DevnetError> {
    let _ = (root, nodes, kill, ops);
    todo!("devnet stop")
}

/// Stop node `node` (`stop-node`).
pub fn stop_node(
    root: &Path,
    node: u32,
    role: Option<Role>,
    kill: bool,
    ops: &dyn ProcessOps,
) -> Result<(), DevnetError> {
    let _ = (root, node, role, kill, ops);
    todo!("devnet stop-node")
}

/// Start node `node` (`start-node`).
pub fn start_node(
    root: &Path,
    node: u32,
    role: Option<Role>,
    ops: &dyn ProcessOps,
    api: &dyn NodeApi,
    clock: &dyn Clock,
) -> Result<(), DevnetError> {
    let _ = (root, node, role, ops, api, clock);
    todo!("devnet start-node")
}

/// Restart node `node` gracefully (`restart-node`).
pub fn restart_node(
    root: &Path,
    node: u32,
    role: Option<Role>,
    ops: &dyn ProcessOps,
    api: &dyn NodeApi,
    clock: &dyn Clock,
) -> Result<(), DevnetError> {
    let _ = (root, node, role, ops, api, clock);
    todo!("devnet restart-node")
}

/// The devnet's status.
pub fn status(
    root: &Path,
    ops: &dyn ProcessOps,
    api: &dyn NodeApi,
) -> Result<DevnetStatus, DevnetError> {
    let _ = (root, ops, api);
    todo!("devnet status")
}

/// Write the last `lines` lines of the selected nodes' logs to `out`, each prefixed with its
/// node and role.
pub fn logs(
    root: &Path,
    nodes: &NodeSelection,
    lines: usize,
    out: &mut dyn Write,
) -> Result<(), DevnetError> {
    let _ = (root, nodes, lines, out);
    todo!("devnet logs")
}

/// Delete node `node`'s data, logs, temp and cache; its roles must be stopped.
pub fn reset(root: &Path, node: u32, ops: &dyn ProcessOps) -> Result<(), DevnetError> {
    let _ = (root, node, ops);
    todo!("devnet reset")
}

/// Wait until a relay of the devnet reaches `height`, polling every 2 s, for at most `timeout`;
/// `pause` waits between polls. Gives the height reached.
pub fn wait(
    root: &Path,
    height: u32,
    timeout: Duration,
    api: &dyn NodeApi,
    pause: &dyn Fn(Duration),
) -> Result<u32, DevnetError> {
    let _ = (root, height, timeout, api, pause);
    todo!("devnet wait")
}

/// Node `node`'s environment: its `CORE_PATH_*` and `.env` values.
pub fn env(root: &Path, node: u32) -> Result<Env, DevnetError> {
    let _ = (root, node);
    todo!("devnet env")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nodes_and_forgers_have_their_loopback_addresses() {
        assert_eq!(
            node_address(DEFAULT_PREFIX, 1),
            Some("127.0.0.11".parse().unwrap())
        );
        assert_eq!(
            forger_address([127, 2, 0], 1),
            Some("127.2.0.111".parse().unwrap())
        );
        assert_eq!(node_address(DEFAULT_PREFIX, 246), None);
    }
}
