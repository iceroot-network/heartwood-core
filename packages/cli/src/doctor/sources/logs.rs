//! A role's log window: the known lines of its current log file (and of the newest rotated file
//! when the window reaches before the current file's first line) as typed events, the count of
//! other lines, capped excerpts as untrusted text, and the error capture since the role's start.
//!
//! A log line is evidence that something was logged, not that it happened: text from peers and
//! relays reaches the log. The node escapes control characters in every message, so outside text
//! cannot start a line of its own in logs this build writes; logs of older builds, and any text a
//! message quotes, stay untrusted. A line whose time runs more than 1 s backwards against the line
//! before it in the same file is counted in [`LogWindow::untrusted_lines`] and is not an event.
//!
//! Limits: the last [`MAX_TAIL`] bytes of each file, [`MAX_RUN`] bytes a run (the view's budget),
//! excerpts of at most [`MAX_EXCERPT`] bytes.

use std::net::IpAddr;
use std::path::PathBuf;

use heartwood_kernel::Clock;
use heartwood_kernel::log::Level;

use crate::doctor::report::{Id, Untrusted};
use crate::doctor::seams::DoctorFs;
use crate::doctor::sources::fs::ReadableRoots;
use crate::doctor::view::{Budget, Reason, Source, Unknown, Window};
use crate::seams::Role;

/// The most read from the end of one file.
pub const MAX_TAIL: u64 = 8 * 1024 * 1024;
/// The most read from every log file of a run together.
pub const MAX_RUN: u64 = 32 * 1024 * 1024;
/// The longest excerpt kept.
pub const MAX_EXCERPT: usize = 512;
/// How far a line's time may run backwards against the line before it and still count.
pub const BACKWARDS_MS: i64 = 1_000;

/// Why a role's start failed, from the start errors the relay and the forger print (their error
/// capture, and the relay's own log at error level). The kinds are the node's own error types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StartError {
    /// "the P2P server cannot listen on ...": `in_use` when the system said the address is in use.
    P2pCannotListen {
        /// The port, when the line names one.
        port: Option<u16>,
        /// Whether the address is in use.
        in_use: bool,
    },
    /// "the API cannot listen on ...".
    ApiCannotListen {
        /// The port, when the line names one.
        port: Option<u16>,
        /// Whether the address is in use.
        in_use: bool,
    },
    /// The version floor in force refuses this release.
    VersionFloor,
    /// `CORE_API_SSL` is set: the API serves plain HTTP only.
    ApiHttps,
    /// The key file does not exist.
    KeyFileMissing,
    /// The key file holds no private key.
    NoKeys,
    /// The key file is open to its group or to others.
    KeyFileTooOpen {
        /// Its permission bits.
        mode: u32,
    },
    /// The key file lists passphrases, which the forger does not read.
    KeyFileSecrets,
    /// The key file is not valid or cannot be read.
    KeyFileInvalid,
    /// The signing record belongs to another network.
    SigningRecordOtherNetwork,
    /// The signing record is not valid or cannot be read or written.
    SigningRecordInvalid,
    /// A forger option is not valid.
    ForgerOption,
    /// The configuration is refused.
    Config,
    /// Another start error.
    Other,
}

/// A known log line, with its typed captures. A captured name is kept only when it passes the
/// username grammar; a line whose captures do not parse counts as other.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LineKind {
    // The relay.
    /// "Heartwood Core `<version>`: a relay of `<token>` on `<network>`".
    RelayStarted,
    /// "Blockchain 100% in sync".
    InSync,
    /// "Starting fork recovery".
    ForkRecoveryStarted,
    /// "Fork detected - rolling back ... to fork at height H (...)".
    ForkDetected {
        /// The fork's height.
        height: u32,
    },
    /// "Refreshing N peers after fork".
    PeersRefreshedAfterFork {
        /// The peers refreshed.
        peers: u32,
    },
    /// "Removing N blocks. Reset to height H".
    BlocksRemoved {
        /// The blocks removed.
        blocks: u32,
        /// The height reset to.
        reset_to: u32,
    },
    /// "Downloaded block not accepted".
    DownloadedBlockNotAccepted,
    /// "Could not download any blocks from any peer from height H".
    NoBlocksFromAnyPeer {
        /// The height.
        from: u32,
    },
    /// "Could not download blocks: we have 0 peers".
    NoPeersToDownload,
    /// "Failed to download blocks ... from ...: `<payload>`": carries a peer's text, kept only as
    /// an excerpt.
    DownloadFailed,
    /// "Couldn't find enough peers. Falling back to seed peers".
    FallingBackToSeeds,
    /// "Not enough peers available to check network health".
    NotEnoughPeersForHealth,
    /// "No verified peers available to check network health".
    NoVerifiedPeersForHealth,
    /// "Median Network Height: H".
    MedianNetworkHeight {
        /// The height.
        height: u32,
    },
    /// "Network keeps missing blocks".
    NetworkMissingBlocks,
    /// "Discarded block H because it takes a future slot" (at info or error).
    DiscardedFutureSlot {
        /// The block's height.
        height: u32,
    },
    /// "Discarded block H because it was received too late".
    DiscardedReceivedLate {
        /// The block's height.
        height: u32,
    },
    /// "Discarded block H because it was processed too late".
    DiscardedProcessedLate {
        /// The block's height.
        height: u32,
    },
    /// "Shutting down app, because state is corrupted".
    StateCorrupted,
    /// "Shutting down app, because state might be corrupted".
    StateMightBeCorrupted,
    /// "The node stops: ...".
    NodeStops,
    /// "Detected double forging by D".
    DoubleForgingDetected {
        /// The delegate, when its name passes the grammar.
        username: Option<Id>,
    },
    /// "A new attestation session from A replaces the session from B".
    AttestationSessionReplaced {
        /// The new session's address.
        new: IpAddr,
        /// The replaced session's address.
        old: IpAddr,
    },
    /// "The attestation session from A declares N keys again; ...".
    AttestationKeysRepeated {
        /// The session's address.
        from: IpAddr,
        /// The keys declared again.
        repeated: u32,
    },
    /// "Received new block forged by delegate D (#R) at height H with ... reward": its time is the
    /// relay's clock at receipt.
    BlockReceived {
        /// The generator.
        username: Id,
        /// Its rank.
        rank: u32,
        /// The block's height.
        height: u32,
    },
    /// A `remoteAccess` entry that matches no address.
    RemoteAccessNoMatch,
    /// "remoteAccess is empty: no forger can reach this relay's internal routes".
    RemoteAccessEmpty,
    /// "The API is disabled".
    ApiDisabled,
    /// "...: the API is not started".
    ApiNotStarted,
    /// A start error the relay or forger logged at error level before it exited.
    StartError(StartError),
    /// "From height H the minimum versions are ..., which this node's version ... does not
    /// satisfy".
    FloorLaterRefuses {
        /// The milestone's height.
        height: u32,
    },

    // The forger.
    /// "Heartwood Core `<version>`: a forger of `<token>` on `<network>`".
    ForgerStarted,
    /// "Forger Manager started".
    ForgerManagerStarted,
    /// "Loaded N active delegates: ..." or "Loaded N inactive delegates: ...".
    DelegatesLoaded {
        /// Active (true) or inactive.
        active: bool,
        /// N.
        count: u32,
        /// The names that pass the username grammar.
        names: Vec<Id>,
    },
    /// "Delegate D is now in an active forging position" (and the plural), or "... no longer
    /// in ...".
    ActivePosition {
        /// Entered (true) or left.
        entered: bool,
        /// The names that pass the username grammar.
        names: Vec<Id>,
    },
    /// "Delegate D is due to forge in N seconds from now".
    DueToForge {
        /// The delegate.
        username: Id,
        /// N.
        seconds: u32,
    },
    /// "Delegate D forged a new block at height H with N transactions".
    Forged {
        /// The delegate.
        username: Id,
        /// H.
        height: u32,
        /// N.
        transactions: u32,
    },
    /// "Delegate D sends block ID at height H of slot S again".
    SendsAgain {
        /// The delegate.
        username: Id,
        /// The block id.
        block_id: Id,
        /// H.
        height: u32,
        /// S.
        slot: i64,
    },
    /// "Delegate D will not sign: ...".
    WillNotSign {
        /// The delegate.
        username: Id,
        /// The refused slot is more than one away from the forger's clock.
        far_slot: bool,
    },
    /// "Not going to forge because delegate D has already forged on another node".
    ForgedOnAnotherNode {
        /// The delegate.
        username: Id,
    },
    /// "Delegate D already forged a block. Will not forge".
    AlreadyForgedThisRound {
        /// The delegate.
        username: Id,
    },
    /// "Not enough quorum to forge next block. Will not forge".
    NotEnoughQuorum,
    /// "Not peered with enough delegates to get quorum. Will not forge".
    NotPeeredEnough,
    /// "Skipping slot because of cold start. Will not forge".
    ColdStartSkip,
    /// "Failed to get network state from client. Will not forge".
    NoNetworkState,
    /// "Detected N distinct overheight block headers".
    OverheightHeaders {
        /// N.
        count: u32,
    },
    /// "The NetworkState height (A) and round height (B) are out of sync".
    HeightsOutOfSync {
        /// A.
        ours: u32,
        /// B.
        theirs: u32,
    },
    /// "Failed to forge new block by delegate D, because there were Nms left in the current slot
    /// (less than Mms)".
    TooLittleTime {
        /// The delegate.
        username: Id,
        /// N.
        left_ms: i64,
        /// M.
        minimum_ms: i64,
    },
    /// "Failed to forge new block by delegate D, because already in next slot".
    AlreadyNextSlot {
        /// The delegate.
        username: Id,
    },
    /// "Waiting for a responsive host".
    WaitingForHost,
    /// "Waiting for relay to become ready".
    WaitingForRelayReady,
    /// "No open socket connection to any host: ..." (debug).
    NoOpenSocket,
    /// "Request to ... failed, because of '...'": carries a relay's text, kept only as an excerpt.
    RelayRequestFailed,
    /// "... didn't respond. Trying again later".
    HostsDidNotRespond,
    /// A session error the forger logs at error level: kept only as an excerpt.
    SessionError,
    /// "Could not sync check: ...".
    SyncCheckFailed,
    /// "Broadcast block failed: ...".
    BroadcastFailed,
    /// "Failed to emit ...".
    EmitFailed,
    /// "emitEvent: unable to find any local hosts": known and harmless, never a link failure.
    NoLocalHosts,
    /// "Not attesting the relay's status: ...".
    NotAttesting {
        /// The offered slot is more than one away from the forger's clock.
        far_slot: bool,
    },
    /// "The relay gave no status to attest: ..." (debug).
    NoStatusToAttest,
    /// "The status attestations were not delivered: ..." (debug).
    AttestationsNotDelivered,
    /// "Could not get unconfirmed transactions from pool".
    PoolUnavailable,
    /// "Writing the signing record took N ms".
    SigningRecordSlow {
        /// N.
        millis: u64,
    },
    /// "delegates.json lists the key of delegate ... twice".
    KeyListedTwice,
    /// "The relay reports height H, beyond height ... that this forger's clock allows ...".
    RelayHeightBeyondClock {
        /// H.
        height: u32,
    },
}

/// One known line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogEvent {
    /// What it is.
    pub kind: LineKind,
    /// Its level.
    pub level: Level,
    /// Its time, in Unix milliseconds (the logger writes UTC).
    pub at_ms: i64,
    /// Its byte offset in its file.
    pub offset: u64,
    /// Whether it came from the rotated file rather than the current one.
    pub rotated: bool,
}

/// The time a log window covers.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Covered {
    /// The first line's time within the window.
    pub first_ms: Option<i64>,
    /// The last line's time.
    pub last_ms: Option<i64>,
    /// Whether the files reach back before the window's start, so it is covered whole.
    pub whole: bool,
}

/// A role's log in the window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogWindow {
    /// The role.
    pub role: Role,
    /// The known lines, in order.
    pub events: Vec<LogEvent>,
    /// The count of other lines.
    pub other_lines: u32,
    /// The count of lines not trusted for their time (more than 1 s backwards).
    pub untrusted_lines: u32,
    /// What the window covers.
    pub covered: Covered,
    /// The file logger's level, from the configuration, when known.
    pub file_level: Option<Level>,
    /// Excerpts of the lines that carry outside text, and of other lines at warning or worse.
    pub excerpts: Vec<Untrusted>,
    /// Whether a byte cap cut the read.
    pub capped: bool,
}

impl LogWindow {
    /// Whether the file log level writes lines of `level`; the unknown for when it does not.
    pub fn shows(&self, level: Level) -> Result<(), Unknown> {
        match self.file_level {
            Some(file) if file < level => Err(Unknown::new(source_of(self.role), Reason::LogLevel)),
            _ => Ok(()),
        }
    }
}

/// The log source of `role`.
pub fn source_of(role: Role) -> Source {
    if role == Role::Relay {
        Source::RelayLog
    } else {
        Source::ForgerLog
    }
}

/// A role's error capture (`<name>-error.log`) since its last start.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CaptureWindow {
    /// The start errors recognised.
    pub errors: Vec<StartError>,
    /// The count of other lines.
    pub other_lines: u32,
    /// Excerpts of the lines.
    pub excerpts: Vec<Untrusted>,
}

/// A role's log files: `<dir>/<stem>-current.log` and the rotated `<stem>-<date>[.<n>].log.gz`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogFiles {
    /// The log directory.
    pub dir: PathBuf,
    /// The instance's name.
    pub stem: String,
}

impl LogFiles {
    /// The current log.
    pub fn current(&self) -> PathBuf {
        self.dir.join(format!("{}-current.log", self.stem))
    }

    /// The error capture.
    pub fn error_capture(&self) -> PathBuf {
        self.dir.join(format!("{}-error.log", self.stem))
    }
}

/// What reading a role's log needs.
pub struct LogContext<'a> {
    /// The files.
    pub fs: &'a dyn DoctorFs,
    /// Where the doctor may read.
    pub roots: &'a ReadableRoots,
    /// The run's budget (32 MiB of log a run).
    pub budget: &'a Budget,
    /// The clock, for the run's deadline.
    pub clock: &'a dyn Clock,
    /// The window.
    pub window: Window,
    /// The role.
    pub role: Role,
    /// The files.
    pub files: LogFiles,
    /// The file logger's level, when known.
    pub file_level: Option<Level>,
}

/// The role's log window.
pub fn read_log(context: &LogContext<'_>) -> Result<LogWindow, Unknown> {
    Err(Unknown::new(source_of(context.role), Reason::NotBuilt))
}

/// The role's error capture from `since_ms` (its last start) on.
pub fn read_capture(
    context: &LogContext<'_>,
    since_ms: Option<i64>,
) -> Result<CaptureWindow, Unknown> {
    let _ = (context, since_ms);
    Err(Unknown::new(Source::ErrorCapture, Reason::NotBuilt))
}
