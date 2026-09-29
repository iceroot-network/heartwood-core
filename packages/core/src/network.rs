//! The network monitor as the blockchain service calls it: [`heartwood_blockchain::Network`] over
//! the monitor that owns the relay's peers.

use std::future::Future;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use heartwood_blockchain::Network;
use heartwood_crypto::{Block, Transaction, TransactionId};
use heartwood_p2p::errors::{DownloadError, SeedError};
use heartwood_p2p::monitor::{CleanseOptions, NetworkStatus};
use heartwood_p2p::seams::{PeerAcceptor, PeerClient, PeerRules};
use heartwood_p2p::{DownloadedBlock, NetworkMonitor};

/// The relay's network monitor, through the `Arc` that owns it (its background work needs one).
pub struct Monitored<C, A, R>(pub Arc<NetworkMonitor<C, A, R>>);

impl<C, A, R> Network for Monitored<C, A, R>
where
    C: PeerClient + 'static,
    A: PeerAcceptor + 'static,
    R: PeerRules + 'static,
{
    fn boot(&self) -> impl Future<Output = Result<(), SeedError>> + Send {
        NetworkMonitor::boot(&self.0)
    }

    fn update_network_status(&self, initial: bool) -> impl Future<Output = ()> + Send {
        NetworkMonitor::update_network_status(&self.0, initial)
    }

    fn cleanse_peers(&self, options: CleanseOptions) -> impl Future<Output = ()> + Send {
        NetworkMonitor::cleanse_peers(&self.0, options)
    }

    fn discover_peers(
        &self,
        ping_all: bool,
        add_all: bool,
        silent: bool,
    ) -> impl Future<Output = bool> + Send {
        NetworkMonitor::discover_peers(&self.0, ping_all, add_all, silent)
    }

    fn network_height(&self) -> u32 {
        NetworkMonitor::network_height(&self.0)
    }

    fn peer_count(&self) -> usize {
        NetworkMonitor::peer_count(&self.0)
    }

    fn check_network_health(&self, fast: bool) -> impl Future<Output = NetworkStatus> + Send {
        NetworkMonitor::check_network_health(&self.0, fast)
    }

    fn check_for_fork(&self) -> impl Future<Output = u32> + Send {
        NetworkMonitor::check_for_fork(&self.0)
    }

    fn download_block_at_height(
        &self,
        ip: IpAddr,
        height: u32,
    ) -> impl Future<Output = Result<Option<DownloadedBlock>, DownloadError>> + Send {
        NetworkMonitor::download_block_at_height(&self.0, ip, height)
    }

    fn download_blocks_from_height(
        &self,
        from: u32,
        max_parallel: usize,
        silent: bool,
        timeout: Option<Duration>,
        check_throttle: bool,
    ) -> impl Future<Output = Vec<DownloadedBlock>> + Send {
        NetworkMonitor::download_blocks_from_height(
            &self.0,
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
        NetworkMonitor::download_transactions(&self.0, exclude)
    }

    fn broadcast_block(&self, block: &Block) -> impl Future<Output = ()> + Send {
        NetworkMonitor::broadcast_block(&self.0, block)
    }

    fn broadcast_transactions(&self, transactions: &[Transaction]) {
        NetworkMonitor::broadcast_transactions(&self.0, transactions);
    }

    fn has_minimum_peers(&self, silent: bool) -> bool {
        NetworkMonitor::has_minimum_peers(&self.0, silent)
    }

    fn populate_seed_peers(&self) -> impl Future<Output = Result<(), SeedError>> + Send {
        NetworkMonitor::populate_seed_peers(&self.0)
    }

    fn ping_all(&self) {
        NetworkMonitor::ping_all(&self.0);
    }

    fn synced(&self) {
        NetworkMonitor::synced(&self.0);
    }

    fn refresh_peers_after_fork(&self) -> impl Future<Output = ()> + Send {
        NetworkMonitor::refresh_peers_after_fork(&self.0)
    }

    fn disconnect_invalid_peers(&self) {
        NetworkMonitor::disconnect_invalid_peers(&self.0);
    }

    fn is_cold_start(&self) -> bool {
        NetworkMonitor::is_cold_start(&self.0)
    }

    fn complete_cold_start(&self) {
        NetworkMonitor::complete_cold_start(&self.0);
    }

    fn refused_block(&self, ip: IpAddr) {
        NetworkMonitor::refused_block(&self.0, ip);
    }
}
