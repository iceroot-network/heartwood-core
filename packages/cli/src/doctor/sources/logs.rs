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

use std::ffi::OsString;
use std::io::Read;
use std::net::IpAddr;
use std::path::{Path, PathBuf};

use heartwood_kernel::Clock;
use heartwood_kernel::log::Level;

use crate::doctor::report::{Id, Untrusted, UntrustedSource};
use crate::doctor::seams::{DoctorFs, FsError, Tail};
use crate::doctor::sources::fs::{Readable, ReadableRoots};
use crate::doctor::sources::patterns;
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
        /// The delegate: its public key, as the signer names it.
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
        /// The delegate: its public key, as the signer names it.
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
    /// The forger's lines that list its delegates ("Loaded" and "active forging position") from
    /// its last start read before the window up to the window's start, in order: the forger
    /// writes "Loaded" once a start, so a window that starts later holds none. Empty when the
    /// forger started again inside the window, since the lines before are an earlier run's. The
    /// window's own lines are among [`LogWindow::events`].
    pub roster: Vec<LogEvent>,
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

/// The most excerpts a window keeps (the latest ones).
const MAX_EXCERPTS: usize = 32;
/// The most decompressed bytes read from a rotated file.
const MAX_INFLATED: u64 = 128 * 1024 * 1024;
/// The most physical lines after a line that opens a longer message that are read as its text.
const MAX_CONTINUED: usize = 64;

/// Whether the line's message holds text of a peer, a relay or another outside source: its
/// excerpt is kept, and an older build's log can hold lines forged inside it.
pub fn carries_outside(kind: &LineKind) -> bool {
    use LineKind as K;
    matches!(
        kind,
        K::DownloadFailed
            | K::RelayRequestFailed
            | K::SessionError
            | K::SyncCheckFailed
            | K::BroadcastFailed
            | K::EmitFailed
            | K::NodeStops
            | K::NotAttesting { .. }
            | K::NoStatusToAttest
            | K::AttestationsNotDelivered
            | K::NoOpenSocket
    )
}

/// `text` as an excerpt, cut to [`MAX_EXCERPT`] bytes.
fn excerpt(source: UntrustedSource, at_ms: Option<i64>, text: &str) -> Untrusted {
    let mut end = text.len().min(MAX_EXCERPT);
    while !text.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    let text = text.get(..end).unwrap_or_default().to_owned();
    Untrusted {
        source,
        at_ms,
        text,
    }
}

/// The days from 1970-01-01 to a civil date.
fn days(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let of_year = year - era * 400;
    let of_day = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    let of_era = of_year * 365 + of_year / 4 - of_year / 100 + of_day;
    era * 146_097 + of_era - 719_468
}

/// A line: `[YYYY-MM-DD HH:MM:ss.lll] LEVEL: message`, its time in Unix milliseconds.
fn parse_line(line: &str) -> Option<(i64, Level, &str)> {
    let (stamp, rest) = line.strip_prefix('[')?.split_once("] ")?;
    let (label, message) = rest.split_once(':')?;
    let level = Level::from_name(&label.to_ascii_lowercase())?;
    let message = message.strip_prefix(' ').unwrap_or(message);
    let bytes = stamp.as_bytes();
    let shape = bytes.len() == 23
        && [
            (4, b'-'),
            (7, b'-'),
            (10, b' '),
            (13, b':'),
            (16, b':'),
            (19, b'.'),
        ]
        .iter()
        .all(|&(at, sign)| bytes.get(at) == Some(&sign));
    let field = |from: usize, to: usize| -> Option<i64> {
        let digits = stamp.get(from..to)?;
        digits
            .bytes()
            .all(|b| b.is_ascii_digit())
            .then(|| digits.parse().ok())?
    };
    let (month, day) = (field(5, 7)?, field(8, 10)?);
    let (hour, minute, second) = (field(11, 13)?, field(14, 16)?, field(17, 19)?);
    let valid = shape && (1..=12).contains(&month) && (1..=31).contains(&day);
    if !valid || hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    let date = days(field(0, 4)?, month, day);
    let time = ((date * 24 + hour) * 60 + minute) * 60 + second;
    Some((time * 1000 + field(20, 23)?, level, message))
}

/// What the lines of one or two files gave.
#[derive(Default)]
struct Scan {
    events: Vec<LogEvent>,
    other: u32,
    untrusted: u32,
    excerpts: Vec<Untrusted>,
    first: Option<i64>,
    last: Option<i64>,
    /// The time of the first line read, whatever the window.
    earliest: Option<i64>,
    /// The forger's delegate lines before the window, since the last start line read.
    roster: Vec<LogEvent>,
    /// Whether a start line of the forger came before the window.
    restarted: bool,
}

impl Scan {
    fn keep(&mut self, excerpt: Untrusted) {
        if self.excerpts.len() >= MAX_EXCERPTS {
            self.excerpts.remove(0);
        }
        self.excerpts.push(excerpt);
    }

    /// The lines of `text`, which starts `base` bytes into its file.
    fn lines(&mut self, context: &LogContext<'_>, text: &str, base: u64, rotated: bool) {
        let (role, window) = (context.role, context.window);
        let source = UntrustedSource::Log(role);
        let (mut previous, mut offset) = (None::<i64>, base);
        let mut open: Option<(&str, usize)> = None;
        for line in text.split('\n') {
            let at = offset;
            offset = offset.saturating_add(u64::try_from(line.len()).unwrap_or(0) + 1);
            if line.is_empty() {
                continue;
            }
            if let Some((emoji, used)) = open {
                self.untrusted += 1;
                self.keep(excerpt(source, previous, line));
                // The message's last line ends in its emoji, or in the space the emoji's code
                // leaves when the logger removes codes.
                let ended = line.ends_with(emoji) || line.ends_with(' ') || used >= MAX_CONTINUED;
                open = (!ended).then_some((emoji, used + 1));
                continue;
            }
            let Some((at_ms, level, message)) = parse_line(line) else {
                self.other += 1;
                continue;
            };
            self.earliest.get_or_insert(at_ms);
            let backwards = previous.is_some_and(|before| at_ms < before - BACKWARDS_MS);
            if at_ms <= window.to_ms {
                previous = Some(at_ms);
            }
            if backwards {
                self.untrusted += 1;
                self.keep(excerpt(source, Some(at_ms), line));
                continue;
            }
            if let Some(emoji) = patterns::continues(role, level, message) {
                open = Some((emoji, 0));
            }
            let event = |kind| LogEvent {
                kind,
                level,
                at_ms,
                offset: at,
                rotated,
            };
            // Before the window, only the forger's start and the lines that list its delegates.
            if at_ms < window.from_ms {
                match roster_line(role, level, message) {
                    Some(LineKind::ForgerStarted) => {
                        (self.roster, self.restarted) = (Vec::new(), true);
                    }
                    kind => self.roster.extend(kind.map(event)),
                }
                continue;
            }
            if at_ms > window.to_ms {
                continue;
            }
            self.first.get_or_insert(at_ms);
            self.last = Some(at_ms);
            match patterns::classify(role, level, message) {
                Some(kind) => {
                    if carries_outside(&kind) {
                        self.keep(excerpt(source, Some(at_ms), message));
                    }
                    self.events.push(event(kind));
                }
                None => {
                    self.other += 1;
                    if level <= Level::Warning {
                        self.keep(excerpt(source, Some(at_ms), message));
                    }
                }
            }
        }
    }
}

/// The kind of a line before the window that the roster needs: the forger's start (its version
/// line, which comes before the start's delegate lines; its manager's line comes after them), or a
/// line that lists its delegates. A test of the line's opening spares the other lines the patterns.
fn roster_line(role: Role, level: Level, message: &str) -> Option<LineKind> {
    const OPENINGS: [&str; 3] = ["Loaded ", "Delegate", "Heartwood Core "];
    let opens = OPENINGS.iter().any(|opening| message.starts_with(opening));
    if role != Role::Forger || level != Level::Info || !opens {
        return None;
    }
    match patterns::classify(role, level, message)? {
        kind @ (LineKind::ForgerStarted
        | LineKind::DelegatesLoaded { .. }
        | LineKind::ActivePosition { .. }) => Some(kind),
        _ => None,
    }
}

/// `path` as a file the doctor may read.
fn readable(context: &LogContext<'_>, path: &Path, source: Source) -> Result<Readable, Unknown> {
    let refused = |error| Unknown::from_fs(source, &FsError::Refused(error));
    Readable::new(path, context.roots).map_err(refused)
}

/// The end of `file`, at most [`MAX_TAIL`] bytes and what the run's budget still allows, charged
/// for the bytes the file has.
fn tail_of(context: &LogContext<'_>, file: &Readable, source: Source) -> Result<Tail, Unknown> {
    context.budget.check(source, context.clock.now_ms())?;
    let size = context
        .fs
        .facts(file.path())
        .ok()
        .map(|facts| facts.own.size);
    let wanted = size.map_or(MAX_TAIL, |size| size.min(MAX_TAIL));
    let granted = context.budget.take_log_bytes(wanted);
    if granted == 0 && wanted > 0 {
        return Err(Unknown::new(source, Reason::RunBudget));
    }
    context
        .fs
        .tail(file, granted)
        .map_err(|error| Unknown::from_fs(source, &error))
}

/// The text of a tail, without the line the cut may have made partial.
fn whole_lines(tail: &Tail) -> (String, u64) {
    let text = String::from_utf8_lossy(&tail.bytes);
    if tail.start == 0 {
        return (text.into_owned(), 0);
    }
    let cut = text.find('\n').map_or(text.len(), |at| at + 1);
    let skipped = u64::try_from(cut).unwrap_or(0);
    (
        text.get(cut..).unwrap_or_default().to_owned(),
        tail.start.saturating_add(skipped),
    )
}

/// The newest rotated file among `names`, by date then by number.
fn newest_rotated(stem: &str, names: &[OsString]) -> Option<String> {
    let key = |name: &str| -> Option<(String, u32)> {
        let middle = name
            .strip_prefix(stem)?
            .strip_prefix('-')?
            .strip_suffix(".log.gz")?;
        let (date, number) = middle.split_once('.').unwrap_or((middle, "0"));
        let dated = date.len() == 10 && date.bytes().all(|b| b.is_ascii_digit() || b == b'-');
        dated.then(|| Some((date.to_owned(), number.parse().ok()?)))?
    };
    let rotated = (names.iter())
        .filter_map(|name| name.to_str())
        .filter_map(|name| Some((name, key(name)?)));
    let newest = rotated.max_by(|a, b| a.1.cmp(&b.1));
    newest.map(|(name, _)| name.to_owned())
}

/// The last [`MAX_TAIL`] bytes of what `bytes` inflate to, and how many came before them.
fn inflate_tail(bytes: &[u8]) -> Option<(Vec<u8>, u64)> {
    let (mut decoder, mut kept, mut chunk) =
        (flate2::read::GzDecoder::new(bytes), Vec::new(), [0; 65_536]);
    let (keep, mut total) = (usize::try_from(MAX_TAIL).ok()?, 0_u64);
    loop {
        let read = decoder.read(&mut chunk).ok()?;
        if read == 0 {
            break;
        }
        kept.extend_from_slice(chunk.get(..read)?);
        total = total.saturating_add(u64::try_from(read).ok()?);
        if total > MAX_INFLATED {
            return None;
        }
        if kept.len() > keep.saturating_mul(2) {
            kept.drain(..kept.len() - keep);
        }
    }
    if kept.len() > keep {
        kept.drain(..kept.len() - keep);
    }
    let before = total.saturating_sub(u64::try_from(kept.len()).ok()?);
    Some((kept, before))
}

/// The newest rotated file of the role, scanned into `scan`.
fn read_rotated(context: &LogContext<'_>, scan: &mut Scan) -> Option<()> {
    let source = source_of(context.role);
    let dir = readable(context, &context.files.dir, source).ok()?;
    let names = context.fs.list(&dir).ok()?;
    let name = newest_rotated(&context.files.stem, &names)?;
    let file = readable(context, &context.files.dir.join(name), source).ok()?;
    let size = context
        .fs
        .facts(file.path())
        .ok()
        .map(|facts| facts.own.size);
    let granted = context
        .budget
        .take_log_bytes(size.map_or(MAX_TAIL, |size| size.min(MAX_TAIL)));
    let compressed = context.fs.read(&file, granted).ok()?;
    let (bytes, before) = inflate_tail(&compressed)?;
    let tail = Tail {
        bytes,
        start: before,
        len: 0,
    };
    let (text, base) = whole_lines(&tail);
    scan.lines(context, &text, base, true);
    Some(())
}

/// The role's log window.
pub fn read_log(context: &LogContext<'_>) -> Result<LogWindow, Unknown> {
    let source = source_of(context.role);
    let file = readable(context, &context.files.current(), source)?;
    let tail = tail_of(context, &file, source)?;
    let (text, base) = whole_lines(&tail);
    let mut current = Scan::default();
    current.lines(context, &text, base, false);
    let from = context.window.from_ms;
    let mut older = Scan::default();
    let before_window = current.earliest.is_none_or(|first| first > from);
    if tail.start == 0 && before_window {
        read_rotated(context, &mut older);
    }
    // Whole only when a line read is from the window's start or before: a log that starts inside
    // the window (a new node, a cleared log, a rotated file left unread) covers less.
    let earliest = older.earliest.or(current.earliest);
    let covered = Covered {
        first_ms: older.first.or(current.first),
        last_ms: current.last.or(older.last),
        whole: earliest.is_some_and(|first| first <= from),
    };
    let mut excerpts = older.excerpts;
    excerpts.extend(current.excerpts);
    let surplus = excerpts.len().saturating_sub(MAX_EXCERPTS);
    excerpts.drain(..surplus);
    older.events.extend(current.events);
    // A start inside the window makes the lines before it an earlier run's.
    let started = |event: &LogEvent| event.kind == LineKind::ForgerStarted;
    let roster = match (older.events.iter().any(started), current.restarted) {
        (true, _) => Vec::new(),
        (false, true) => current.roster,
        (false, false) => [older.roster, current.roster].concat(),
    };
    Ok(LogWindow {
        role: context.role,
        events: older.events,
        other_lines: older.other + current.other,
        untrusted_lines: older.untrusted + current.untrusted,
        covered,
        file_level: context.file_level,
        excerpts,
        capped: tail.start > 0,
        roster,
    })
}

/// The role's error capture from `since_ms` (its last start) on. The capture has no times: a
/// file last written before the start holds nothing since it, and otherwise its end is read
/// whole, so a start error of an earlier start may still be in it.
pub fn read_capture(
    context: &LogContext<'_>,
    since_ms: Option<i64>,
) -> Result<CaptureWindow, Unknown> {
    let source = Source::ErrorCapture;
    let file = readable(context, &context.files.error_capture(), source)?;
    let facts = context.fs.facts(file.path()).ok();
    let stale = since_ms
        .zip(facts)
        .is_some_and(|(since, facts)| facts.own.modified_ms < since);
    let mut window = CaptureWindow::default();
    if stale {
        return Ok(window);
    }
    let tail = tail_of(context, &file, source)?;
    let (text, _) = whole_lines(&tail);
    let origin = UntrustedSource::ErrorCapture(context.role);
    for line in text.lines().filter(|line| !line.is_empty()) {
        match patterns::classify(context.role, Level::Error, line) {
            Some(LineKind::StartError(error)) => {
                if window.errors.last() != Some(&error) {
                    window.errors.push(error);
                }
            }
            _ => {
                window.other_lines += 1;
                if window.excerpts.len() >= MAX_EXCERPTS {
                    window.excerpts.remove(0);
                }
                window.excerpts.push(excerpt(origin, None, line));
            }
        }
    }
    Ok(window)
}
