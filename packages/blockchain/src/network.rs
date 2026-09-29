//! What the runtime asks of the peer-to-peer network: the network monitor's calls, as a trait, so
//! the runtime's tests run against a fake network. The relay's composition root implements it over
//! [`heartwood_p2p::NetworkMonitor`].

use std::future::Future;
use std::net::IpAddr;
use std::time::Duration;

use heartwood_crypto::{Block, Transaction, TransactionId};
use heartwood_p2p::DownloadedBlock;
use heartwood_p2p::errors::{DownloadError, SeedError};
use heartwood_p2p::monitor::{CleanseOptions, NetworkStatus};

/// The network monitor's calls that the runtime makes.
pub trait Network: Send + Sync + 'static {
    /// Boot: the seeds, then (unless discovery is skipped) the first network status.
    fn boot(&self) -> impl Future<Output = Result<(), SeedError>> + Send;

    /// Discovery, then a cleanse when discovery pinged.
    fn update_network_status(&self, initial: bool) -> impl Future<Output = ()> + Send;

    /// Ping every peer (or a sample), as a race against the options' delay.
    fn cleanse_peers(&self, options: CleanseOptions) -> impl Future<Output = ()> + Send;

    /// getPeers from sampled peers, and acceptance of the candidates; whether it accepted them.
    fn discover_peers(
        &self,
        ping_all: bool,
        add_all: bool,
        silent: bool,
    ) -> impl Future<Output = bool> + Send;

    /// The median of the peers' heights.
    fn network_height(&self) -> u32;

    /// How many peers the node knows.
    fn peer_count(&self) -> usize;

    /// Whether the network is forked, and how many blocks to roll back.
    fn check_network_health(&self, fast: bool) -> impl Future<Output = NetworkStatus> + Send;

    /// The rollback a fork check asks for; 0 without a fork.
    fn check_for_fork(&self) -> impl Future<Output = u32> + Send;

    /// The block at `height` from the peer at `ip`.
    fn download_block_at_height(
        &self,
        ip: IpAddr,
        height: u32,
    ) -> impl Future<Output = Result<Option<DownloadedBlock>, DownloadError>> + Send;

    /// The blocks after `from`, in parallel chunks, each getBlocks with `timeout`, or with the
    /// configured `getBlocksTimeout` when `None` (the reference's call without a timeout).
    fn download_blocks_from_height(
        &self,
        from: u32,
        max_parallel: usize,
        silent: bool,
        timeout: Option<Duration>,
        check_throttle: bool,
    ) -> impl Future<Output = Vec<DownloadedBlock>> + Send;

    /// The pool transactions of sampled peers, without `exclude` and those seen in the last 30 s.
    fn download_transactions(
        &self,
        exclude: &[TransactionId],
    ) -> impl Future<Output = Vec<Vec<u8>>> + Send;

    /// Broadcast `block` to every peer.
    fn broadcast_block(&self, block: &Block) -> impl Future<Output = ()> + Send;

    /// Broadcast `transactions` to every peer.
    fn broadcast_transactions(&self, transactions: &[Transaction]);

    /// Whether the node knows `minimumNetworkReach` peers.
    fn has_minimum_peers(&self, silent: bool) -> bool;

    /// Forget each seed and accept it again.
    fn populate_seed_peers(&self) -> impl Future<Output = Result<(), SeedError>> + Send;

    /// A fast forced cleanse, at most every 10 s, in the background.
    fn ping_all(&self);

    /// The `blockchain.synced` listener: `pingAll` after 1 s.
    fn synced(&self);

    /// A forced cleanse after a fork recovery.
    fn refresh_peers_after_fork(&self) -> impl Future<Output = ()> + Send;

    /// Disconnect the peers whose version no longer passes the gate.
    fn disconnect_invalid_peers(&self);

    /// Whether the node started a new network and has not completed its cold start.
    fn is_cold_start(&self) -> bool;

    /// Complete the cold start.
    fn complete_cold_start(&self);

    /// The peer at `ip` served a block that no honest peer serves (a fault the reference does not
    /// score): a violation of the peer, which is then asked for no blocks until it expires.
    fn refused_block(&self, _ip: IpAddr) {}
}
