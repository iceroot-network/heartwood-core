//! The machine's thirteen entry actions, by the reference's names. The blockchain service runs
//! them ([`crate::blockchain::Blockchain`]); each dispatches the events that move the machine on.
//!
//! - `initialise`: the start on the restored state: the pool re-added from its storage, the online
//!   record loaded, the monitor booted, then `STARTED` or `NETWORKSTART` (`FAILURE` stops the
//!   node; a failed boot check never rewinds, so `ROLLBACK` is never dispatched).
//! - `checkLastDownloadedBlockSynced`, `downloadBlocks`, `downloadFinished`, `downloadPaused`,
//!   `checkLastBlockSynced`, `syncingComplete`: the sync.
//! - `checkLater` and `blockchainReady`: the idle loops and the wake-up timer, and the start flag.
//! - `startForkRecovery`: the recovery ([`crate::fork_recovery`]).
//! - `rollbackDatabase`, `stopped`, `exitApp`: the rewind (never entered), a stop, an exit with a
//!   non-zero status.

mod idle;
mod sync;

use std::sync::Arc;

use heartwood_kernel::{info, warning};

use crate::blockchain::{Blockchain, Queue, locale};
use crate::network::Network;
use crate::state_machine::MachineEvent;

named! {
    /// An entry action of the machine.
    Action {
        /// `initialise`.
        Initialise = "initialise",
        /// `checkLastDownloadedBlockSynced`.
        CheckLastDownloadedBlockSynced = "checkLastDownloadedBlockSynced",
        /// `downloadBlocks`.
        DownloadBlocks = "downloadBlocks",
        /// `downloadFinished`.
        DownloadFinished = "downloadFinished",
        /// `downloadPaused`.
        DownloadPaused = "downloadPaused",
        /// `checkLastBlockSynced`.
        CheckLastBlockSynced = "checkLastBlockSynced",
        /// `syncingComplete`.
        SyncingComplete = "syncingComplete",
        /// `checkLater`.
        CheckLater = "checkLater",
        /// `blockchainReady`.
        BlockchainReady = "blockchainReady",
        /// `startForkRecovery`.
        StartForkRecovery = "startForkRecovery",
        /// `rollbackDatabase`.
        RollbackDatabase = "rollbackDatabase",
        /// `stopped`.
        Stopped = "stopped",
        /// `exitApp`.
        ExitApp = "exitApp",
    }
}

impl<N: Network, Q: Queue> Blockchain<N, Q> {
    /// Run `action`, as the machine's entry into a state asks.
    pub(crate) async fn run(self: Arc<Self>, action: Action) {
        match action {
            Action::Initialise => self.initialise().await,
            Action::CheckLastDownloadedBlockSynced => {
                self.check_last_downloaded_block_synced().await
            }
            Action::DownloadBlocks => self.download_blocks().await,
            Action::DownloadFinished => {
                info!("Block download finished :rocket:");
                if std::mem::take(&mut self.chain().network_start) {
                    self.dispatch(MachineEvent::SyncFinished);
                } else if !self.queue.is_running() {
                    self.dispatch(MachineEvent::ProcessFinished);
                }
            }
            Action::DownloadPaused => info!("Blockchain download paused :clock1030:"),
            Action::CheckLastBlockSynced => self.dispatch(match self.is_synced(None) {
                true => MachineEvent::Synced,
                false => MachineEvent::NotSynced,
            }),
            Action::SyncingComplete => self.syncing_complete(),
            Action::CheckLater => self.check_later().await,
            Action::BlockchainReady => {
                let mut chain = self.chain();
                if !chain.started {
                    chain.started = true;
                    let accepted = chain.last_accepted_ms;
                    drop(chain);
                    self.flags().seen_accept = accepted;
                    self.set_started();
                }
            }
            Action::StartForkRecovery => self.start_fork_recovery().await,
            // A failed boot check stops the start instead of rewinding the store, so nothing
            // enters this state.
            Action::RollbackDatabase => {
                warning!("The database is not rewound at start :warning:");
                self.dispatch(MachineEvent::Failure);
            }
            Action::Stopped => info!("The blockchain has been stopped :guitar:"),
            Action::ExitApp => {
                heartwood_kernel::error!(
                    "Failed to startup blockchain. Exiting Heartwood Core! :rotating_light:"
                );
                self.flags().failed = true;
                self.queue.stop().await;
            }
        }
    }

    /// `initialise`, on the restored state: the pool re-added from its storage and the online
    /// record saved (the node's drain), the network booted, then `STARTED`; a failure is
    /// `FAILURE`.
    async fn initialise(self: Arc<Self>) {
        info!(
            "Last block in database: {}",
            locale(self.chain().last_height())
        );
        let booted = match self.parts.node.drain().await {
            Ok(()) => self.parts.network.boot().await.map_err(|e| e.to_string()),
            Err(error) => Err(error.to_string()),
        };
        self.dispatch(match booted {
            Ok(()) => MachineEvent::Started,
            Err(error) => {
                heartwood_kernel::error!("{error}");
                MachineEvent::Failure
            }
        });
    }
}
