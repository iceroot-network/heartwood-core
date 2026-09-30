//! The cases of the slot checks, and the builders the forger and quorum cases share: a chain whose
//! network epoch is far from the Unix epoch, a relay and a forger on this host, their logs, the
//! store window and the process table.

use std::net::SocketAddr;
use std::path::PathBuf;

use heartwood_cli::doctor::DoctorOptions;
use heartwood_cli::doctor::registry::{CheckInput, ENTRIES};
use heartwood_cli::doctor::report::{Finding, Id, IdKind, Status};
use heartwood_cli::doctor::seams::{
    AccessEntry, ChainFacts, ForgerFacts, MissedSlot, RelayFacts, RoleState, StoreWindow,
    StoredBlock, Subject, SupervisorFacts,
};
use heartwood_cli::doctor::sources::logs::{Covered, LineKind, LogEvent, LogWindow};
use heartwood_cli::doctor::view::{ProcessFacts, Reason, RolePresence, Source, Unknown};
use heartwood_cli::{Role, StatusRow, SupervisedStatus};
use heartwood_kernel::log::Level;

use super::{Case, Expect};
use crate::doctor_support::{FakeView, NOW_MS};

/// The network epoch: 2026-09-01 00:00:00 UTC, far from the Unix epoch.
pub const EPOCH_MS: i64 = 1_788_220_800_000;
/// The slot the fakes' clock falls in.
const SLOT: i64 = (NOW_MS - EPOCH_MS) / 8_000;
/// The tip's height, forged in the slot before [`NOW_MS`]'s.
pub const TIP: u32 = 10_000;
/// Our delegate.
pub const OURS: &str = "genesis_1";

/// The start of the slot `back` slots before the current one, in Unix milliseconds.
pub fn at(back: i64) -> i64 {
    EPOCH_MS + (SLOT - back) * 8_000
}

/// The same slot's first second since the network epoch, as the store keeps it.
pub fn ts(back: i64) -> u32 {
    u32::try_from((SLOT - back) * 8).unwrap()
}

/// A public key: 66 hex digits from `n`.
pub fn key(n: u8) -> String {
    format!("02{}", format!("{n:02x}").repeat(32))
}

/// A block id: 64 hex digits from `n`.
pub fn block_id(n: u8) -> String {
    format!("{n:02x}").repeat(32)
}

/// A delegate's name.
pub fn username(name: &str) -> Id {
    Id::new(IdKind::Username, name).unwrap()
}

/// A socket address.
pub fn addr(text: &str) -> SocketAddr {
    text.parse().unwrap()
}

/// The chain definition.
pub fn chain() -> ChainFacts {
    ChainFacts {
        nethash: block_id(0xab),
        epoch_ms: EPOCH_MS,
        block_time_s: 8,
        active_delegates: 53,
    }
}

/// A relay on 127.0.0.1 that wants 20 peers.
pub fn relay() -> RelayFacts {
    RelayFacts {
        p2p: addr("0.0.0.0:6002"),
        api: Some(addr("127.0.0.1:6003")),
        remote_access: vec![AccessEntry::Address("127.0.0.1".parse().unwrap())],
        minimum_network_reach: 20,
        file_log_level: Level::Info,
        warnings: Vec::new(),
        env_names: Vec::new(),
        chain: chain(),
        store_file: PathBuf::from("/nodes/node1/data/database/blockchain.db"),
    }
}

/// A forger with `hosts`.
pub fn forger(hosts: &[&str]) -> ForgerFacts {
    ForgerFacts {
        hosts: hosts.iter().map(|host| addr(host)).collect(),
        local_address: None,
        aux_set: false,
        file_log_level: Level::Info,
        key_file: PathBuf::from("/nodes/node1/config/delegates.json"),
        signing_record: PathBuf::from("/nodes/node1/data/forger/signing-record.json"),
        chain: chain(),
    }
}

/// A log of `role` with `events` at their times, at info.
pub fn log(role: Role, events: Vec<(i64, LineKind)>) -> LogWindow {
    let times: Vec<i64> = events.iter().map(|(at, _)| *at).collect();
    let events = events
        .into_iter()
        .enumerate()
        .map(|(n, (at_ms, kind))| LogEvent {
            kind,
            level: Level::Info,
            at_ms,
            offset: u64::try_from(n).unwrap(),
            rotated: false,
        });
    LogWindow {
        role,
        events: events.collect(),
        other_lines: 0,
        untrusted_lines: 0,
        covered: Covered {
            first_ms: times.first().copied(),
            last_ms: times.last().copied(),
            whole: true,
        },
        file_level: None,
        excerpts: Vec::new(),
        capped: false,
        roster: Vec::new(),
    }
}

/// "Loaded 1 active delegates: genesis_1", 53 slots ago.
pub fn loaded() -> (i64, LineKind) {
    let names = vec![username(OURS)];
    let kind = LineKind::DelegatesLoaded {
        active: true,
        count: 1,
        names,
    };
    (at(400), kind)
}

/// A block `back` slots ago at `height`, signed by the key of `generator`.
pub fn block(back: i64, height: u32, generator: u8, name: &str) -> StoredBlock {
    StoredBlock {
        height,
        timestamp: ts(back),
        id: block_id(generator ^ u8::try_from(height % 256).unwrap()),
        generator_public_key: key(generator),
        username: Some(name.into()),
    }
}

/// A missed slot of ours `back` slots ago.
pub fn miss(back: i64) -> MissedSlot {
    MissedSlot {
        timestamp: ts(back),
        height: TIP - u32::try_from(back).unwrap() + 1,
        username: OURS.into(),
    }
}

/// A role's process in `state`, its supervisor started at `started_ms`.
pub fn process(state: RoleState, started_ms: i64) -> ProcessFacts {
    let status = match state {
        RoleState::Running { .. } => SupervisedStatus::Online,
        _ => SupervisedStatus::Stopped,
    };
    ProcessFacts {
        row: StatusRow {
            pid: Some(4242),
            name: "solar-testnet-forger".into(),
            version: "4.3.1-heartwood.0.1.0".into(),
            status,
            uptime: None,
            restarts: 0,
            cpu_percent: None,
            rss_bytes: None,
        },
        state,
        supervisor: Some(SupervisorFacts {
            started_at_ms: Some(started_ms),
            restarts: 0,
            unstable: 0,
            version: "4.3.1-heartwood.0.1.0".into(),
        }),
    }
}

/// Running since two hours ago.
pub const RUNNING: RoleState = RoleState::Running { supervised: true };

/// A relay and a forger on this host, both running for two hours; the forger's log loaded our
/// delegate; the tip at [`TIP`]; an empty relay log, store window and signing record.
pub fn view() -> FakeView {
    let mut view = FakeView::new();
    view.roles_present = Ok(RolePresence {
        relay: true,
        forger: true,
    });
    view.relay_config = Ok(relay());
    view.forger_config = Ok(forger(&["127.0.0.1:6002", "127.0.0.2:6002"]));
    view.api_last_block = Ok(heartwood_cli::doctor::sources::api::ApiBlock {
        id: block_id(1),
        height: TIP,
        timestamp: Some(ts(1)),
        generator_public_key: Some(key(2)),
        username: Some("genesis_2".into()),
    });
    view.store_window = Ok(StoreWindow::default());
    view.signing_record = Ok(None);
    view.logs
        .insert(Role::Forger, Ok(log(Role::Forger, vec![loaded()])));
    view.logs
        .insert(Role::Relay, Ok(log(Role::Relay, Vec::new())));
    let started = NOW_MS - 7_200_000;
    for role in Role::ALL {
        view.processes.insert(role, Ok(process(RUNNING, started)));
    }
    view
}

/// The same view with the forger's log holding `events` after the "Loaded" line.
pub fn with_log(events: Vec<(i64, LineKind)>) -> FakeView {
    let mut view = view();
    let mut all = vec![loaded()];
    all.extend(events);
    view.logs.insert(Role::Forger, Ok(log(Role::Forger, all)));
    view
}

/// A case of `check` over `view` with the default options.
pub fn case(
    name: &'static str,
    check: &'static str,
    view: FakeView,
    expect: &[(Subject, Status, &'static str)],
) -> Case {
    let expect = expect.iter().map(|(subject, status, advice)| Expect {
        subject: *subject,
        status: *status,
        advice,
    });
    Case {
        name,
        check,
        view,
        options: DoctorOptions::default(),
        expect: expect.collect(),
    }
}

/// The case of `check` on a host with no forger: skipped.
pub fn absent(name: &'static str, check: &'static str) -> Case {
    let mut view = view();
    view.roles_present = Ok(RolePresence {
        relay: true,
        forger: false,
    });
    let expect = [(Subject::Forger, Status::Skipped, "skipped-absent")];
    case(name, check, view, &expect)
}

/// The findings of `check` over `view`.
pub fn run(check: &str, view: &FakeView, options: &DoctorOptions) -> Vec<Finding> {
    let entry = ENTRIES.iter().find(|entry| entry.id == check).unwrap();
    (entry.run)(&CheckInput {
        entry,
        view,
        options,
    })
}

/// An unknown source.
pub fn unknown(source: Source, reason: Reason) -> Unknown {
    Unknown::new(source, reason)
}

const F: Subject = Subject::Forger;

/// A store window of our missed slots `missed` and our blocks `forged`, both by slots back.
fn store(missed: &[i64], forged: &[i64]) -> StoreWindow {
    let height = |back: &i64| TIP - u32::try_from(*back).unwrap() + 1;
    let blocks = forged
        .iter()
        .map(|back| block(*back, height(back), 1, OURS));
    let mut blocks: Vec<StoredBlock> = blocks.collect();
    blocks.sort_by_key(|block| block.height);
    StoreWindow {
        missed: missed.iter().map(|back| miss(*back)).collect(),
        blocks,
        capped: false,
    }
}

/// The view with that store window and forger log.
fn missing(missed: &[i64], forged: &[i64], events: Vec<(i64, LineKind)>) -> FakeView {
    let mut view = with_log(events);
    view.store_window = Ok(store(missed, forged));
    view
}

/// A line inside the slot `back` slots ago.
fn inside(back: i64, kind: LineKind) -> (i64, LineKind) {
    (at(back) + 1_500, kind)
}

fn quorum(back: i64) -> (i64, LineKind) {
    inside(back, LineKind::NotPeeredEnough)
}

fn forged(back: i64, height: u32) -> (i64, LineKind) {
    let kind = LineKind::Forged {
        username: username(OURS),
        height,
        transactions: 0,
    };
    inside(back, kind)
}

/// Every case of the area.
pub fn cases() -> Vec<Case> {
    let mut cases = vec![
        absent("active: no forger on this host", "slots.active"),
        absent("missed: no forger on this host", "slots.missed"),
        case(
            "active: loaded as active",
            "slots.active",
            view(),
            &[(F, Status::Pass, "active")],
        ),
    ];
    // slots.active
    let left = LineKind::ActivePosition {
        entered: false,
        names: vec![username(OURS)],
    };
    let mut view_left = with_log(vec![(at(20), left)]);
    cases.push(case(
        "active: left the active set",
        "slots.active",
        view_left,
        &[(F, Status::Warn, "left")],
    ));
    view_left = view();
    view_left
        .logs
        .insert(Role::Forger, Ok(log(Role::Forger, Vec::new())));
    cases.push(case(
        "active: no delegate known",
        "slots.active",
        view_left,
        &[(F, Status::Skipped, "no-delegates")],
    ));
    let mut quiet = view();
    quiet
        .logs
        .insert(Role::Forger, Ok(log(Role::Forger, Vec::new())));
    let mut named = case(
        "active: named but not in the log",
        "slots.active",
        quiet,
        &[(F, Status::Unknown, "no-evidence")],
    );
    named.options.delegates = vec![OURS.into()];
    cases.push(named);
    let mut hidden = view();
    let mut level = log(Role::Forger, vec![loaded()]);
    level.file_level = Some(Level::Warning);
    hidden.logs.insert(Role::Forger, Ok(level));
    cases.push(case(
        "active: the log level hides info",
        "slots.active",
        hidden,
        &[(F, Status::Unknown, "unknown")],
    ));

    // slots.missed: the rules.
    cases.push(case(
        "missed: none",
        "slots.missed",
        missing(&[], &[53, 106], vec![]),
        &[(F, Status::Pass, "none")],
    ));
    let one = missing(&[60], &[7, 113], vec![quorum(60)]);
    cases.push(case(
        "missed: one in 5 rounds, no quorum",
        "slots.missed",
        one,
        &[(F, Status::Warn, "cause-quorum")],
    ));
    let edge = missing(&[264], &[7, 317], vec![]);
    cases.push(case(
        "missed: at the edge of 5 rounds",
        "slots.missed",
        edge,
        &[(F, Status::Warn, "cause-none")],
    ));
    let before = missing(&[265], &[7, 318], vec![]);
    cases.push(case(
        "missed: just before 5 rounds",
        "slots.missed",
        before,
        &[(F, Status::Pass, "cause-none")],
    ));
    let last_two = missing(&[10, 63], &[116], vec![quorum(10), quorum(63)]);
    cases.push(case(
        "missed: our last 2 slots",
        "slots.missed",
        last_two,
        &[(F, Status::Fail, "cause-quorum")],
    ));
    let three = missing(&[30, 83, 136], &[10], vec![]);
    cases.push(case(
        "missed: 3 in 5 rounds",
        "slots.missed",
        three,
        &[(F, Status::Fail, "cause-none")],
    ));
    let two = missing(&[30, 83], &[10], vec![]);
    cases.push(case(
        "missed: 2 in 5 rounds, the last forged",
        "slots.missed",
        two,
        &[(F, Status::Warn, "cause-none")],
    ));
    // The share over the window counts from 100 of our slots on.
    let older: Vec<i64> = (0..99).map(|n| 300 + n).collect();
    let share = missing(&[1_000], &older, vec![]);
    cases.push(case(
        "missed: 1% of 100 slots",
        "slots.missed",
        share,
        &[(F, Status::Warn, "cause-none")],
    ));
    let share = missing(&[1_000], &older[1..], vec![]);
    cases.push(case(
        "missed: 1 of 99 slots",
        "slots.missed",
        share,
        &[(F, Status::Pass, "cause-none")],
    ));

    // slots.missed: the causes.
    let mut stopped = missing(&[60], &[7, 113], vec![]);
    let events = vec![loaded(), (at(70), LineKind::ForgerManagerStarted)];
    stopped
        .logs
        .insert(Role::Forger, Ok(log(Role::Forger, events)));
    stopped.processes.insert(
        Role::Forger,
        Ok(process(RoleState::Stopped, NOW_MS - 7_200_000)),
    );
    cases.push(case(
        "missed: the forger was not running",
        "slots.missed",
        stopped,
        &[(F, Status::Warn, "cause-not-running")],
    ));
    let mut restarted = missing(&[60], &[7, 113], vec![]);
    restarted
        .processes
        .insert(Role::Forger, Ok(process(RUNNING, at(60) + 3_000)));
    cases.push(case(
        "missed: the forger started inside the slot",
        "slots.missed",
        restarted,
        &[(F, Status::Warn, "cause-not-running")],
    ));
    let late = LineKind::TooLittleTime {
        username: username(OURS),
        left_ms: 1_200,
        minimum_ms: 2_000,
    };
    let late = missing(&[60], &[7, 113], vec![inside(60, late)]);
    cases.push(case(
        "missed: the forger was late",
        "slots.missed",
        late,
        &[(F, Status::Warn, "cause-late")],
    ));
    let mut lost = missing(&[60], &[7, 113], vec![forged(60, TIP - 59)]);
    if let Ok(store) = &mut lost.store_window {
        store.blocks.push(block(59, TIP - 59, 2, "genesis_2"));
        store.blocks.sort_by_key(|block| block.height);
    }
    cases.push(case(
        "missed: signed but not on chain",
        "slots.missed",
        lost,
        &[
            (F, Status::Warn, "cause-not-on-chain"),
            (F, Status::Warn, "not-on-chain"),
        ],
    ));
    let mut unread = missing(&[60], &[7, 113], vec![]);
    unread.logs.remove(&Role::Forger);
    let mut named = case(
        "missed: the forger's log unread",
        "slots.missed",
        unread,
        &[(F, Status::Warn, "cause-unread")],
    );
    named.options.delegates = vec![OURS.into()];
    cases.push(named);

    // slots.missed: blocks the forger logged as forged that the chain does not hold as ours.
    let mut twice = missing(
        &[],
        &[7, 113],
        vec![forged(60, TIP - 59), forged(33, TIP - 32)],
    );
    if let Ok(store) = &mut twice.store_window {
        store.blocks.push(block(59, TIP - 59, 2, "genesis_2"));
        store.blocks.push(block(32, TIP - 32, 2, "genesis_2"));
        store.blocks.sort_by_key(|block| block.height);
    }
    cases.push(case(
        "missed: two blocks forged but not on chain",
        "slots.missed",
        twice,
        &[(F, Status::Pass, "none"), (F, Status::Fail, "not-on-chain")],
    ));

    // slots.missed: what it cannot read.
    let mut down = missing(&[60], &[7], vec![]);
    down.store_window = Err(unknown(Source::Store, Reason::NotRunning));
    cases.push(case(
        "missed: the relay does not run",
        "slots.missed",
        down,
        &[(F, Status::Unknown, "unknown")],
    ));
    let mut nobody = missing(&[60], &[7], vec![]);
    nobody
        .logs
        .insert(Role::Forger, Ok(log(Role::Forger, Vec::new())));
    cases.push(case(
        "missed: no delegate known",
        "slots.missed",
        nobody,
        &[(F, Status::Unknown, "no-delegates")],
    ));
    cases
}

#[test]
fn missed_slots_match_log_times_in_network_epoch_seconds_over_a_window_bounded_by_height() {
    // The epoch is 2026-09-01: read as Unix seconds, the missed slot would fall in 1970 and no
    // forger line would match it.
    let view = missing(&[60], &[7, 113], vec![quorum(60)]);
    let findings = run("slots.missed", &view, &DoctorOptions::default());
    let first = &findings[0];
    assert_eq!(first.advice.key, "cause-quorum");
    let time = first
        .facts
        .iter()
        .find(|(name, _)| *name == "time")
        .unwrap();
    assert_eq!(time.1, heartwood_cli::doctor::report::Fact::Time(at(60)));
    // The window of 60 minutes is 450 slots of 8 s: the store is read from the tip less 451.
    let calls = view.store_calls.borrow();
    assert_eq!(calls.as_slice(), [(vec![OURS.to_owned()], TIP - 451)]);
}

#[test]
fn our_delegates_come_from_the_flag_then_the_log_then_the_signing_record() {
    use heartwood_cli::doctor::sources::api::ApiWallet;
    use heartwood_cli::doctor::sources::fs::{SignedEntry, SigningRecordFacts};
    let mut view = missing(&[], &[7], vec![]);
    view.logs
        .insert(Role::Forger, Ok(log(Role::Forger, Vec::new())));
    view.signing_record = Ok(Some(SigningRecordFacts {
        known_format: true,
        nethash: block_id(0xab),
        entries: vec![SignedEntry {
            public_key: key(1),
            height: TIP - 6,
            slot: SLOT - 7,
            block_id: block_id(1),
        }],
    }));
    let wallet = ApiWallet {
        address: "D000".into(),
        public_key: Some(key(1)),
        username: Some("genesis_7".into()),
    };
    view.api_wallets.insert(key(1), Ok(Some(wallet)));
    let _ = run("slots.missed", &view, &DoctorOptions::default());
    let options = DoctorOptions {
        delegates: vec!["genesis_9".into(), "Not A Name".into()],
        ..DoctorOptions::default()
    };
    let _ = run("slots.missed", &view, &options);
    let calls = view.store_calls.borrow();
    let names: Vec<&Vec<String>> = calls.iter().map(|(names, _)| names).collect();
    assert_eq!(
        names,
        [&vec!["genesis_7".to_owned()], &vec!["genesis_9".to_owned()]]
    );
}

#[test]
fn blocks_logged_as_forged_that_the_chain_does_not_hold_rest_on_the_log() {
    use heartwood_cli::doctor::report::Basis;
    let mut twice = missing(
        &[60],
        &[7, 113],
        vec![forged(60, TIP - 59), forged(33, TIP - 32)],
    );
    if let Ok(store) = &mut twice.store_window {
        store.blocks.push(block(59, TIP - 59, 2, "genesis_2"));
        store.blocks.push(block(32, TIP - 32, 2, "genesis_2"));
        store.blocks.sort_by_key(|block| block.height);
    }
    let findings = run("slots.missed", &twice, &DoctorOptions::default());
    let basis = |key: &str| {
        let finding = findings.iter().find(|f| f.advice.key == key).unwrap();
        (finding.status, finding.basis)
    };
    // The missed slot is the store's; the blocks the chain does not hold are the log's alone.
    assert_eq!(basis("cause-not-on-chain"), (Status::Warn, Basis::Chain));
    assert_eq!(basis("not-on-chain"), (Status::Fail, Basis::Log));
}

#[test]
fn a_window_past_the_stores_row_cap_reads_the_latest_blocks_and_a_cut_window_is_not_judged() {
    use heartwood_cli::doctor::checks::slots::MAX_WINDOW_BLOCKS;
    use heartwood_cli::doctor::view::Window;
    // 24 hours of 8 s slots are 10,800 blocks: the store is asked for the latest 4,900.
    let mut long = missing(&[], &[7, 113], vec![]);
    long.window = Window {
        from_ms: NOW_MS - 24 * 3_600_000,
        to_ms: NOW_MS,
    };
    let _ = run("slots.missed", &long, &DoctorOptions::default());
    let calls = long.store_calls.borrow();
    assert_eq!(MAX_WINDOW_BLOCKS, 4_900);
    assert_eq!(
        calls.as_slice(),
        [(vec![OURS.to_owned()], TIP - MAX_WINDOW_BLOCKS)]
    );
    // A cut window holds the oldest blocks and every missed row: two missed slots after its last
    // block are not "our last 2 slots".
    let mut cut = missing(&[3, 2], &[400, 300], vec![]);
    if let Ok(store) = &mut cut.store_window {
        store.capped = true;
    }
    let findings = run("slots.missed", &cut, &DoctorOptions::default());
    assert_eq!(findings.len(), 1, "{findings:#?}");
    assert_eq!(findings[0].status, Status::Unknown);
}

#[test]
fn our_delegates_come_from_the_forger_s_lines_since_its_start_before_the_window_too() {
    let mut view = missing(&[], &[7], vec![]);
    let mut window = log(Role::Forger, Vec::new());
    let (at_ms, _) = loaded();
    let kind = LineKind::DelegatesLoaded {
        active: true,
        count: 2,
        names: vec![username(OURS), username("genesis_3")],
    };
    window.roster = vec![LogEvent {
        kind,
        level: Level::Info,
        at_ms: at_ms - 7_200_000,
        offset: 0,
        rotated: false,
    }];
    view.logs.insert(Role::Forger, Ok(window));
    let _ = run("slots.missed", &view, &DoctorOptions::default());
    let calls = view.store_calls.borrow();
    let names: Vec<&Vec<String>> = calls.iter().map(|(names, _)| names).collect();
    assert_eq!(names, [&vec![OURS.to_owned(), "genesis_3".to_owned()]]);
    let active = run("slots.active", &view, &DoctorOptions::default());
    assert_eq!(
        (active.len(), active[0].status, active[0].advice.key),
        (1, Status::Pass, "active")
    );
    assert_eq!(
        active[0].facts,
        [("count", heartwood_cli::doctor::report::Fact::Int(2))]
    );
    // A stopped forger logs no change of the active set: its lines from before the window say
    // nothing of the window.
    view.processes.insert(
        Role::Forger,
        Ok(process(RoleState::Stopped, NOW_MS - 7_200_000)),
    );
    let active = run("slots.active", &view, &DoctorOptions::default());
    assert_eq!(
        (active.len(), active[0].status, active[0].advice.key),
        (1, Status::Unknown, "no-evidence")
    );
}

#[test]
fn a_chain_definition_at_the_limits_of_its_numbers_gives_findings_and_no_panic() {
    let limits = [
        (u32::MAX, u32::MAX),
        (1 << 31, 1 << 31),
        (100_000_000, 100_000_000),
        (u32::MAX, 53),
        (8, u32::MAX),
    ];
    for (block_time_s, active_delegates) in limits {
        let mut view = missing(&[3, 2], &[7, 113], vec![quorum(3), forged(7, TIP - 6)]);
        let facts = ChainFacts {
            block_time_s,
            active_delegates,
            ..chain()
        };
        if let Ok(relay) = &mut view.relay_config {
            relay.chain = facts.clone();
        }
        if let Ok(forger) = &mut view.forger_config {
            forger.chain = facts;
        }
        for entry in ENTRIES.iter() {
            let findings = run(entry.id, &view, &DoctorOptions::default());
            assert!(
                !findings.is_empty(),
                "{} at {block_time_s}/{active_delegates}",
                entry.id
            );
        }
    }
}
