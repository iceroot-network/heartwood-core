//! The log window: line kinds against a sample of each template, line integrity, lines forged
//! inside a peer's or a relay's text (raw, as an older build wrote them, and escaped), rotated
//! files across midnight, the read caps, and the error capture.

mod doctor_support;

use std::cell::RefCell;
use std::ffi::OsString;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use flate2::Compression;
use flate2::write::GzEncoder;
use heartwood_cli::Role;
use heartwood_cli::doctor::report::UntrustedSource;
use heartwood_cli::doctor::seams::{DoctorFs, FileFacts, FileKind, FsError, FsStats, Meta, Tail};
use heartwood_cli::doctor::sources::fs::{Readable, ReadableRoots};
use heartwood_cli::doctor::sources::logs::{
    LineKind, LogContext, LogFiles, LogWindow, MAX_EXCERPT, MAX_RUN, MAX_TAIL, StartError,
    read_capture, read_log,
};
use heartwood_cli::doctor::sources::patterns::{TEMPLATES, classify};
use heartwood_cli::doctor::view::{Budget, Reason, Source, Unknown, Window};
use heartwood_kernel::clock::ManualClock;
use heartwood_kernel::log::Level;

use doctor_support::{FakeFs, NOW_MS, scope};

const SAMPLES: &str = include_str!("fixtures/doctor/logs/samples.tsv");
const FORGED_DOWNLOAD_RAW: &str = include_str!("fixtures/doctor/logs/forged-download-raw.log");
const FORGED_DOWNLOAD_ESCAPED: &str =
    include_str!("fixtures/doctor/logs/forged-download-escaped.log");
const FORGED_PAYLOAD_RAW: &str = include_str!("fixtures/doctor/logs/forged-payload-raw.log");
const FORGED_PAYLOAD_ESCAPED: &str =
    include_str!("fixtures/doctor/logs/forged-payload-escaped.log");
const BACKWARDS: &str = include_str!("fixtures/doctor/logs/backwards.log");
const ROTATION_PREVIOUS: &str = include_str!("fixtures/doctor/logs/rotation-previous.log");
const ROTATION_CURRENT: &str = include_str!("fixtures/doctor/logs/rotation-current.log");
const FORGED_REQUEST_RAW: &str = include_str!("fixtures/doctor/logs/forged-request-raw.log");
const FORGED_REQUEST_ESCAPED: &str =
    include_str!("fixtures/doctor/logs/forged-request-escaped.log");
const CAPTURED_RELAY: &str = include_str!("fixtures/doctor/logs/captured-relay.log");
const CAPTURED_FORGER: &str = include_str!("fixtures/doctor/logs/captured-forger.log");

/// 2026-10-02 00:00:00 UTC, in Unix milliseconds.
const DAY_MS: i64 = NOW_MS - 50_591_000;
const MIB: usize = 1024 * 1024;

/// A sample line of the table.
struct Sample {
    role: Role,
    level: Level,
    kind: &'static str,
    message: &'static str,
}

impl Sample {
    /// The message as the node writes it: with the trailing emoji of its template.
    fn line(&self) -> String {
        let emoji = TEMPLATES
            .iter()
            .filter(|t| (t.role, t.level, t.kind) == (self.role, self.level, self.kind))
            .find_map(|t| t.emoji);
        match emoji {
            Some(emoji) => format!("{} {emoji}", self.message),
            None => self.message.to_owned(),
        }
    }
}

fn samples() -> Vec<Sample> {
    let row = |line: &'static str| {
        let mut cells = line.splitn(4, '\t');
        let (role, level) = (cells.next()?, cells.next()?);
        Some(Sample {
            role: if role == "relay" {
                Role::Relay
            } else {
                Role::Forger
            },
            level: Level::from_name(level)?,
            kind: cells.next()?,
            message: cells.next()?,
        })
    };
    let lines = SAMPLES
        .lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty());
    lines
        .map(|line| row(line).unwrap_or_else(|| panic!("a sample row is not valid: {line}")))
        .collect()
}

/// The kind's name as the table spells it, from the typed kind's `Debug`.
fn flat(kind: &LineKind) -> String {
    let debug = format!("{kind:?}");
    let head = debug.split('{').next().unwrap_or_default();
    head.chars().filter(|c| c.is_alphanumeric()).collect()
}

/// Whether the sample's kind is the one `classified` is.
fn names(expected: &str, classified: &LineKind) -> bool {
    let flat = flat(classified);
    match classified {
        LineKind::ActivePosition { entered, .. } => {
            let side = if *entered { "Entered" } else { "Left" };
            expected == format!("{flat}{side}")
        }
        _ => expected == flat,
    }
}

fn in_order(pieces: &[&str], text: &str) -> bool {
    let mut rest = text;
    for piece in pieces {
        match rest.split_once(piece) {
            Some((_, after)) => rest = after,
            None => return false,
        }
    }
    true
}

#[test]
fn every_template_has_a_sample_that_it_matches_and_that_classifies_to_its_kind() {
    let samples = samples();
    for template in TEMPLATES {
        let matching = samples.iter().filter(|s| {
            (s.role, s.level, s.kind) == (template.role, template.level, template.kind)
                && in_order(template.pieces, s.message)
        });
        assert!(matching.count() > 0, "no sample of {template:?}");
    }
    for sample in &samples {
        let classified = classify(sample.role, sample.level, &sample.line());
        let classified = classified.unwrap_or_else(|| panic!("{}: not classified", sample.line()));
        assert!(
            names(sample.kind, &classified),
            "{}: {classified:?}",
            sample.line()
        );
    }
}

#[test]
fn a_line_without_its_emoji_or_at_another_level_or_role_is_not_a_known_kind() {
    for sample in samples() {
        let line = sample.line();
        let emoji_less = line != sample.message;
        if emoji_less {
            assert_eq!(
                classify(sample.role, sample.level, sample.message),
                None,
                "{line}"
            );
        }
        let other = if sample.role == Role::Relay {
            Role::Forger
        } else {
            Role::Relay
        };
        let shared = TEMPLATES
            .iter()
            .any(|t| (t.role, t.level, t.kind) == (other, sample.level, sample.kind));
        let same = classify(other, sample.level, &line);
        assert!(
            shared || same.is_none_or(|k| !names(sample.kind, &k)),
            "{line} as {other:?}"
        );
    }
}

#[test]
fn a_capture_that_fails_its_grammar_makes_the_line_another_one() {
    let sync = |message: &str, role, level| classify(role, level, message);
    let forger = Role::Forger;
    // A name is lowercase only; a height is digits; a block id is 64 hex digits.
    // A name that fails it is left out of the names; the line is still known.
    let name = "Delegate Alice is now in an active forging position 🎉";
    assert_eq!(
        sync(name, forger, Level::Info),
        Some(LineKind::ActivePosition {
            entered: true,
            names: Vec::new()
        })
    );
    let height = "Delegate alice forged a new block at height x with 3 transactions 🔱";
    assert_eq!(sync(height, forger, Level::Info), None);
    let id = "Delegate alice sends block abc at height 1,301 of slot 77 again";
    assert_eq!(sync(id, forger, Level::Info), None);
    let relay = Role::Relay;
    assert_eq!(
        sync("Blockchain 100% in sync 💯 💯", relay, Level::Info),
        None
    );
    assert_eq!(sync("", relay, Level::Info), None);
    assert_eq!(
        sync(
            "Removing many blocks. Reset to height 9 ⚠️",
            relay,
            Level::Info
        ),
        None
    );
}

/// A rig: the fake file system, a clock at [`NOW_MS`], the run's budget and the readable roots of
/// the node under `/nodes/node1`.
struct Rig {
    fs: FakeFs,
    clock: ManualClock,
    budget: Budget,
    roots: ReadableRoots,
    window: Window,
}

fn facts(path: &Path, size: u64, modified_ms: i64) -> FileFacts {
    let own = Meta {
        kind: FileKind::File,
        mode: 0o644,
        uid: 1000,
        gid: 1000,
        size,
        dev: 1,
        ino: 100 + size,
        nlink: 1,
        modified_ms,
    };
    FileFacts {
        path: path.to_path_buf(),
        own,
        target: None,
        link_target: None,
    }
}

fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(bytes).expect("gzip a buffer");
    encoder.finish().expect("finish a gzip buffer")
}

impl Rig {
    fn new() -> Rig {
        let paths = scope("/nodes/node1").paths;
        Rig {
            fs: FakeFs::default(),
            clock: ManualClock::new(NOW_MS),
            budget: Budget::new(NOW_MS, std::time::Duration::from_secs(5)),
            roots: ReadableRoots::for_paths(&paths),
            window: Window {
                from_ms: NOW_MS - 3_600_000,
                to_ms: NOW_MS,
            },
        }
    }

    fn put(&mut self, name: &str, bytes: impl Into<Vec<u8>>, modified_ms: i64) {
        let path = PathBuf::from("/nodes/node1/log").join(name);
        let bytes = bytes.into();
        let size = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        self.fs
            .facts
            .insert(path.clone(), Ok(facts(&path, size, modified_ms)));
        self.fs.contents.insert(path, bytes);
    }

    fn files(stem: &str) -> LogFiles {
        LogFiles {
            dir: PathBuf::from("/nodes/node1/log"),
            stem: stem.to_owned(),
        }
    }

    fn context<'a>(&'a self, fs: &'a dyn DoctorFs, role: Role, stem: &str) -> LogContext<'a> {
        LogContext {
            fs,
            roots: &self.roots,
            budget: &self.budget,
            clock: &self.clock,
            window: self.window,
            role,
            files: Rig::files(stem),
            file_level: None,
        }
    }

    fn log(&self, role: Role) -> Result<LogWindow, Unknown> {
        read_log(&self.context(&self.fs, role, "node1"))
    }
}

/// The file system with every `tail` and `read` it serves recorded.
struct Counting<'a> {
    inner: &'a FakeFs,
    tails: RefCell<Vec<u64>>,
    opened: RefCell<Vec<PathBuf>>,
}

impl DoctorFs for Counting<'_> {
    fn facts(&self, path: &Path) -> io::Result<FileFacts> {
        self.inner.facts(path)
    }
    fn read(&self, file: &Readable, max: u64) -> Result<Vec<u8>, FsError> {
        self.opened.borrow_mut().push(file.path().to_path_buf());
        self.inner.read(file, max)
    }
    fn tail(&self, file: &Readable, max: u64) -> Result<Tail, FsError> {
        self.opened.borrow_mut().push(file.path().to_path_buf());
        self.tails.borrow_mut().push(max);
        self.inner.tail(file, max)
    }
    fn list(&self, dir: &Readable) -> Result<Vec<OsString>, FsError> {
        self.inner.list(dir)
    }
    fn stats(&self, path: &Path) -> Result<FsStats, FsError> {
        self.inner.stats(path)
    }
}

fn kinds(window: &LogWindow) -> Vec<&LineKind> {
    window.events.iter().map(|event| &event.kind).collect()
}

const IN_SYNC: &str = "[2026-10-02 13:10:00.000] INFO: Blockchain 100% in sync 💯\n";

#[test]
fn a_file_of_one_sample_of_each_kind_gives_one_event_of_each_kind_in_order() {
    let samples = samples();
    for role in [Role::Relay, Role::Forger] {
        let mine: Vec<&Sample> = samples.iter().filter(|s| s.role == role).collect();
        let mut text = String::new();
        for (i, sample) in mine.iter().enumerate() {
            let level = sample.level.name().to_uppercase();
            let (minute, second) = (10 + i / 60, i % 60);
            let stamp = format!("2026-10-02 13:{minute:02}:{second:02}.000");
            text.push_str(&format!("[{stamp}] {level}: {}\n", sample.line()));
        }
        let mut rig = Rig::new();
        rig.put("node1-current.log", text, NOW_MS - 1000);
        let window = rig.log(role).expect("a window");
        assert_eq!(window.other_lines, 0, "{role:?}");
        assert_eq!(window.untrusted_lines, 0, "{role:?}");
        assert_eq!(window.events.len(), mine.len(), "{role:?}");
        for (event, sample) in window.events.iter().zip(&mine) {
            assert!(
                names(sample.kind, &event.kind),
                "{}: {event:?}",
                sample.line()
            );
            assert_eq!(event.level, sample.level, "{}", sample.line());
            assert!(!event.rotated);
        }
        // The first line is at 13:10, after the window's start at 13:03:11.
        assert!(!window.covered.whole, "{role:?}");
        assert_eq!(
            window.covered.first_ms,
            Some(DAY_MS + 13 * 3_600_000 + 600_000)
        );
    }
}

#[test]
fn a_line_has_its_time_in_unix_milliseconds_in_utc_leap_days_included() {
    let mut rig = Rig::new();
    let line = |stamp: &str| format!("[{stamp}] INFO: Blockchain 100% in sync 💯\n");
    let text = [
        line("2026-10-02 14:03:11.000"),
        line("2028-02-29 23:59:59.999"),
    ];
    rig.put("node1-current.log", text.concat(), NOW_MS);
    rig.window = Window {
        from_ms: NOW_MS - 1000,
        to_ms: 1_835_481_600_000,
    };
    let window = rig.log(Role::Relay).expect("a window");
    // The second line is more than a second before the last of the window's end, not before its
    // predecessor: it is in the window, and its time is the leap day's last millisecond.
    let times: Vec<i64> = window.events.iter().map(|e| e.at_ms).collect();
    assert_eq!(times, [NOW_MS, 1_835_481_599_999]);
}

#[test]
fn lines_that_are_not_log_lines_are_counted_and_never_events() {
    let mut rig = Rig::new();
    let mut bytes = Vec::new();
    let junk = [
        "not a line at all",
        "[2026-13-02 13:10:00.000] INFO: Blockchain 100% in sync 💯",
        "[2026-10-02 25:10:00.000] INFO: Blockchain 100% in sync 💯",
        "[2026-10-02 13:10:00.000] LOUD: Blockchain 100% in sync 💯",
        "[2026-10-02 13:10:00] INFO: Blockchain 100% in sync 💯",
        "[2026-10-02 13:10:00.000]INFO: Blockchain 100% in sync 💯",
        "[2026-10-02 13:10:00.000] INFO:",
        "[2026-10-02 13:10:00.000] INFO: Blockchain 100% in sync",
        "[2026-1a-02 13:10:00.000] INFO: Blockchain 100% in sync 💯",
        "[",
        "]",
        "\t\t",
    ];
    for line in junk {
        bytes.extend_from_slice(line.as_bytes());
        bytes.push(b'\n');
    }
    bytes.extend_from_slice(b"[2026-10-02 13:10:00.000] INFO: \xff\xfe\xfd broken utf-8 \n");
    bytes.extend_from_slice(&[0_u8; 64]);
    bytes.extend_from_slice(b"\n\n\n");
    bytes.extend_from_slice(IN_SYNC.as_bytes());
    rig.put("node1-current.log", bytes, NOW_MS);
    let window = rig.log(Role::Relay).expect("a window");
    assert_eq!(kinds(&window), [&LineKind::InSync]);
    // A line of a template without a capture that lacks its emoji holds no outside text, so it
    // opens no run of untrusted lines: every non-empty line but the last is another line.
    assert_eq!(window.untrusted_lines, 0);
    assert_eq!(
        window.other_lines,
        u32::try_from(junk.len() + 2).unwrap_or(u32::MAX)
    );
}

#[test]
fn a_file_of_random_bytes_gives_a_window_and_no_panic() {
    let mut state = 0x2545_F491_4F6C_DD1D_u64;
    let mut bytes = Vec::new();
    for _ in 0..200_000 {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        bytes.push(state.to_le_bytes()[3]);
    }
    let mut rig = Rig::new();
    rig.put("node1-current.log", bytes.clone(), NOW_MS);
    rig.put("node1-error.log", bytes, NOW_MS);
    for role in [Role::Relay, Role::Forger] {
        let window = rig.log(role).expect("a window");
        assert!(window.events.is_empty());
        assert!(read_capture(&rig.context(&rig.fs, role, "node1"), None).is_ok());
    }
}

#[test]
fn lines_outside_the_window_are_not_events_and_the_window_covered_is_told() {
    let mut rig = Rig::new();
    let line = |time: &str| format!("[2026-10-02 {time}] INFO: Blockchain 100% in sync 💯\n");
    // The window is 13:03:11 to 14:03:11.
    let text = [
        line("12:00:00.000"),
        line("13:30:00.000"),
        line("13:45:00.000"),
        line("15:00:00.000"),
    ];
    rig.put("node1-current.log", text.concat(), NOW_MS);
    let window = rig.log(Role::Relay).expect("a window");
    assert_eq!(window.events.len(), 2);
    assert_eq!(
        window.covered.first_ms,
        Some(DAY_MS + 13 * 3_600_000 + 30 * 60_000)
    );
    assert_eq!(
        window.covered.last_ms,
        Some(DAY_MS + 13 * 3_600_000 + 45 * 60_000)
    );
    // The file has lines from before the window: it covers the window's start.
    assert!(window.covered.whole);
    assert!(!window.capped);
}

#[test]
fn a_log_that_starts_inside_the_window_covers_less_than_the_window() {
    let mut rig = Rig::new();
    rig.put("node1-current.log", IN_SYNC, NOW_MS);
    let window = rig.log(Role::Relay).expect("a window");
    // A new node or a cleared log: what was read starts at 13:10, not at the window's start.
    assert!(!window.covered.whole);
    assert_eq!(
        window.covered.first_ms,
        Some(DAY_MS + 13 * 3_600_000 + 600_000)
    );
    // With an older rotated file that is not read (the window's start lies before this file),
    // whole is told from what was read.
    rig.put(
        "node1-2026-10-02.log.gz",
        gzip(b"[2026-10-02 12:00:00.000] INFO: not a known line\n"),
        NOW_MS,
    );
    let window = rig.log(Role::Relay).expect("a window");
    assert_eq!(window.events.len(), 1);
}

#[test]
fn a_line_more_than_a_second_before_the_line_above_it_is_untrusted_and_no_event() {
    let mut rig = Rig::new();
    rig.window = Window {
        from_ms: DAY_MS + 12 * 3_600_000,
        to_ms: DAY_MS + 14 * 3_600_000,
    };
    rig.put("node1-current.log", BACKWARDS, NOW_MS);
    let window = rig.log(Role::Relay).expect("a window");
    // 10.000, 10.500, then 9.500 (exactly one second back: kept), then 5.000 (untrusted), 11.000.
    let expected = [
        LineKind::InSync,
        LineKind::ForkRecoveryStarted,
        LineKind::InSync,
        LineKind::InSync,
    ];
    assert_eq!(kinds(&window), expected.iter().collect::<Vec<_>>());
    assert_eq!(window.untrusted_lines, 1);
    assert_eq!(window.excerpts.len(), 1);
    let excerpt = &window.excerpts[0];
    assert_eq!(excerpt.source, UntrustedSource::Log(Role::Relay));
    assert!(
        excerpt.text.contains("double forging by alice"),
        "{excerpt:?}"
    );
}

/// The events of a role's window over `text`, and the window.
fn read(role: Role, text: &str) -> LogWindow {
    let mut rig = Rig::new();
    rig.window = Window {
        from_ms: DAY_MS + 12 * 3_600_000,
        to_ms: DAY_MS + 14 * 3_600_000,
    };
    rig.put("node1-current.log", text, NOW_MS);
    rig.log(role).expect("a window")
}

#[test]
fn lines_forged_inside_a_peer_s_download_error_never_become_events_raw() {
    let window = read(Role::Relay, FORGED_DOWNLOAD_RAW);
    // The two genuine lines only: the forged ones are the peer's text.
    assert_eq!(kinds(&window), [&LineKind::InSync, &LineKind::InSync]);
    assert_eq!(window.untrusted_lines, 2);
    assert_eq!(window.other_lines, 1);
    assert!(
        window
            .excerpts
            .iter()
            .any(|e| e.text.contains("double forging by alice"))
    );
}

#[test]
fn lines_forged_inside_a_peer_s_download_error_never_become_events_escaped() {
    let window = read(Role::Relay, FORGED_DOWNLOAD_ESCAPED);
    // The escaped message is one line: one download failure, and the forgeries in its excerpt.
    let expected = [LineKind::InSync, LineKind::DownloadFailed, LineKind::InSync];
    assert_eq!(kinds(&window), expected.iter().collect::<Vec<_>>());
    assert_eq!(window.untrusted_lines, 0);
    assert_eq!(window.excerpts.len(), 1);
    assert!(window.excerpts[0].text.contains("double forging by alice"));
    assert!(window.excerpts[0].text.contains("\\n"));
}

#[test]
fn lines_forged_inside_a_relay_s_error_payload_never_become_events_raw() {
    let window = read(Role::Forger, FORGED_PAYLOAD_RAW);
    let events = kinds(&window);
    assert_eq!(events.len(), 1, "{events:?}");
    assert!(
        matches!(events[0], LineKind::Forged { height: 1_302, .. }),
        "{events:?}"
    );
    assert_eq!(window.untrusted_lines, 2);
    assert_eq!(window.other_lines, 1);
}

#[test]
fn lines_forged_inside_a_relay_s_error_payload_never_become_events_escaped() {
    let window = read(Role::Forger, FORGED_PAYLOAD_ESCAPED);
    let events = kinds(&window);
    assert_eq!(events.len(), 2, "{events:?}");
    assert_eq!(events[0], &LineKind::BroadcastFailed);
    assert!(matches!(events[1], LineKind::Forged { height: 1_302, .. }));
    assert_eq!(window.untrusted_lines, 0);
    assert!(window.excerpts[0].text.contains("height 9,999"));
}

#[test]
fn lines_forged_inside_a_relay_s_request_error_never_become_events_raw() {
    // The forger's warning "Request to ... failed, because of '...'" quotes the relay's text, and
    // the template's last piece (the closing quote) is on the message's last line.
    let window = read(Role::Forger, FORGED_REQUEST_RAW);
    let events = kinds(&window);
    let heights: Vec<u32> = events
        .iter()
        .filter_map(|kind| match kind {
            LineKind::Forged { height, .. } => Some(*height),
            _ => None,
        })
        .collect();
    assert_eq!(heights, [1_301, 1_302], "{events:?}");
    assert_eq!(events.len(), 2, "{events:?}");
    assert_eq!(window.untrusted_lines, 3);
    assert_eq!(window.other_lines, 1);
}

#[test]
fn lines_forged_inside_a_relay_s_request_error_never_become_events_escaped() {
    let window = read(Role::Forger, FORGED_REQUEST_ESCAPED);
    let events = kinds(&window);
    assert_eq!(events.len(), 3, "{events:?}");
    assert_eq!(events[1], &LineKind::RelayRequestFailed);
    assert_eq!(window.untrusted_lines, 0);
    assert!(
        window.excerpts[0]
            .text
            .contains("already forged on another node")
    );
}

#[test]
fn lines_as_a_devnet_node_wrote_them_are_known_kinds() {
    // The messages are as the node wrote them on a devnet (times moved into the window,
    // addresses changed to documentation ones).
    let relay = read(Role::Relay, CAPTURED_RELAY);
    let names: Vec<String> = relay.events.iter().map(|e| flat(&e.kind)).collect();
    assert_eq!(
        names,
        [
            "RelayStarted",
            "MedianNetworkHeight",
            "InSync",
            "FallingBackToSeeds",
            "BlockReceived",
            "NoBlocksFromAnyPeer",
            "NetworkMissingBlocks",
            "DownloadFailed",
            "ForkDetected",
            "BlocksRemoved",
        ]
    );
    assert_eq!((relay.other_lines, relay.untrusted_lines), (0, 0));
    assert!(relay.events.iter().any(|e| e.kind
        == LineKind::BlockReceived {
            username: user("genesis_41"),
            rank: 16,
            height: 2
        }));
    let forger = read(Role::Forger, CAPTURED_FORGER);
    let names: Vec<String> = forger.events.iter().map(|e| flat(&e.kind)).collect();
    assert_eq!(
        names,
        [
            "ForgerStarted",
            "WaitingForRelayReady",
            "ForgerManagerStarted",
            "NoLocalHosts",
            "DelegatesLoaded",
            "DueToForge",
            "Forged",
            "SigningRecordSlow",
            "AttestationsNotDelivered",
            "NoStatusToAttest",
            "NotPeeredEnough",
            "ActivePosition",
            "ActivePosition",
            "ForgedOnAnotherNode",
        ]
    );
    assert_eq!((forger.other_lines, forger.untrusted_lines), (0, 0));
    let loaded = forger.events.iter().find_map(|e| match &e.kind {
        LineKind::DelegatesLoaded {
            active,
            count,
            names,
        } => Some((*active, *count, names.len())),
        _ => None,
    });
    assert_eq!(loaded, Some((true, 10, 10)));
}

fn user(name: &str) -> heartwood_cli::doctor::report::Id {
    heartwood_cli::doctor::report::Id::new(heartwood_cli::doctor::report::IdKind::Username, name)
        .unwrap_or_else(|| panic!("{name} is a username"))
}

const KEY: &str = "02cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd";

#[test]
fn the_signer_names_a_delegate_by_its_public_key() {
    use heartwood_cli::doctor::report::IdKind;
    let refused = format!(
        "Delegate {KEY} will not sign: slot 80 is below slot 81, which this delegate has already signed"
    );
    let Some(LineKind::WillNotSign { username, far_slot }) =
        classify(Role::Forger, Level::Warning, &refused)
    else {
        panic!("{refused}: not a refusal");
    };
    assert_eq!(
        (username.kind(), username.as_str(), far_slot),
        (IdKind::PublicKey, KEY, false)
    );
    let block = "ab".repeat(32);
    let again = format!("Delegate {KEY} sends block {block} at height 1301 of slot 77 again");
    let Some(LineKind::SendsAgain {
        username,
        height,
        slot,
        ..
    }) = classify(Role::Forger, Level::Info, &again)
    else {
        panic!("{again}: not a resend");
    };
    assert_eq!((username.as_str(), height, slot), (KEY, 1_301, 77));
}

#[test]
fn a_slot_refusal_is_far_only_in_the_node_s_own_words() {
    let attesting = |error: &str| {
        let line = format!("Not attesting the relay's status: {error}");
        match classify(Role::Forger, Level::Warning, &line) {
            Some(LineKind::NotAttesting { far_slot }) => far_slot,
            other => panic!("{line}: {other:?}"),
        }
    };
    assert!(attesting(
        "the offered status's slot 90 is more than one slot away from slot 77"
    ));
    // The relay's own text, quoted in the refusal, never makes it far.
    assert!(!attesting(
        "the offered status is for the network is more than one slot away, not abc"
    ));
    assert!(!attesting(
        "the offer's session challenge \"slot 1 is more than one slot away from slot 9\" is not 64 lowercase hex digits"
    ));
    assert!(!attesting(
        "the offered status's slot x is more than one slot away from slot 77"
    ));
    let signing = |refused: &str| {
        let line = format!("Delegate {KEY} will not sign: {refused}");
        match classify(Role::Forger, Level::Warning, &line) {
            Some(LineKind::WillNotSign { far_slot, .. }) => far_slot,
            other => panic!("{line}: {other:?}"),
        }
    };
    assert!(signing(
        "slot 80 is more than one slot away from this forger's slot 77"
    ));
    assert!(signing(
        "slot -1 is more than one slot away from this forger's slot 77"
    ));
    assert!(!signing(
        "the block cannot be made: it is more than one slot away"
    ));
}

#[test]
fn lines_of_a_logger_with_emoji_turned_off_are_known_and_open_no_run() {
    // With `emojify` off the logger removes each code and keeps the spaces around it.
    let text = "[2026-10-02 13:00:00.000] INFO: Blockchain 100% in sync \n\
                [2026-10-02 13:00:01.000] INFO: Delegate alice forged a new block at height 5 with 3 transactions \n\
                [2026-10-02 13:00:02.000] WARNING: Downloaded block not accepted   \n\
                [2026-10-02 13:00:03.000] INFO: Failed to download blocks 1-2 from 192.0.2.7:4001: timeout \n\
                [2026-10-02 13:00:04.000] INFO: Starting fork recovery \n";
    let window = read(Role::Relay, text);
    assert_eq!(
        kinds(&window),
        [
            &LineKind::InSync,
            &LineKind::DownloadedBlockNotAccepted,
            &LineKind::DownloadFailed,
            &LineKind::ForkRecoveryStarted
        ]
    );
    assert_eq!((window.untrusted_lines, window.other_lines), (0, 1));
    let forger = read(Role::Forger, text);
    assert!(
        matches!(forger.events.as_slice(), [e] if matches!(e.kind, LineKind::Forged { height: 5, transactions: 3, .. })),
        "{:?}",
        forger.events
    );
    assert_eq!(forger.untrusted_lines, 0);
}

#[test]
fn a_forgery_that_never_ends_swallows_no_more_than_a_bounded_run_of_lines() {
    let mut text = String::from(
        "[2026-10-02 13:00:00.000] INFO: Failed to download blocks 1,201-1,300 from 192.0.2.7:4001: x\n",
    );
    for _ in 0..500 {
        text.push_str("filler\n");
    }
    text.push_str(IN_SYNC);
    let window = read(Role::Relay, &text);
    // The genuine line after the run is read again.
    assert_eq!(kinds(&window), [&LineKind::InSync]);
    // The line that opened it and the bounded run after it.
    assert!(window.untrusted_lines <= 65, "{}", window.untrusted_lines);
}

#[test]
fn excerpts_are_capped_at_512_bytes_at_a_character_and_the_latest_are_kept() {
    let long = "é".repeat(2000);
    let mut text = format!(
        "[2026-10-02 13:00:00.000] INFO: Failed to download blocks 1,201-1,300 from 192.0.2.7:4001: {long} ‼️\n"
    );
    for i in 0..100 {
        text.push_str(&format!(
            "[2026-10-02 13:{:02}:{:02}.000] WARNING: odd thing number {i}\n",
            1 + i / 60,
            i % 60
        ));
    }
    let window = read(Role::Relay, &text);
    assert!(window.excerpts.len() <= 32, "{}", window.excerpts.len());
    assert!(window.excerpts.iter().all(|e| e.text.len() <= MAX_EXCERPT));
    // The oldest were dropped for the latest.
    let last = window.excerpts.last().expect("an excerpt");
    assert!(last.text.contains("number 99"), "{last:?}");
    let first = read(Role::Relay, text.lines().next().unwrap_or_default());
    let excerpt = &first.excerpts[0];
    assert!(excerpt.text.len() <= MAX_EXCERPT);
    assert!(excerpt.text.chars().all(|c| c != '\u{fffd}'));
}

#[test]
fn debug_lines_are_read_when_present_and_are_never_required() {
    let debug =
        "[2026-10-02 13:00:00.000] DEBUG: No open socket connection to any host: 192.0.2.7 ‼️\n";
    let with = read(Role::Forger, debug);
    assert_eq!(kinds(&with), [&LineKind::NoOpenSocket]);
    assert_eq!(with.excerpts.len(), 1);
    let without = read(
        Role::Forger,
        "[2026-10-02 13:00:00.000] INFO: Forger Manager started 🔨\n",
    );
    assert_eq!(kinds(&without), [&LineKind::ForgerManagerStarted]);
    // The level the file logger writes is passed on, so a check can tell hidden from absent.
    let mut rig = Rig::new();
    rig.put("node1-current.log", debug, NOW_MS);
    let mut context = rig.context(&rig.fs, Role::Forger, "node1");
    context.file_level = Some(Level::Info);
    let window = read_log(&context).expect("a window");
    assert_eq!(
        window.shows(Level::Debug).map_err(|u| u.reason),
        Err(Reason::LogLevel)
    );
    assert_eq!(window.shows(Level::Info), Ok(()));
}

#[test]
fn a_missing_or_refused_log_is_unknown_of_the_role_s_source() {
    let rig = Rig::new();
    let missing = rig.log(Role::Relay).map(|_| ()).unwrap_err();
    assert_eq!(
        (missing.source, missing.reason),
        (Source::RelayLog, Reason::Missing)
    );
    let missing = rig.log(Role::Forger).map(|_| ()).unwrap_err();
    assert_eq!(
        (missing.source, missing.reason),
        (Source::ForgerLog, Reason::Missing)
    );
    let mut outside = Rig::new();
    outside.put("node1-current.log", IN_SYNC, NOW_MS);
    let mut context = outside.context(&outside.fs, Role::Relay, "node1");
    context.files = LogFiles {
        dir: PathBuf::from("/etc"),
        stem: "node1".to_owned(),
    };
    assert_eq!(
        read_log(&context).map(|_| ()).unwrap_err().reason,
        Reason::Denied
    );
}

#[test]
fn a_run_out_of_time_reads_no_log() {
    let rig = Rig::new();
    rig.clock.set(NOW_MS + 10_000);
    assert_eq!(
        rig.log(Role::Relay).map(|_| ()).unwrap_err().reason,
        Reason::RunBudget
    );
}

/// The window from 23:30 on 2026-10-02 to 00:30 on 2026-10-03.
fn across_midnight(rig: &mut Rig) {
    let midnight = DAY_MS + 24 * 3_600_000;
    rig.window = Window {
        from_ms: midnight - 30 * 60_000,
        to_ms: midnight + 30 * 60_000,
    };
    rig.clock.set(midnight + 30 * 60_000);
    rig.budget = Budget::new(midnight + 30 * 60_000, std::time::Duration::from_secs(5));
}

#[test]
fn the_newest_rotated_file_is_read_when_the_window_reaches_before_the_current_file() {
    let mut rig = Rig::new();
    across_midnight(&mut rig);
    // The rotation names the file after the day it rotated on, which holds the day before.
    rig.put(
        "node1-2026-10-03.log.gz",
        gzip(ROTATION_PREVIOUS.as_bytes()),
        NOW_MS,
    );
    rig.put("node1-2026-10-02.log.gz", gzip(b"[2026-10-01 23:59:00.000] WARNING: Detected double forging by mallory \xe2\x9b\x93\xef\xb8\x8f\n"), NOW_MS);
    rig.put("node1-current.log.txt", "node1-2026-10-03.log.gz\n", NOW_MS);
    rig.put("node1-current.log", ROTATION_CURRENT, NOW_MS);
    let window = rig.log(Role::Relay).expect("a window");
    let expected = [
        LineKind::InSync,
        LineKind::ForkRecoveryStarted,
        LineKind::BlocksRemoved {
            blocks: 2,
            reset_to: 900,
        },
        LineKind::InSync,
    ];
    assert_eq!(kinds(&window), expected.iter().collect::<Vec<_>>());
    let rotated: Vec<bool> = window.events.iter().map(|e| e.rotated).collect();
    assert_eq!(rotated, [true, true, false, false]);
    let times: Vec<i64> = window.events.iter().map(|e| e.at_ms).collect();
    assert!(times.windows(2).all(|pair| pair[0] < pair[1]), "{times:?}");
    // The first line read is after the window's start and an older file exists: not the whole.
    assert!(!window.covered.whole);
    assert_eq!(
        window.covered.first_ms,
        Some(DAY_MS + 23 * 3_600_000 + 40 * 60_000)
    );
    assert_eq!(
        window.covered.last_ms,
        Some(DAY_MS + 24 * 3_600_000 + 10 * 60_000)
    );
}

#[test]
fn a_rotated_file_that_starts_inside_the_window_still_covers_less() {
    let mut rig = Rig::new();
    across_midnight(&mut rig);
    rig.put(
        "node1-2026-10-03.log.gz",
        gzip(ROTATION_PREVIOUS.as_bytes()),
        NOW_MS,
    );
    rig.put("node1-current.log", ROTATION_CURRENT, NOW_MS);
    let window = rig.log(Role::Relay).expect("a window");
    assert_eq!(window.events.len(), 4);
    // Nothing is older, but the log read starts at 23:40, ten minutes into the window.
    assert!(!window.covered.whole);
    assert_eq!(
        window.covered.first_ms,
        Some(DAY_MS + 23 * 3_600_000 + 40 * 60_000)
    );
}

#[test]
fn the_rotated_file_is_left_unread_when_the_current_file_reaches_the_window_s_start() {
    let mut rig = Rig::new();
    across_midnight(&mut rig);
    rig.window.from_ms = DAY_MS + 24 * 3_600_000 + 1000;
    rig.put(
        "node1-2026-10-03.log.gz",
        gzip(ROTATION_PREVIOUS.as_bytes()),
        NOW_MS,
    );
    rig.put("node1-current.log", ROTATION_CURRENT, NOW_MS);
    let counting = Counting {
        inner: &rig.fs,
        tails: RefCell::default(),
        opened: RefCell::default(),
    };
    let window = read_log(&rig.context(&counting, Role::Relay, "node1")).expect("a window");
    assert_eq!(window.events.len(), 2);
    assert!(
        counting
            .opened
            .borrow()
            .iter()
            .all(|p| !p.to_string_lossy().ends_with(".gz"))
    );
}

#[test]
fn a_corrupt_rotated_file_is_left_out_and_the_current_file_still_reads() {
    let mut rig = Rig::new();
    across_midnight(&mut rig);
    rig.put(
        "node1-2026-10-03.log.gz",
        b"this is not gzip".to_vec(),
        NOW_MS,
    );
    rig.put("node1-current.log", ROTATION_CURRENT, NOW_MS);
    let window = rig.log(Role::Relay).expect("a window");
    assert_eq!(window.events.len(), 2);
    assert!(!window.covered.whole);
}

#[test]
fn a_rotated_file_of_the_same_day_is_chosen_by_its_number() {
    let mut rig = Rig::new();
    across_midnight(&mut rig);
    let older = "[2026-10-02 23:35:00.000] INFO: Starting fork recovery 🍴\n";
    rig.put("node1-2026-10-03.log.gz", gzip(older.as_bytes()), NOW_MS);
    rig.put(
        "node1-2026-10-03.1.log.gz",
        gzip(ROTATION_PREVIOUS.as_bytes()),
        NOW_MS,
    );
    rig.put("node1-2026-10-03.10.log.gz", gzip(older.as_bytes()), NOW_MS);
    rig.put("node1-current.log", ROTATION_CURRENT, NOW_MS);
    let window = rig.log(Role::Relay).expect("a window");
    // .10 is newer than .1 (numbers, not text): its one line and the current file's two.
    assert_eq!(window.events.len(), 3);
}

#[test]
fn the_current_file_is_read_from_its_last_eight_mebibytes_and_a_cut_line_is_dropped() {
    let mut rig = Rig::new();
    let line = IN_SYNC.as_bytes();
    let count = 20 * MIB / line.len();
    let bytes = line.repeat(count);
    let size = u64::try_from(bytes.len()).unwrap_or(0);
    rig.put("node1-current.log", bytes, NOW_MS);
    let counting = Counting {
        inner: &rig.fs,
        tails: RefCell::default(),
        opened: RefCell::default(),
    };
    let window = read_log(&rig.context(&counting, Role::Relay, "node1")).expect("a window");
    assert!(window.capped);
    assert_eq!(*counting.tails.borrow(), [MAX_TAIL]);
    // The line the cut left partial is not read as a line.
    assert_eq!(window.other_lines, 0);
    assert!(window.events.len() <= usize::try_from(MAX_TAIL).unwrap_or(0) / line.len());
    assert!(window.events.iter().all(|e| e.offset > size - MAX_TAIL - 1));
    // Only the last eight mebibytes are of the log: its start is not covered.
    assert!(window.events.len() > 100_000);
}

#[test]
fn a_run_reads_at_most_thirty_two_mebibytes_of_logs() {
    let mut rig = Rig::new();
    let line = IN_SYNC.as_bytes();
    let bytes = line.repeat(9 * MIB / line.len());
    for stem in ["a", "b", "c", "d", "e"] {
        rig.put(&format!("{stem}-current.log"), bytes.clone(), NOW_MS);
    }
    let counting = Counting {
        inner: &rig.fs,
        tails: RefCell::default(),
        opened: RefCell::default(),
    };
    let mut reads = Vec::new();
    for stem in ["a", "b", "c", "d", "e"] {
        reads.push(read_log(&rig.context(&counting, Role::Relay, stem)));
    }
    assert!(reads[..4].iter().all(Result::is_ok));
    assert_eq!(
        reads[4].as_ref().map(|_| ()).unwrap_err().reason,
        Reason::RunBudget
    );
    let total: u64 = counting.tails.borrow().iter().sum();
    assert_eq!(total, MAX_RUN);
    assert_eq!(*counting.tails.borrow(), [MAX_TAIL; 4]);
}

#[test]
fn a_small_file_charges_the_run_only_for_its_size() {
    let mut rig = Rig::new();
    rig.put("node1-current.log", IN_SYNC, NOW_MS);
    let counting = Counting {
        inner: &rig.fs,
        tails: RefCell::default(),
        opened: RefCell::default(),
    };
    for _ in 0..40 {
        assert!(read_log(&rig.context(&counting, Role::Relay, "node1")).is_ok());
    }
    let size = u64::try_from(IN_SYNC.len()).unwrap_or(0);
    assert_eq!(*counting.tails.borrow(), [size; 40]);
}

fn capture_at(
    text: &str,
    modified_ms: i64,
    since_ms: Option<i64>,
    role: Role,
) -> Result<heartwood_cli::doctor::sources::logs::CaptureWindow, Unknown> {
    let mut rig = Rig::new();
    rig.put("node1-error.log", text, modified_ms);
    read_capture(&rig.context(&rig.fs, role, "node1"), since_ms)
}

#[test]
fn the_error_capture_holds_start_errors_and_counts_the_rest_as_other_lines() {
    let text = "the P2P server cannot listen on 192.0.2.10:4001: address in use\n\
                at Object.listen (node:net:1)\n\
                the API cannot listen on 192.0.2.10:4003: Address already in use (os error 98)\n\
                the API cannot listen on 192.0.2.10:4003: Address already in use (os error 98)\n";
    let window = capture_at(text, NOW_MS, Some(NOW_MS - 60_000), Role::Relay).expect("a capture");
    assert_eq!(
        window.errors,
        [
            StartError::P2pCannotListen {
                port: Some(4001),
                in_use: true
            },
            StartError::ApiCannotListen {
                port: Some(4003),
                in_use: true
            },
        ]
    );
    assert_eq!(window.other_lines, 1);
    assert_eq!(window.excerpts.len(), 1);
    let excerpt = &window.excerpts[0];
    assert_eq!(excerpt.source, UntrustedSource::ErrorCapture(Role::Relay));
    assert_eq!(excerpt.at_ms, None);
    assert!(excerpt.text.contains("Object.listen"));
}

#[test]
fn the_error_capture_of_the_forger_names_its_start_errors() {
    let text = "The delegates.json file does not exist\n\
                delegates.json is open to its group or to others (mode 644); make it private (chmod 600)\n";
    let window = capture_at(text, NOW_MS, None, Role::Forger).expect("a capture");
    assert_eq!(
        window.errors,
        [
            StartError::KeyFileMissing,
            StartError::KeyFileTooOpen { mode: 0o644 }
        ]
    );
    assert_eq!(window.other_lines, 0);
}

#[test]
fn an_error_capture_last_written_before_the_start_holds_nothing_since_it() {
    let text = "The delegates.json file does not exist\n";
    let window = capture_at(text, NOW_MS - 120_000, Some(NOW_MS - 60_000), Role::Forger);
    assert_eq!(window.expect("a capture"), Default::default());
    // Without a start time the whole file is read.
    let window = capture_at(text, NOW_MS - 120_000, None, Role::Forger).expect("a capture");
    assert_eq!(window.errors, [StartError::KeyFileMissing]);
}

#[test]
fn the_error_capture_is_read_by_the_same_caps_and_its_excerpts_are_capped() {
    let long = "x".repeat(3000);
    let window = capture_at(&format!("{long}\n"), NOW_MS, None, Role::Relay).expect("a capture");
    assert_eq!(window.excerpts.len(), 1);
    assert_eq!(window.excerpts[0].text.len(), MAX_EXCERPT);
    let missing = Rig::new();
    let error = read_capture(&missing.context(&missing.fs, Role::Relay, "node1"), None);
    let error = error.map(|_| ()).unwrap_err();
    assert_eq!(
        (error.source, error.reason),
        (Source::ErrorCapture, Reason::Missing)
    );
}

#[test]
fn the_forger_s_delegate_lines_before_the_window_since_its_last_start_are_kept_apart() {
    let text = "\
[2026-10-02 09:00:00.000] INFO: Loaded 2 active delegates: genesis_8, genesis_9
[2026-10-02 10:00:00.000] INFO: Forger Manager started 🔨
[2026-10-02 10:00:08.000] INFO: Loaded 2 active delegates: genesis_1, genesis_2
[2026-10-02 10:00:08.000] INFO: Loaded 1 inactive delegate: genesis_3
[2026-10-02 11:00:00.000] INFO: Delegate genesis_4 is now in an active forging position 🎉
[2026-10-02 11:00:08.000] WARNING: Loaded 1 active delegate: genesis_7
[2026-10-02 13:10:00.000] INFO: Loaded 1 active delegate: genesis_5
";
    let window = read(Role::Forger, text);
    let listed = |events: &[heartwood_cli::doctor::sources::logs::LogEvent]| {
        let names = events.iter().map(|event| match &event.kind {
            LineKind::DelegatesLoaded { names, .. } | LineKind::ActivePosition { names, .. } => {
                names.iter().map(|name| name.as_str().to_owned()).collect()
            }
            other => vec![format!("{other:?}")],
        });
        names.collect::<Vec<Vec<String>>>()
    };
    // The lines since the last start, before the window; not the start before it, nor a line of
    // another level.
    assert_eq!(
        listed(&window.roster),
        [
            vec!["genesis_1", "genesis_2"],
            vec!["genesis_3"],
            vec!["genesis_4"]
        ]
    );
    assert_eq!(listed(&window.events), [vec!["genesis_5"]]);
    // The relay's log keeps no such lines.
    assert!(read(Role::Relay, text).roster.is_empty());
}
