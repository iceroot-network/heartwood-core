//! The cases of the clock checks: the host's synchronization, the relay's clock against the chain's
//! slots, and the symptoms of a wrong clock in the logs.

use heartwood_cli::Role;
use heartwood_cli::doctor::report::{Id, IdKind, Status};
use heartwood_cli::doctor::seams::{StoreWindow, StoredBlock, Subject};
use heartwood_cli::doctor::sources::logs::{Covered, LineKind, LogEvent, LogWindow};
use heartwood_cli::doctor::sources::probes::{ChronyTracking, LeapStatus, NtpState};
use heartwood_cli::doctor::view::{NodeFile, Reason, Source, Unknown};
use heartwood_kernel::log::Level;

use super::Case;
use super::net::{
    EPOCH_MS, base, case, chain, denied, ex, file_of, forger, presence, relay, resolve,
};
use crate::doctor_support::FakeView;

const HOST: Subject = Subject::Host;
const RELAY: Subject = Subject::Relay;
const FORGER: Subject = Subject::Forger;

/// A view whose time service says `synchronized` and `ntp`, and whose chrony (when there) is
/// `offset_us` microseconds off.
fn sync_view(
    synchronized: Option<bool>,
    ntp: Option<bool>,
    chrony: Option<(i64, LeapStatus)>,
) -> FakeView {
    let mut view = base();
    view.ntp = Ok(NtpState { synchronized, ntp });
    view.chrony = match chrony {
        Some((offset_us, leap)) => Ok(ChronyTracking { offset_us, leap }),
        None => Err(Unknown::new(Source::Probes, Reason::ProbeMissing)),
    };
    view
}

/// The host's synchronization.
fn sync() -> Vec<Case> {
    use LeapStatus::{Normal, NotSynchronised};
    let id = "clock.sync";
    let mut cases = Vec::new();
    let mut add = |name, view, status, key| {
        cases.push(case(name, id, view, vec![ex(HOST, status, key)]));
    };
    let (yes, on) = (Some(true), Some(true));
    add(
        "synchronized, no chrony",
        sync_view(yes, on, None),
        Status::Pass,
        "synchronized",
    );
    add(
        "offset 0",
        sync_view(yes, on, Some((0, Normal))),
        Status::Pass,
        "synchronized-offset",
    );
    add(
        "offset 99 ms",
        sync_view(yes, on, Some((99_999, Normal))),
        Status::Pass,
        "synchronized-offset",
    );
    add(
        "offset 100 ms",
        sync_view(yes, on, Some((100_000, Normal))),
        Status::Warn,
        "offset",
    );
    add(
        "offset 499 ms",
        sync_view(yes, on, Some((499_999, Normal))),
        Status::Warn,
        "offset",
    );
    add(
        "offset 500 ms",
        sync_view(yes, on, Some((500_000, Normal))),
        Status::Fail,
        "offset-large",
    );
    add(
        "offset 500 ms behind",
        sync_view(yes, on, Some((-500_000, Normal))),
        Status::Fail,
        "offset-large",
    );
    add(
        "offset -99 ms behind",
        sync_view(yes, on, Some((-99_000, Normal))),
        Status::Pass,
        "synchronized-offset",
    );
    add(
        "time service off",
        sync_view(yes, Some(false), None),
        Status::Warn,
        "ntp-off",
    );
    add(
        "not synchronized",
        sync_view(Some(false), on, None),
        Status::Fail,
        "not-synchronized",
    );
    add(
        "chrony lost its source",
        sync_view(yes, on, Some((0, NotSynchronised))),
        Status::Fail,
        "not-synchronized",
    );
    add(
        "chrony alone, in step",
        sync_view(None, None, Some((10_000, Normal))),
        Status::Pass,
        "synchronized-offset",
    );
    let mut view = sync_view(None, None, None);
    view.ntp = Err(Unknown::new(Source::Probes, Reason::ProbeMissing));
    view.files
        .insert(NodeFile::TimesyncMarker, Ok(Some(file_of(0))));
    add(
        "synchronization marker only",
        view,
        Status::Pass,
        "synchronized",
    );
    let mut view = sync_view(None, None, None);
    view.ntp = Err(Unknown::new(Source::Probes, Reason::ProbeMissing));
    view.files.insert(NodeFile::TimesyncMarker, Ok(None));
    add("no time service to ask", view, Status::Unknown, "unknown");
    let mut view = base();
    view.ntp = Err(denied(Source::Probes));
    view.chrony = Err(denied(Source::Probes));
    add("probes unreadable", view, Status::Unknown, "unknown");
    cases
}

/// A log of `role` with `events`, all at `level`.
fn log(role: Role, level: Level, events: Vec<(i64, LineKind)>) -> LogWindow {
    let events: Vec<LogEvent> = events
        .into_iter()
        .zip(0_u64..)
        .map(|((at_ms, kind), offset)| LogEvent {
            kind,
            level,
            at_ms,
            offset,
            rotated: false,
        })
        .collect();
    LogWindow {
        role,
        covered: Covered {
            first_ms: events.first().map(|event| event.at_ms),
            last_ms: events.last().map(|event| event.at_ms),
            whole: true,
        },
        events,
        other_lines: 0,
        untrusted_lines: 0,
        file_level: None,
        excerpts: Vec::new(),
        capped: false,
    }
}

/// A delegate's name.
fn name() -> Id {
    Id::new(IdKind::Username, "delegate1").unwrap()
}

/// A view of a relay that received blocks 100, 101, ... `offsets` after their slots began (a
/// block a slot 8 s after the last), and whose store knows the blocks up to `stored` (a height).
fn receipts_view(offsets: &[i64], stored: u32) -> FakeView {
    let mut view = base();
    view.relay_config = Ok(relay("0.0.0.0:4001", None, Vec::new()));
    let heights = 100_u32..;
    let slots = (0_i64..).map(|slot| slot * 8_000);
    let received = offsets.iter().zip(heights.clone()).zip(slots.clone());
    let events = received
        .map(|((offset, height), slot)| {
            let kind = LineKind::BlockReceived {
                username: name(),
                rank: 1,
                height,
            };
            (EPOCH_MS + slot + offset, kind)
        })
        .collect();
    let blocks = heights
        .zip(slots)
        .take_while(|(height, _)| *height <= stored)
        .map(|(height, slot)| StoredBlock {
            height,
            timestamp: u32::try_from(slot / 1000).unwrap(),
            id: "1".repeat(64),
            generator_public_key: format!("02{}", "2".repeat(64)),
            username: None,
        })
        .collect();
    view.logs
        .insert(Role::Relay, Ok(log(Role::Relay, Level::Info, events)));
    view.store_window = Ok(StoreWindow {
        missed: Vec::new(),
        blocks,
        capped: false,
    });
    view
}

/// `count` receipts, all `offset` ms after their slots began.
fn all_at(offset: i64, count: usize) -> FakeView {
    receipts_view(&vec![offset; count], 1_000)
}

/// The relay's clock against the chain's slots.
fn vs_chain() -> Vec<Case> {
    let id = "clock.vs-chain";
    let mut cases = Vec::new();
    let mut add = |name, view, status, key| {
        cases.push(case(name, id, view, vec![ex(RELAY, status, key)]));
    };
    add(
        "blocks 500 ms early",
        all_at(-500, 10),
        Status::Fail,
        "early",
    );
    add(
        "blocks 499 ms early",
        all_at(-499, 10),
        Status::Warn,
        "early",
    );
    add(
        "blocks 100 ms early",
        all_at(-100, 10),
        Status::Warn,
        "early",
    );
    add(
        "blocks 99 ms early",
        all_at(-99, 10),
        Status::Pass,
        "in-step",
    );
    add("blocks on the slot", all_at(0, 10), Status::Pass, "in-step");
    add(
        "blocks 1999 ms late",
        all_at(1999, 10),
        Status::Pass,
        "in-step",
    );
    add(
        "blocks 2000 ms late",
        all_at(2000, 10),
        Status::Warn,
        "late",
    );
    let mut mixed = vec![-600; 5];
    mixed.extend([0; 5]);
    add(
        "even count averages the middle two",
        receipts_view(&mixed, 1_000),
        Status::Warn,
        "early",
    );
    let mut odd = vec![-600; 5];
    odd.extend([0; 6]);
    add(
        "odd count takes the middle one",
        receipts_view(&odd, 1_000),
        Status::Pass,
        "in-step",
    );
    add("nine receipts", all_at(0, 9), Status::Unknown, "too-few");
    add("no receipts", all_at(0, 0), Status::Unknown, "too-few");
    add("ten receipts", all_at(0, 10), Status::Pass, "in-step");
    let mut early_liars = vec![300; 10];
    early_liars.extend([-60_000; 3]);
    add(
        "false early receipts do not move the median",
        receipts_view(&early_liars, 1_000),
        Status::Pass,
        "in-step",
    );
    let mut late_liars = vec![300; 10];
    late_liars.extend([90_000; 3]);
    add(
        "false late receipts do not move the median",
        receipts_view(&late_liars, 1_000),
        Status::Pass,
        "in-step",
    );
    let mut honest = vec![-700; 10];
    honest.extend([0; 3]);
    add(
        "a few timely receipts do not hide an early clock",
        receipts_view(&honest, 1_000),
        Status::Fail,
        "early",
    );
    add(
        "receipts the store does not hold are not counted",
        receipts_view(&[0; 12], 108),
        Status::Unknown,
        "too-few",
    );
    add(
        "receipts the store holds are counted",
        receipts_view(&[0; 12], 109),
        Status::Pass,
        "in-step",
    );
    let mut view = all_at(0, 10);
    view.store_window = Err(denied(Source::Store));
    add("store unreadable", view, Status::Unknown, "unknown");
    let mut view = all_at(0, 10);
    view.logs.clear();
    add("log unreadable", view, Status::Unknown, "unknown");
    let mut view = all_at(0, 10);
    if let Some(Ok(window)) = view.logs.get_mut(&Role::Relay) {
        window.file_level = Some(Level::Warning);
    }
    add(
        "log level hides the receipts",
        view,
        Status::Unknown,
        "unknown",
    );
    let mut view = all_at(0, 10);
    view.roles_present = Ok(presence(false, true));
    add("no relay here", view, Status::Skipped, "skipped-absent");
    cases
}

/// A view whose relay log is `relay_events` and whose forger log is `forger_events` (at the level
/// the lines are written at), the forger's block time `block_time_s`.
fn symptoms_view(
    relay_events: Vec<(i64, LineKind)>,
    forger_events: Vec<(i64, LineKind)>,
    block_time_s: u32,
) -> FakeView {
    let mut view = base();
    let mut config = forger(&["127.0.0.1:4001"], None);
    config.chain = chain(block_time_s);
    view.forger_config = Ok(config);
    view.logs
        .insert(Role::Relay, Ok(log(Role::Relay, Level::Info, relay_events)));
    view.logs.insert(
        Role::Forger,
        Ok(log(Role::Forger, Level::Warning, forger_events)),
    );
    view
}

/// Discards, one after another.
fn discards(count: usize) -> Vec<(i64, LineKind)> {
    let kinds = [
        LineKind::DiscardedFutureSlot { height: 5 },
        LineKind::DiscardedReceivedLate { height: 6 },
        LineKind::DiscardedProcessedLate { height: 7 },
    ];
    (0..count)
        .zip(kinds.into_iter().cycle())
        .map(|(n, kind)| (EPOCH_MS + i64::try_from(n).unwrap() * 1000, kind))
        .collect()
}

/// A forger's failed attempt, `late` ms into the log after a start line at time zero.
fn too_little(at: i64) -> (i64, LineKind) {
    let kind = LineKind::TooLittleTime {
        username: name(),
        left_ms: 100,
        minimum_ms: 500,
    };
    (EPOCH_MS + at, kind)
}

/// The forger's start line at time zero.
fn started() -> (i64, LineKind) {
    (EPOCH_MS, LineKind::ForgerStarted)
}

/// The symptoms of a wrong clock.
fn symptoms() -> Vec<Case> {
    let id = "clock.symptoms";
    let mut cases = Vec::new();
    let mut add = |name, view, subject, status, key| {
        cases.push(case(name, id, view, vec![ex(subject, status, key)]));
    };
    let quiet = || symptoms_view(Vec::new(), Vec::new(), 8);
    add("relay log clean", quiet(), RELAY, Status::Pass, "clean");
    add("forger log clean", quiet(), FORGER, Status::Pass, "clean");
    add(
        "one discard",
        symptoms_view(discards(1), Vec::new(), 8),
        RELAY,
        Status::Warn,
        "discarded",
    );
    add(
        "two discards",
        symptoms_view(discards(2), Vec::new(), 8),
        RELAY,
        Status::Warn,
        "discarded",
    );
    add(
        "three discards",
        symptoms_view(discards(3), Vec::new(), 8),
        RELAY,
        Status::Fail,
        "discarded",
    );
    let far = |username| {
        (
            EPOCH_MS,
            LineKind::WillNotSign {
                username,
                far_slot: true,
            },
        )
    };
    let near = (
        EPOCH_MS,
        LineKind::WillNotSign {
            username: name(),
            far_slot: false,
        },
    );
    add(
        "a far slot refused",
        symptoms_view(Vec::new(), vec![far(name())], 8),
        FORGER,
        Status::Fail,
        "refused",
    );
    add(
        "a far slot not attested",
        symptoms_view(
            Vec::new(),
            vec![(EPOCH_MS, LineKind::NotAttesting { far_slot: true })],
            8,
        ),
        FORGER,
        Status::Fail,
        "refused",
    );
    add(
        "a near slot refused",
        symptoms_view(Vec::new(), vec![near], 8),
        FORGER,
        Status::Pass,
        "clean",
    );
    add(
        "a far refusal outweighs late attempts",
        symptoms_view(Vec::new(), vec![too_little(60_000), far(name())], 8),
        FORGER,
        Status::Fail,
        "refused",
    );
    add(
        "one late attempt",
        symptoms_view(Vec::new(), vec![too_little(60_000)], 8),
        FORGER,
        Status::Warn,
        "late",
    );
    let three = vec![too_little(60_000), too_little(70_000), too_little(80_000)];
    add(
        "three late attempts",
        symptoms_view(Vec::new(), three, 8),
        FORGER,
        Status::Fail,
        "late",
    );
    let next = (
        EPOCH_MS + 60_000,
        LineKind::AlreadyNextSlot { username: name() },
    );
    add(
        "already in the next slot",
        symptoms_view(Vec::new(), vec![next], 8),
        FORGER,
        Status::Warn,
        "late",
    );
    let first = vec![started(), too_little(7_999)];
    add(
        "first slot after a start, last millisecond",
        symptoms_view(Vec::new(), first, 8),
        FORGER,
        Status::Pass,
        "clean",
    );
    let second = vec![started(), too_little(8_000)];
    add(
        "second slot after a start",
        symptoms_view(Vec::new(), second, 8),
        FORGER,
        Status::Warn,
        "late",
    );
    let manager = vec![(EPOCH_MS, LineKind::ForgerManagerStarted), too_little(100)];
    add(
        "first slot after the manager starts",
        symptoms_view(Vec::new(), manager, 8),
        FORGER,
        Status::Pass,
        "clean",
    );
    let next_first = (
        EPOCH_MS + 3_000,
        LineKind::AlreadyNextSlot { username: name() },
    );
    add(
        "next-slot line in the first slot",
        symptoms_view(Vec::new(), vec![started(), next_first], 8),
        FORGER,
        Status::Pass,
        "clean",
    );
    let short = vec![started(), too_little(3_999)];
    add(
        "a 4 s slot, inside",
        symptoms_view(Vec::new(), short, 4),
        FORGER,
        Status::Pass,
        "clean",
    );
    let short = vec![started(), too_little(4_000)];
    add(
        "a 4 s slot, outside",
        symptoms_view(Vec::new(), short, 4),
        FORGER,
        Status::Warn,
        "late",
    );
    let before = vec![started(), too_little(-1)];
    add(
        "an attempt before the start line",
        symptoms_view(Vec::new(), before, 8),
        FORGER,
        Status::Warn,
        "late",
    );
    let mut view = quiet();
    view.logs.remove(&Role::Relay);
    add(
        "relay log unreadable",
        view,
        RELAY,
        Status::Unknown,
        "unknown",
    );
    let mut view = quiet();
    if let Some(Ok(window)) = view.logs.get_mut(&Role::Forger) {
        window.file_level = Some(Level::Error);
    }
    add(
        "forger log level hides the warnings",
        view,
        FORGER,
        Status::Unknown,
        "unknown",
    );
    let mut view = quiet();
    view.logs.remove(&Role::Forger);
    add(
        "forger log unreadable",
        view,
        FORGER,
        Status::Unknown,
        "unknown",
    );
    let mut view = quiet();
    view.roles_present = Ok(presence(true, false));
    add(
        "no forger here",
        view,
        FORGER,
        Status::Skipped,
        "skipped-absent",
    );
    let mut view = quiet();
    view.roles_present = Ok(presence(false, true));
    add(
        "no relay here",
        view,
        RELAY,
        Status::Skipped,
        "skipped-absent",
    );
    cases
}

/// Every case of the area.
pub fn cases() -> Vec<Case> {
    [sync(), vs_chain(), symptoms()]
        .into_iter()
        .flatten()
        .collect()
}

#[test]
fn every_clock_advice_has_its_template_and_its_values() {
    resolve("clock", cases());
}
