//! `checkLater`: the wait for the epoch, the idle loops (started once) and the wake-up timer.
//!
//! | Every | While idle |
//! |---|---|
//! | 500 ms | download from the tip (silent, 2 s, throttled peers skipped); of the blocks above the tip (read again after the download), one block named by a ranked delegate is logged once per block and tip and handled as an incoming block without the event; several are `NEWBLOCK` and enqueued |
//! | 4 s | the pool transactions of sampled peers, without the pool's ids, to the download channel |
//! | 1 s | below the minimum network reach: the seeds again |
//! | 30 s | discovery that adds every candidate, then a cleanse when it added |
//!
//! Each tick runs on its own task, as `setInterval` runs its callback: a slow tick does not delay
//! the next one. The first entry into idle also starts a fast forced cleanse without common
//! blocks.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use heartwood_crypto::identities::PublicKeyBytes;
use heartwood_crypto::utils::format_satoshi;
use heartwood_kernel::utils::pluralise::pluralise;
use heartwood_kernel::{debug, info};
use heartwood_p2p::DownloadedBlock;
use heartwood_p2p::monitor::CleanseOptions;
use heartwood_pool::Entry;

use crate::blockchain::{Blockchain, Queue, link, locale};
use crate::network::Network;
use crate::state_machine::MachineEvent;
use crate::work::{Channel, IncomingBlock};

impl<N: Network, Q: Queue> Blockchain<N, Q> {
    /// `checkLater`: wait for the epoch slot by slot; then, unless stopped or the wake-up timer is
    /// set, start the idle loops the first time and set the wake-up timer.
    pub(super) async fn check_later(self: Arc<Self>) {
        let seconds = self.seconds_to_epoch();
        if seconds > 0 {
            info!(
                "The network launches in {} :stopwatch:",
                pluralise("second", seconds.into(), true)
            );
            // The slots start at the epoch, so the reference's wait slot by slot ends there.
            let until = self.epoch_ms().saturating_sub(self.parts.clock.now_ms());
            tokio::time::sleep(Duration::from_millis(u64::try_from(until).unwrap_or(0))).await;
            let slot = pluralise("second", self.block_ms().div_euclid(1000).into(), true);
            info!(
                "The network has launched and the next block is due in {slot}! Welcome aboard :tada:"
            );
        }
        let now = self.parts.clock.now_ms();
        let first = {
            let mut flags = self.flags();
            if flags.stopped || flags.wake_up.is_some() {
                return;
            }
            flags.wake_up = Some(now);
            !std::mem::replace(&mut flags.polled, true)
        };
        if first {
            let fast = CleanseOptions {
                fast: true,
                force_ping: true,
                log: false,
                skip_common_blocks: true,
                ..CleanseOptions::default()
            };
            let service = Arc::clone(&self);
            self.spawn(async move { service.parts.network.cleanse_peers(fast).await });
            self.every(Duration::from_millis(500), Blockchain::poll_blocks);
            self.every(Duration::from_secs(4), Blockchain::poll_transactions);
            self.every(Duration::from_secs(1), Blockchain::reseed);
            self.every(Duration::from_secs(30), Blockchain::rediscover);
        }
    }

    /// Run `tick` every `period` while idle, each tick on its own task.
    fn every<F, T>(self: &Arc<Self>, period: Duration, tick: F)
    where
        F: Fn(Arc<Self>) -> T + Send + 'static,
        T: Future<Output = ()> + Send + 'static,
    {
        let service = Arc::downgrade(self);
        let task = self.parts.runtime.spawn(async move {
            let mut interval = tokio::time::interval(period);
            // The first tick is at once; `setInterval` runs its callback first after one period.
            interval.tick().await;
            loop {
                interval.tick().await;
                let Some(service) = service.upgrade() else {
                    return;
                };
                if service.flags().stopped {
                    return;
                }
                if service.state().is_idle() {
                    service.spawn(tick(Arc::clone(&service)));
                }
            }
        });
        self.tasks().push(task);
    }

    /// The 500 ms block poll.
    async fn poll_blocks(self: Arc<Self>) {
        let tip = self.chain().last_height();
        let network = &self.parts.network;
        let two = Duration::from_secs(2);
        let blocks = network
            .download_blocks_from_height(tip, 10, true, two, true)
            .await;
        let tip = self.chain().last_block().cloned();
        let Some(tip) = tip.filter(|_| self.state().is_idle()) else {
            return;
        };
        let mut blocks: Vec<_> = blocks
            .into_iter()
            .filter(|b| b.height() > tip.height())
            .collect();
        match blocks.pop() {
            Some(block) if blocks.is_empty() => self.poll_one(block, *tip.id()).await,
            Some(block) => {
                blocks.push(block);
                self.dispatch(MachineEvent::NewBlock);
                self.enqueue_blocks(blocks.into_iter().map(IncomingBlock::downloaded).collect());
            }
            None => {}
        }
    }

    /// The block poll's single block above the tip `tip`.
    async fn poll_one(self: Arc<Self>, block: DownloadedBlock, tip: heartwood_crypto::BlockId) {
        let header = &block.header.header;
        let key = header.generator_public_key.as_deref();
        let key = key.and_then(|key| PublicKeyBytes::from_hex(key).ok());
        let state = self.parts.node.state().borrow().clone();
        let delegate = (key.filter(|_| header.version == Some(0)))
            .and_then(|key| state.wallets.find_by_public_key(&key)?.delegate().cloned());
        let (Some(delegate), Some(link)) = (delegate, link(&block)) else {
            return;
        };
        let Some(rank) = delegate.rank else {
            return;
        };
        if self.flags().last_polled == Some((link.id, tip)) {
            return;
        }
        let (name, height) = (delegate.username(), locale(link.height));
        let reward = format_satoshi(block.header.reward.value(), "");
        let reward = reward.trim_end();
        info!(
            "Downloaded new block forged by delegate {name} (#{rank}) at height {height} with {reward} reward :package:"
        );
        debug!("The id of the new block is {}", link.id);
        self.handle_incoming_block(IncomingBlock::downloaded(block), false)
            .await;
        self.flags().last_polled = Some((link.id, tip));
    }

    /// The 4 s transaction pull.
    async fn poll_transactions(self: Arc<Self>) {
        let pool = self.parts.node.pool().borrow().clone();
        let exclude: Vec<_> = pool.by_priority.iter().map(|t| *t.id()).collect();
        let transactions = self.parts.network.download_transactions(&exclude).await;
        if !transactions.is_empty() {
            let entries = transactions.into_iter().map(Entry::Bytes).collect();
            let _ = self
                .parts
                .node
                .pool_entries(Channel::Download, entries)
                .await;
        }
    }

    /// The 1 s re-seed below the minimum network reach.
    async fn reseed(self: Arc<Self>) {
        if !self.parts.network.has_minimum_peers(true) {
            let _ = self.parts.network.populate_seed_peers().await;
        }
    }

    /// The 30 s discovery.
    async fn rediscover(self: Arc<Self>) {
        if self.parts.network.discover_peers(false, true, true).await {
            let quiet = CleanseOptions {
                log: false,
                ..CleanseOptions::default()
            };
            self.parts.network.cleanse_peers(quiet).await;
        }
    }

    /// Whole seconds from now to the epoch, as the reference compares them.
    pub(super) fn seconds_to_epoch(&self) -> i64 {
        let now = self.parts.clock.now_ms().div_euclid(1000);
        self.epoch_ms().div_euclid(1000).saturating_sub(now)
    }
}
