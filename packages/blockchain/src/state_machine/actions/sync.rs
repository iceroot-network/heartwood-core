//! The sync's actions: `checkLastDownloadedBlockSynced`, `downloadBlocks` and `syncingComplete`.

use std::sync::Arc;

use heartwood_kernel::utils::pluralise::pluralise;
use heartwood_kernel::{calculate_round, debug, info, is_block_chained, warning};

use crate::blockchain::{Blockchain, GET_BLOCKS_TIMEOUT as TIMEOUT, Queue, link, locale};
use crate::network::Network;
use crate::state_machine::MachineEvent;
use crate::work::{IncomingBlock, OnlineWork};

impl<N: Network, Q: Queue> Blockchain<N, Q> {
    /// `checkLastDownloadedBlockSynced`: `SYNCED` before the epoch; `PAUSED` with more than 100
    /// queued jobs; `NETWORKHALTED` after more than 3 empty downloads with the queue idle (every
    /// third time a full health check may make it `FORK`); `SYNCED` when the last downloaded block
    /// is synced, or with `--networkStart`; else `NOTSYNCED`.
    pub(super) async fn check_last_downloaded_block_synced(self: Arc<Self>) {
        if self.seconds_to_epoch() >= 0 {
            return self.dispatch(MachineEvent::Synced);
        }
        let (size, running) = (self.queue.size(), self.queue.is_running());
        debug!("Queued chunks of blocks (process: {size})");
        let mut event = match size > 100 {
            true => MachineEvent::Paused,
            false => MachineEvent::NotSynced,
        };
        let halted = {
            let mut chain = self.chain();
            let halted = chain.no_block_counter > 3 && !running;
            if halted {
                chain.no_block_counter = 0;
            }
            halted.then_some(chain.p2p_update_counter)
        };
        if let Some(updates) = halted {
            info!(
                "Tried to sync 3 times to different nodes, looks like the network is missing blocks :umbrella:"
            );
            event = MachineEvent::NetworkHalted;
            if updates.saturating_add(1) > 2 {
                info!("Network keeps missing blocks :umbrella:");
                let status = self.parts.network.check_network_health(false).await;
                let mut chain = self.chain();
                if status.forked {
                    chain.blocks_to_rollback = status.blocks_to_rollback.unwrap_or(0);
                    event = MachineEvent::Fork;
                }
                chain.p2p_update_counter = 0;
            } else {
                self.chain().p2p_update_counter = updates.saturating_add(1);
            }
        } else {
            let last = self.chain().last_downloaded;
            if last.is_some_and(|last| self.is_synced(Some(&last))) {
                let mut chain = self.chain();
                chain.no_block_counter = 0;
                chain.p2p_update_counter = 0;
                event = MachineEvent::Synced;
            }
        }
        if self.chain().network_start {
            event = MachineEvent::Synced;
        }
        self.dispatch(event);
    }

    /// `downloadBlocks`: download from the last downloaded block; blocks that chain to it are
    /// enqueued (`DOWNLOADED`); nothing, or blocks that do not chain (the queue cleared and a fork
    /// check in the background), is `NOBLOCK`, counted as an empty download when the queue is
    /// empty, with the last downloaded block back at the tip.
    pub(super) async fn download_blocks(self: Arc<Self>) {
        let last = self.chain().last_downloaded;
        let Some(last) = last.or_else(|| self.tip_link()) else {
            return;
        };
        let network = &self.parts.network;
        let blocks = network.download_blocks_from_height(last.height, 10, false, TIMEOUT, false);
        let blocks = blocks.await;
        if self.flags().stopped {
            return;
        }
        // It could have changed meanwhile, as after a rollback.
        if (self.chain().last_downloaded.as_ref()).is_some_and(|now| now.id != last.id) {
            return;
        }
        let first = blocks.first().and_then(link);
        let milestones = &self.parts.milestones;
        if first.is_some_and(|first| is_block_chained(&last, &first, milestones).is_ok()) {
            let count = blocks.len();
            let transactions = blocks.iter().try_fold(0_i128, |sum, block| {
                let count = block.header.header.number_of_transactions?;
                Some(sum.saturating_add(count.into()))
            });
            let total = transactions.map_or("NaN transactions".into(), |sum| {
                pluralise("transaction", sum, true)
            });
            let noun = pluralise("block", i128::try_from(count).unwrap_or(0), false);
            info!("Downloaded {count} new {noun} accounting for a total of {total}");
            let tail = blocks.last().and_then(link);
            self.enqueue_blocks(blocks.into_iter().map(IncomingBlock::downloaded).collect());
            self.chain().last_downloaded = tail;
            return self.dispatch(MachineEvent::Downloaded);
        }
        if blocks.is_empty() {
            let from = locale(last.height.saturating_add(1));
            info!("Could not download any blocks from any peer from height {from} :warning:");
        } else {
            self.queue.clear();
            let service = Arc::clone(&self);
            self.spawn(async move {
                let blocks: Vec<_> = blocks.into_iter().map(IncomingBlock::downloaded).collect();
                if !service.flags().fork_check && !service.check_for_fork(&blocks).await {
                    warning!("Downloaded block not accepted :warning: :warning: :warning:");
                    let first = blocks.first().map(IncomingBlock::height).unwrap_or(0);
                    warning!("First downloaded block at height {first}");
                    warning!("Last downloaded block at height {}", last.height);
                }
            });
        }
        if self.queue.size() == 0 {
            let mut chain = self.chain();
            chain.no_block_counter = chain.no_block_counter.saturating_add(1);
            chain.reset_last_downloaded();
        }
        self.dispatch(MachineEvent::NoBlock);
    }

    /// `syncingComplete`: the online record's versions older than the round minus 5 expire,
    /// `blockchain.synced`, then `SYNCFINISHED`.
    pub(super) fn syncing_complete(self: &Arc<Self>) {
        info!("Blockchain 100% in sync :100:");
        let height = self.chain().last_height();
        let round = calculate_round(height, &self.parts.milestones).round;
        let _ = self.parts.node.online(OnlineWork::Expire { round });
        self.parts.network.synced();
        self.dispatch(MachineEvent::SyncFinished);
    }
}
