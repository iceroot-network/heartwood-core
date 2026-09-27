//! The network monitor: boot, discovery, cleansing, the network height, fork health, block and
//! transaction downloads, broadcasts and seeds, as the reference's network monitor runs them.
//!
//! - Boot: the seeds, then (unless `skipDiscovery`) the network status with a first discovery and
//!   one `Discovered N peers with v<version>` line per version.
//! - Network status, every 600 s (60 s below `minimumNetworkReach`, after re-seeding): discovery,
//!   then a cleanse when discovery pinged.
//! - Discovery: getPeers from 8 sampled peers, 50 sampled entries of each reply, merged by IP;
//!   every candidate accepted (less verbose) when pinging all, adding all, below the minimum, or
//!   knowing fewer than 3/4 as many peers as they sent.
//! - Cleanse: every peer (or `peerCount` sampled ones) pinged, 2.5 s when fast and
//!   `verifyTimeout` otherwise, as a race: after the delay the cleanse returns and each peer still
//!   pinging gets an infraction; a failed ping disconnects and forgets the peer (`peer.removed`).
//! - Network height: the median of the peers' heights, the element at `floor(n / 2)` of the sorted
//!   heights; 0 without peers.
//! - Fork health: peers counted once per ranked delegate they attest (and relay peers above our
//!   height added when fewer than half the delegates plus one are counted); the distinct common
//!   heights of the forked results sorted as strings and reversed (reproduced); a rollback to the
//!   first height where more peers are forked than ahead of it, at most 5,000 blocks.
//! - Download: chunks of 400 (a tenth after a download that returned nothing), chunk i asked of
//!   the repository's peer i first and then of the other peers in a random order, cached chunks
//!   above a missing one.
//! - Broadcast: a block to every peer (after the rest of 500 ms from a peer's recent block ping),
//!   transactions in batches through each peer's queue.

use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use heartwood_crypto::{Block, Transaction, TransactionId};
use heartwood_kernel::Clock;
use heartwood_p2p_wire::messages::peer::Plugins;
use indexmap::IndexMap;

use crate::diversity::Sampler;
use crate::download::DownloadedBlock;
use crate::errors::{DownloadError, SeedError};
use crate::options::P2pOptions;
use crate::repository::PeerRepository;
use crate::seams::{PeerAcceptor, PeerClient, PeerRules};
use crate::view::{ChainView, PeerEvents};

/// The largest rollback a fork check asks for.
pub const MAX_ROLLBACK: u32 = 5_000;

/// How a cleanse runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CleanseOptions {
    /// Ping with 2.5 s instead of `verifyTimeout`.
    pub fast: bool,
    /// Ping peers that were pinged recently too.
    pub force_ping: bool,
    /// Log the "Checking N peers" line.
    pub log: bool,
    /// Ping only this many sampled peers.
    pub peer_count: Option<usize>,
    /// Verify fast, without common blocks.
    pub skip_common_blocks: bool,
}

impl Default for CleanseOptions {
    fn default() -> CleanseOptions {
        CleanseOptions {
            fast: false,
            force_ping: false,
            log: true,
            peer_count: None,
            skip_common_blocks: false,
        }
    }
}

/// The result of a fork health check.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NetworkStatus {
    /// Whether the network is on another chain.
    pub forked: bool,
    /// How many blocks to roll back, when forked.
    pub blocks_to_rollback: Option<u32>,
}

/// A peer as the API lists it.
#[derive(Debug, Clone, PartialEq)]
pub struct PeerInfo {
    /// The address.
    pub ip: IpAddr,
    /// The peer-to-peer port.
    pub port: u16,
    /// The port probe's result per plugin.
    pub ports: IndexMap<String, i32>,
    /// The version the peer reported.
    pub version: Option<String>,
    /// The height of the peer's last state.
    pub height: Option<u32>,
    /// The time of the last successful request, in milliseconds.
    pub latency: Option<u32>,
    /// The plugins of the peer's last status.
    pub plugins: Plugins,
}

/// What a monitor is made of.
pub struct MonitorParts<C, A, R> {
    /// The requests to peers.
    pub client: Arc<C>,
    /// The acceptance of candidates.
    pub acceptor: Arc<A>,
    /// The peer rules.
    pub rules: Arc<R>,
    /// The peers.
    pub repository: Arc<PeerRepository>,
    /// The node's chain.
    pub chain: Arc<dyn ChainView>,
    /// Where peer events go.
    pub events: Arc<dyn PeerEvents>,
    /// The p2p options.
    pub options: Arc<P2pOptions>,
    /// The wall clock.
    pub clock: Arc<dyn Clock>,
    /// The random samples.
    pub sampler: Sampler,
}

/// The network monitor.
pub struct NetworkMonitor<C, A, R> {
    #[expect(dead_code, reason = "the monitor's methods read it")]
    parts: MonitorParts<C, A, R>,
}

impl<C, A, R> NetworkMonitor<C, A, R>
where
    C: PeerClient + 'static,
    A: PeerAcceptor + 'static,
    R: PeerRules + 'static,
{
    /// A monitor of `parts`.
    pub fn new(parts: MonitorParts<C, A, R>) -> NetworkMonitor<C, A, R> {
        NetworkMonitor { parts }
    }

    /// Boot: the seeds, then the first network status unless `skipDiscovery`. The network status
    /// then schedules itself.
    pub async fn boot(self: &Arc<Self>) -> Result<(), SeedError> {
        todo!("boot")
    }

    /// Discovery and a cleanse, then the next run scheduled: in 600 s, or in 60 s after re-seeding
    /// when below the minimum.
    pub async fn update_network_status(self: &Arc<Self>, initial: bool) {
        let _ = initial;
        todo!("the network status")
    }

    /// Ping the peers as `options` asks, as a race against the ping delay.
    pub async fn cleanse_peers(&self, options: CleanseOptions) {
        let _ = options;
        todo!("the cleanse")
    }

    /// Discovery; whether the candidates went through acceptance.
    pub async fn discover_peers(&self, ping_all: bool, add_all: bool, silent: bool) -> bool {
        let _ = (ping_all, add_all, silent);
        todo!("discovery")
    }

    /// The median of the peers' heights.
    pub fn network_height(&self) -> u32 {
        todo!("the network height")
    }

    /// The fork health check (a full discovery first unless `fast`).
    pub async fn check_network_health(&self, fast: bool) -> NetworkStatus {
        let _ = fast;
        todo!("the fork health check")
    }

    /// The fork check of the blockchain: 0 without peers; else, after 1 s, a fast health check and
    /// a full one when the fast one saw no fork; the blocks to roll back.
    pub async fn check_for_fork(&self) -> u32 {
        todo!("the fork check")
    }

    /// The block at `height` from the peer at `ip`, if it is a peer (one block, 2 s, silent).
    pub async fn download_block_at_height(
        &self,
        ip: IpAddr,
        height: u32,
    ) -> Result<Option<DownloadedBlock>, DownloadError> {
        let _ = (ip, height);
        todo!("one block")
    }

    /// The blocks after `from`, downloaded in parallel chunks: an unbroken run of heights from
    /// `from + 1`, up to the first chunk no peer gave.
    pub async fn download_blocks_from_height(
        &self,
        from: u32,
        max_parallel: usize,
        silent: bool,
        timeout: Duration,
        check_throttle: bool,
    ) -> Vec<DownloadedBlock> {
        let _ = (from, max_parallel, silent, timeout, check_throttle);
        todo!("the chunked download")
    }

    /// The pool transactions of up to 5 sampled unthrottled peers, without `exclude` and without
    /// the ids seen in the last 30 s.
    pub async fn download_transactions(&self, exclude: &[TransactionId]) -> Vec<Vec<u8>> {
        let _ = exclude;
        todo!("the transaction download")
    }

    /// Broadcast `block` to every peer.
    pub async fn broadcast_block(&self, block: &Block) {
        let _ = block;
        todo!("the block broadcast")
    }

    /// Broadcast `transactions` to every peer, in batches.
    pub fn broadcast_transactions(&self, transactions: &[Transaction]) {
        let _ = transactions;
        todo!("the transaction broadcast")
    }

    /// Whether the node knows `minimumNetworkReach` peers (always with
    /// `ignoreMinimumNetworkReach`, with a warning unless `silent`).
    pub fn has_minimum_peers(&self, silent: bool) -> bool {
        let _ = silent;
        todo!("the minimum reach")
    }

    /// Forget each seed and accept it again; [`SeedError::NoSeeds`] stops the node.
    pub async fn populate_seed_peers(&self) -> Result<(), SeedError> {
        todo!("the seeds")
    }

    /// A fast forced cleanse without common blocks, at most every 10 s, in the background.
    pub fn ping_all(self: &Arc<Self>) {
        todo!("ping all")
    }

    /// A forced cleanse after a fork recovery.
    pub async fn refresh_peers_after_fork(&self) {
        todo!("the refresh after a fork")
    }

    /// Disconnect the peers whose version no longer passes the version gate (a milestone change).
    pub fn disconnect_invalid_peers(&self) {
        todo!("the peers of an invalid version")
    }

    /// The peers, as the API lists them.
    pub fn peers(&self) -> Vec<PeerInfo> {
        todo!("the peer list")
    }
}
