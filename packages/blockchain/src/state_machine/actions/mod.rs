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
