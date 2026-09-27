//! What the runtime asks of the peer-to-peer network: the network monitor's calls, as a trait, so
//! the runtime's tests run against a fake network. [`heartwood_p2p::NetworkMonitor`] implements
//! it (through the `Arc` that owns it, since its background work needs one).

use std::future::Future;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use heartwood_crypto::{Block, Transaction, TransactionId};
use heartwood_p2p::DownloadedBlock;
use heartwood_p2p::NetworkMonitor;
use heartwood_p2p::errors::{DownloadError, SeedError};
use heartwood_p2p::monitor::{CleanseOptions, NetworkStatus};
use heartwood_p2p::seams::{PeerAcceptor, PeerClient, PeerRules};

use crate::pool_task::TransactionBroadcast;

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

    /// The blocks after `from`, in parallel chunks.
    fn download_blocks_from_height(
        &self,
        from: u32,
        max_parallel: usize,
        silent: bool,
        timeout: Duration,
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
}

impl<C, A, R> Network for Arc<NetworkMonitor<C, A, R>>
where
    C: PeerClient + 'static,
    A: PeerAcceptor + 'static,
    R: PeerRules + 'static,
{
    fn boot(&self) -> impl Future<Output = Result<(), SeedError>> + Send {
        NetworkMonitor::boot(self)
    }

    fn update_network_status(&self, initial: bool) -> impl Future<Output = ()> + Send {
        NetworkMonitor::update_network_status(self, initial)
    }

    fn cleanse_peers(&self, options: CleanseOptions) -> impl Future<Output = ()> + Send {
        NetworkMonitor::cleanse_peers(self, options)
    }

    fn discover_peers(
        &self,
        ping_all: bool,
        add_all: bool,
        silent: bool,
    ) -> impl Future<Output = bool> + Send {
        NetworkMonitor::discover_peers(self, ping_all, add_all, silent)
    }

    fn network_height(&self) -> u32 {
        NetworkMonitor::network_height(self)
    }

    fn peer_count(&self) -> usize {
        NetworkMonitor::peers(self).len()
    }

    fn check_network_health(&self, fast: bool) -> impl Future<Output = NetworkStatus> + Send {
        NetworkMonitor::check_network_health(self, fast)
    }

    fn check_for_fork(&self) -> impl Future<Output = u32> + Send {
        NetworkMonitor::check_for_fork(self)
    }

    fn download_block_at_height(
        &self,
        ip: IpAddr,
        height: u32,
    ) -> impl Future<Output = Result<Option<DownloadedBlock>, DownloadError>> + Send {
        NetworkMonitor::download_block_at_height(self, ip, height)
    }

    fn download_blocks_from_height(
        &self,
        from: u32,
        max_parallel: usize,
        silent: bool,
        timeout: Duration,
        check_throttle: bool,
    ) -> impl Future<Output = Vec<DownloadedBlock>> + Send {
        NetworkMonitor::download_blocks_from_height(
            self,
            from,
            max_parallel,
            silent,
            timeout,
            check_throttle,
        )
    }

    fn download_transactions(
        &self,
        exclude: &[TransactionId],
    ) -> impl Future<Output = Vec<Vec<u8>>> + Send {
        NetworkMonitor::download_transactions(self, exclude)
    }

    fn broadcast_block(&self, block: &Block) -> impl Future<Output = ()> + Send {
        NetworkMonitor::broadcast_block(self, block)
    }

    fn broadcast_transactions(&self, transactions: &[Transaction]) {
        NetworkMonitor::broadcast_transactions(self, transactions);
    }

    fn has_minimum_peers(&self, silent: bool) -> bool {
        NetworkMonitor::has_minimum_peers(self, silent)
    }

    fn populate_seed_peers(&self) -> impl Future<Output = Result<(), SeedError>> + Send {
        NetworkMonitor::populate_seed_peers(self)
    }

    fn ping_all(&self) {
        NetworkMonitor::ping_all(self);
    }

    fn synced(&self) {
        NetworkMonitor::synced(self);
    }

    fn refresh_peers_after_fork(&self) -> impl Future<Output = ()> + Send {
        NetworkMonitor::refresh_peers_after_fork(self)
    }

    fn disconnect_invalid_peers(&self) {
        NetworkMonitor::disconnect_invalid_peers(self);
    }

    fn is_cold_start(&self) -> bool {
        NetworkMonitor::is_cold_start(self)
    }

    fn complete_cold_start(&self) {
        NetworkMonitor::complete_cold_start(self);
    }
}

impl<C, A, R> TransactionBroadcast for NetworkMonitor<C, A, R>
where
    C: PeerClient + 'static,
    A: PeerAcceptor + 'static,
    R: PeerRules + 'static,
{
    fn broadcast_transactions(&self, transactions: &[Transaction]) {
        NetworkMonitor::broadcast_transactions(self, transactions);
    }
}
