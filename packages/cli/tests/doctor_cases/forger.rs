//! The cases of the forger checks: each names the check, the view, the options and the findings it
//! must give. The registry walk requires a case for each status a built check declares.

use std::path::PathBuf;

use heartwood_cli::Role;
use heartwood_cli::doctor::DoctorOptions;
use heartwood_cli::doctor::docs::docs;
use heartwood_cli::doctor::report::{Basis, Status};
use heartwood_cli::doctor::seams::{FileFacts, FileKind, Meta, RoleState, Subject};
use heartwood_cli::doctor::sources::api::ApiSyncing;
use heartwood_cli::doctor::sources::fs::{SignedEntry, SigningRecordFacts};
use heartwood_cli::doctor::sources::logs::{CaptureWindow, LineKind, StartError};
use heartwood_cli::doctor::sources::procfs::{SocketRow, TcpState};
use heartwood_cli::doctor::view::{NodeFile, Reason, Source};
use heartwood_kernel::log::Level;

use super::Case;
use super::slots::{
    OURS, RUNNING, TIP, absent, addr, at, block, block_id, case, forger, key, log, process, run,
    unknown, username, view, with_log,
};
use crate::doctor_support::{FakeView, NOW_MS};

const F: Subject = Subject::Forger;
/// The slot the fakes' clock falls in.
const SLOT: i64 = (NOW_MS - super::slots::EPOCH_MS) / 8_000;

/// The view with the forger's sockets established to `remotes` (inodes 11 on).
pub fn linked(mut view: FakeView, remotes: &[&str]) -> FakeView {
    let rows = remotes.iter().zip(11..).map(|(remote, inode)| SocketRow {
        local: addr("127.0.0.1:41000"),
        remote: addr(remote),
        state: TcpState::Established,
        uid: 1000,
        inode,
    });
    view.sockets = Ok(rows.collect());
    view.role_inodes.insert(Role::Forger, Ok(vec![11, 12, 13]));
    view
}

fn hosts(list: &[&str]) -> FakeView {
    let mut view = view();
    view.forger_config = Ok(forger(list));
    view
}

fn history(events: Vec<(i64, LineKind)>) -> FakeView {
    linked(with_log(events), &["127.0.0.1:6002"])
}

/// A signing record of our key holding `slot` with block `id`.
pub fn record(slot: i64, id: String) -> SigningRecordFacts {
    SigningRecordFacts {
        known_format: true,
        nethash: block_id(0xab),
        entries: vec![SignedEntry {
            public_key: key(1),
            height: TIP - 19,
            slot,
            block_id: id,
        }],
    }
}

/// "Not going to forge because delegate genesis_1 has already forged on another node", inside the
/// slot 20 slots ago.
fn claim() -> (i64, LineKind) {
    let kind = LineKind::ForgedOnAnotherNode {
        username: username(OURS),
    };
    (at(20) + 2_000, kind)
}

/// The chain's block in that slot, signed with our key.
fn chain_block() -> heartwood_cli::doctor::seams::StoredBlock {
    block(20, TIP - 19, 1, OURS)
}

/// A view with the claim, the chain's block in that slot, and a signing record.
fn claimed(record: Option<SigningRecordFacts>, events: Vec<(i64, LineKind)>) -> FakeView {
    let mut all = vec![claim()];
    all.extend(events);
    let mut view = with_log(all);
    let store = &mut view.store_window;
    if let Ok(store) = store {
        store.blocks = vec![block(21, TIP - 20, 2, "genesis_2"), chain_block()];
    }
    view.signing_record = Ok(record);
    view
}

fn meta(kind: FileKind, mode: u32) -> Meta {
    Meta {
        kind,
        mode,
        uid: 1000,
        gid: 1000,
        size: 512,
        dev: 1,
        ino: 7,
        nlink: 1,
        modified_ms: NOW_MS,
    }
}

/// A signing record of `mode` in a directory of `dir`, holding `record`.
fn signed(mode: u32, dir: u32, record: SigningRecordFacts) -> FakeView {
    let mut view = view();
    let facts = |kind, mode, path: &str| {
        Ok(Some(FileFacts {
            path: PathBuf::from(path),
            own: meta(kind, mode),
            target: None,
            link_target: None,
        }))
    };
    let file = "/nodes/node1/data/forger/signing-record.json";
    view.files
        .insert(NodeFile::SigningRecord, facts(FileKind::File, mode, file));
    let dir_path = "/nodes/node1/data/forger";
    view.files.insert(
        NodeFile::SigningRecordDir,
        facts(FileKind::Dir, dir, dir_path),
    );
    view.signing_record = Ok(Some(record));
    view
}

fn captured(errors: Vec<StartError>, state: RoleState) -> FakeView {
    let mut view = view();
    let capture = CaptureWindow {
        errors,
        other_lines: 0,
        excerpts: Vec::new(),
    };
    view.captures.insert(Role::Forger, Ok(capture));
    view.processes
        .insert(Role::Forger, Ok(process(state, NOW_MS - 60_000)));
    view
}

/// Every case of the area.
pub fn cases() -> Vec<Case> {
    let two = ["127.0.0.1:6002", "127.0.0.2:6002"];
    let mut cases = vec![
        // forger.hosts
        absent("hosts: no forger on this host", "forger.hosts"),
        case(
            "hosts: two on loopback",
            "forger.hosts",
            hosts(&two),
            &[(F, Status::Pass, "pass")],
        ),
        case(
            "hosts: three",
            "forger.hosts",
            hosts(&["127.0.0.1:6002", "127.0.0.2:6002", "127.0.0.3:6002"]),
            &[(F, Status::Pass, "pass")],
        ),
        case(
            "hosts: a later relay on another host",
            "forger.hosts",
            hosts(&["127.0.0.1:6002", "203.0.113.5:6002"]),
            &[(F, Status::Pass, "pass-remote")],
        ),
        case(
            "hosts: a mapped loopback first",
            "forger.hosts",
            hosts(&["[::ffff:127.0.0.1]:6002", "[::1]:6002"]),
            &[(F, Status::Pass, "pass")],
        ),
        case(
            "hosts: one relay",
            "forger.hosts",
            hosts(&["127.0.0.1:6002"]),
            &[(F, Status::Warn, "one-relay")],
        ),
        case(
            "hosts: four relays",
            "forger.hosts",
            hosts(&[
                "127.0.0.1:6002",
                "127.0.0.2:6002",
                "127.0.0.3:6002",
                "127.0.0.4:6002",
            ]),
            &[(F, Status::Warn, "too-many")],
        ),
        case(
            "hosts: the first on another host",
            "forger.hosts",
            hosts(&["203.0.113.5:6002", "127.0.0.1:6002"]),
            &[(F, Status::Warn, "first-not-loopback")],
        ),
        // forger.link
        absent("link: no forger on this host", "forger.link"),
        case(
            "link: to the first relay",
            "forger.link",
            linked(view(), &["127.0.0.1:6002"]),
            &[(F, Status::Pass, "first")],
        ),
        case(
            "link: to the first relay, mapped",
            "forger.link",
            linked(view(), &["[::ffff:127.0.0.1]:6002"]),
            &[(F, Status::Pass, "first")],
        ),
        case(
            "link: failed over",
            "forger.link",
            linked(view(), &["127.0.0.2:6002"]),
            &[(F, Status::Warn, "failover")],
        ),
        case(
            "link: none",
            "forger.link",
            linked(view(), &["127.0.0.1:6003"]),
            &[(F, Status::Fail, "none")],
        ),
        // forger.link-history
        absent("history: no forger on this host", "forger.link-history"),
        case(
            "history: none",
            "forger.link-history",
            history(vec![(at(30), LineKind::NoLocalHosts)]),
            &[(F, Status::Pass, "none")],
        ),
        case(
            "history: waiting for a responsive host",
            "forger.link-history",
            history(vec![(NOW_MS - 1_200_000, LineKind::WaitingForHost)]),
            &[(F, Status::Warn, "failures")],
        ),
        case(
            "history: a broadcast failed",
            "forger.link-history",
            history(vec![(at(30), LineKind::BroadcastFailed)]),
            &[(F, Status::Fail, "broadcast")],
        ),
    ];
    let mut syncing = history(vec![(at(30), LineKind::WaitingForRelayReady)]);
    syncing.api_syncing = Ok(ApiSyncing {
        syncing: true,
        blocks: 900,
        height: TIP,
    });
    cases.push(case(
        "history: waiting while the relay syncs",
        "forger.link-history",
        syncing,
        &[(F, Status::Pass, "none")],
    ));
    let mut ready = history(vec![(at(30), LineKind::WaitingForRelayReady)]);
    ready.api_syncing = Ok(ApiSyncing {
        syncing: false,
        blocks: 0,
        height: TIP,
    });
    cases.push(case(
        "history: waiting for a synced relay",
        "forger.link-history",
        ready,
        &[(F, Status::Warn, "failures")],
    ));
    for (name, back, status, key) in [
        (
            "history: down, a failure 5 minutes ago",
            300_000,
            Status::Fail,
            "down",
        ),
        (
            "history: down, a failure just over 5 minutes ago",
            300_001,
            Status::Warn,
            "failures",
        ),
    ] {
        let failed = vec![(NOW_MS - back, LineKind::RelayRequestFailed)];
        let down = linked(with_log(failed), &[]);
        cases.push(case(name, "forger.link-history", down, &[(F, status, key)]));
    }
    let mut quiet = history(vec![(at(30), LineKind::WaitingForHost)]);
    let mut hidden = super::slots::log(Role::Forger, Vec::new());
    hidden.file_level = Some(Level::Error);
    quiet.logs.insert(Role::Forger, Ok(hidden));
    cases.push(case(
        "history: the log level hides warnings",
        "forger.link-history",
        quiet,
        &[(F, Status::Unknown, "unknown")],
    ));
    let mut warnings = history(Vec::new());
    let mut level = super::slots::log(Role::Forger, Vec::new());
    level.file_level = Some(Level::Warning);
    warnings.logs.insert(Role::Forger, Ok(level));
    cases.push(case(
        "history: the log level hides info lines",
        "forger.link-history",
        warnings,
        &[(F, Status::Unknown, "unknown")],
    ));
    for (name, events, status, key) in [
        (
            "history: nothing logged, the forger does not run",
            Vec::new(),
            Status::Unknown,
            "unknown",
        ),
        (
            "history: a failure logged before the forger stopped",
            vec![(NOW_MS - 1_200_000, LineKind::WaitingForHost)],
            Status::Warn,
            "failures",
        ),
    ] {
        let mut stopped = history(events);
        let facts = process(RoleState::Stopped, NOW_MS - 7_200_000);
        stopped.processes.insert(Role::Forger, Ok(facts));
        let expect = [(F, status, key)];
        cases.push(case(name, "forger.link-history", stopped, &expect));
    }

    let mut refused = view();
    let loader = unknown(Source::Config, Reason::KeysDirUnsafe);
    refused.forger_config = Err(loader);
    cases.push(case(
        "hosts: the configuration not loaded",
        "forger.hosts",
        refused,
        &[(F, Status::Unknown, "unknown")],
    ));
    let mut stopped = view();
    let not_running = unknown(Source::Sockets, Reason::NotRunning);
    stopped.role_inodes.insert(Role::Forger, Err(not_running));
    cases.push(case(
        "link: the forger does not run",
        "forger.link",
        stopped,
        &[(F, Status::Unknown, "unknown")],
    ));

    // forger.one-key-one-forger
    let chain_id = chain_block().id;
    let other = SLOT - 100;
    cases.extend([
        absent(
            "one key: no forger on this host",
            "forger.one-key-one-forger",
        ),
        case(
            "one key: no claim",
            "forger.one-key-one-forger",
            with_log(vec![]),
            &[(F, Status::Pass, "none")],
        ),
        case(
            "one key: the record holds the slot with the chain's block",
            "forger.one-key-one-forger",
            claimed(Some(record(SLOT - 20, chain_id.clone())), vec![]),
            &[(F, Status::Pass, "own-block")],
        ),
        case(
            "one key: the record holds another slot, the log the block",
            "forger.one-key-one-forger",
            claimed(Some(record(other, block_id(9))), vec![forged_at(TIP - 19)]),
            &[(F, Status::Pass, "own-block")],
        ),
        case(
            "one key: the record holds the slot with another block",
            "forger.one-key-one-forger",
            claimed(Some(record(SLOT - 20, block_id(9))), vec![]),
            &[(F, Status::Fail, "another-forger")],
        ),
        case(
            "one key: the record holds another slot, no forged line",
            "forger.one-key-one-forger",
            claimed(Some(record(other, block_id(9))), vec![]),
            &[(F, Status::Warn, "logged-only")],
        ),
        case(
            "one key: no record, no forged line",
            "forger.one-key-one-forger",
            claimed(None, vec![]),
            &[(F, Status::Warn, "logged-only")],
        ),
    ]);
    // The file log level hides the "forged" line: its absence confirms nothing.
    let mut hidden = claimed(Some(record(other, block_id(9))), vec![]);
    if let Some(Ok(log)) = hidden.logs.get_mut(&Role::Forger) {
        log.file_level = Some(Level::Warning);
    }
    cases.push(case(
        "one key: the record holds another slot, the level hides info lines",
        "forger.one-key-one-forger",
        hidden,
        &[(F, Status::Warn, "logged-only")],
    ));
    let mut logged = claimed(Some(record(other, block_id(9))), vec![]);
    if let Ok(store) = &mut logged.store_window {
        store
            .blocks
            .retain(|block| block.username.as_deref() != Some(OURS));
    }
    cases.push(case(
        "one key: the chain has no block of ours then",
        "forger.one-key-one-forger",
        logged,
        &[(F, Status::Warn, "logged-only")],
    ));
    let mut no_store = claimed(Some(record(SLOT - 20, block_id(9))), vec![]);
    no_store.store_window = Err(unknown(Source::Store, Reason::NotRunning));
    cases.push(case(
        "one key: the store is not readable",
        "forger.one-key-one-forger",
        no_store,
        &[(F, Status::Warn, "logged-only")],
    ));
    let mut no_record = claimed(None, vec![]);
    no_record.signing_record = Err(unknown(Source::SigningRecord, Reason::Denied));
    cases.push(case(
        "one key: the record is not readable",
        "forger.one-key-one-forger",
        no_record,
        &[(F, Status::Warn, "logged-only")],
    ));
    let mut over = claimed(Some(record(SLOT - 20, block_id(9))), vec![]);
    let already = LineKind::AlreadyForgedThisRound {
        username: username(OURS),
    };
    over.logs.insert(
        Role::Forger,
        Ok(log(
            Role::Forger,
            vec![super::slots::loaded(), (at(20) + 3_000, already)],
        )),
    );
    cases.push(case(
        "one key: already forged a block, confirmed",
        "forger.one-key-one-forger",
        over,
        &[(F, Status::Fail, "another-forger")],
    ));
    let mut unread = view();
    unread.logs.remove(&Role::Forger);
    cases.push(case(
        "one key: the log unread",
        "forger.one-key-one-forger",
        unread,
        &[(F, Status::Unknown, "unknown")],
    ));

    // forger.signing-record
    let own = || record(SLOT - 20, block_id(1));
    let mut other_network = own();
    other_network.nethash = block_id(0xcd);
    let mut unknown_format = own();
    unknown_format.known_format = false;
    let mut none = view();
    none.files.insert(NodeFile::SigningRecord, Ok(None));
    let mut unparsed = signed(0o600, 0o700, own());
    unparsed.signing_record = Err(unknown(Source::SigningRecord, Reason::Unparsed));
    let mut denied = view();
    denied.files.insert(
        NodeFile::SigningRecord,
        Err(unknown(Source::Files, Reason::Denied)),
    );
    cases.extend([
        absent("record: no forger on this host", "forger.signing-record"),
        case(
            "record: private, this network",
            "forger.signing-record",
            signed(0o600, 0o700, own()),
            &[(F, Status::Pass, "pass")],
        ),
        case(
            "record: read-only",
            "forger.signing-record",
            signed(0o400, 0o700, own()),
            &[(F, Status::Pass, "pass")],
        ),
        case(
            "record: none yet",
            "forger.signing-record",
            none,
            &[(F, Status::Pass, "absent")],
        ),
        case(
            "record: group can read it",
            "forger.signing-record",
            signed(0o640, 0o700, own()),
            &[(F, Status::Fail, "wide-mode")],
        ),
        case(
            "record: others can enter its directory",
            "forger.signing-record",
            signed(0o600, 0o701, own()),
            &[(F, Status::Fail, "wide-mode")],
        ),
        case(
            "record: another network",
            "forger.signing-record",
            signed(0o600, 0o700, other_network),
            &[(F, Status::Fail, "other-network")],
        ),
        case(
            "record: not parsed",
            "forger.signing-record",
            unparsed,
            &[(F, Status::Fail, "unparsed")],
        ),
        case(
            "record: an unknown format",
            "forger.signing-record",
            signed(0o600, 0o700, unknown_format),
            &[(F, Status::Fail, "unparsed")],
        ),
        case(
            "record: a slot ahead of the clock",
            "forger.signing-record",
            signed(0o600, 0o700, record(SLOT + 1, block_id(1))),
            &[(F, Status::Warn, "slot-ahead")],
        ),
        case(
            "record: the current slot",
            "forger.signing-record",
            signed(0o600, 0o700, record(SLOT, block_id(1))),
            &[(F, Status::Pass, "pass")],
        ),
        case(
            "record: its metadata denied",
            "forger.signing-record",
            denied,
            &[(F, Status::Unknown, "unknown")],
        ),
    ]);

    // forger.start-errors
    let open = StartError::KeyFileTooOpen { mode: 0o644 };
    let mut no_capture = view();
    no_capture.captures.insert(
        Role::Forger,
        Err(unknown(Source::ErrorCapture, Reason::Missing)),
    );
    cases.extend([
        absent("start: no forger on this host", "forger.start-errors"),
        case(
            "start: no error",
            "forger.start-errors",
            captured(Vec::new(), RoleState::Stopped),
            &[(F, Status::Pass, "none")],
        ),
        case(
            "start: an old error, running now",
            "forger.start-errors",
            captured(vec![open], RUNNING),
            &[(F, Status::Pass, "none")],
        ),
        case(
            "start: the key file is open",
            "forger.start-errors",
            captured(vec![open], RoleState::Stopped),
            &[(F, Status::Fail, "key-file-open")],
        ),
        case(
            "start: another network's record",
            "forger.start-errors",
            captured(
                vec![StartError::SigningRecordOtherNetwork],
                RoleState::Errored,
            ),
            &[(F, Status::Fail, "record-other-network")],
        ),
        case(
            "start: the capture unreadable",
            "forger.start-errors",
            no_capture,
            &[(F, Status::Unknown, "unknown")],
        ),
    ]);
    cases
}

/// "Delegate genesis_1 forged a new block at height H", inside the claimed slot.
fn forged_at(height: u32) -> (i64, LineKind) {
    let kind = LineKind::Forged {
        username: username(OURS),
        height,
        transactions: 0,
    };
    (at(20) + 1_000, kind)
}

#[test]
fn a_logged_claim_is_a_warn_on_the_log_until_the_chain_confirms_it() {
    let options = DoctorOptions::default();
    let mut alone = claimed(Some(record(SLOT - 100, block_id(9))), vec![]);
    alone.store_window = Ok(Default::default());
    let findings = run("forger.one-key-one-forger", &alone, &options);
    assert_eq!(findings.len(), 1);
    assert_eq!(
        (
            findings[0].status,
            findings[0].basis,
            findings[0].advice.key
        ),
        (Status::Warn, Basis::Log, "logged-only")
    );
    let moved_on = claimed(Some(record(SLOT - 100, block_id(9))), vec![]);
    let findings = run("forger.one-key-one-forger", &moved_on, &options);
    assert_eq!(findings.len(), 1);
    assert_eq!(
        (findings[0].status, findings[0].basis),
        (Status::Warn, Basis::Log)
    );
    let confirmed = claimed(Some(record(SLOT - 20, block_id(9))), vec![]);
    let findings = run("forger.one-key-one-forger", &confirmed, &options);
    assert_eq!(findings.len(), 1);
    assert_eq!(
        (
            findings[0].status,
            findings[0].basis,
            findings[0].advice.key
        ),
        (Status::Fail, Basis::Chain, "another-forger")
    );
}

#[test]
fn a_forger_restarted_inside_a_slot_it_signed_gives_nothing_to_act_on() {
    let restarted = claimed(Some(record(SLOT - 20, chain_block().id)), vec![]);
    let findings = run(
        "forger.one-key-one-forger",
        &restarted,
        &DoctorOptions::default(),
    );
    assert!(
        findings.iter().all(|f| f.status == Status::Pass),
        "{findings:#?}"
    );
}

#[test]
fn every_finding_of_these_areas_has_its_advice_template_and_values_and_no_log_only_fail_is_disruptive()
 {
    let docs = docs().unwrap();
    let areas = ["slots.", "forger.", "quorum."];
    let cases = super::all();
    let ours = cases
        .iter()
        .filter(|case| areas.iter().any(|a| case.check.starts_with(a)));
    let disruptive = ["another-forger", "alternating", "other-network"];
    for case in ours {
        for finding in run(case.check, &case.view, &case.options) {
            let key = finding.advice.key;
            let template = docs.template(finding.id, key);
            let template = template.unwrap_or_else(|| panic!("{}: no template {key}", case.name));
            let mut rest = template;
            while let Some((_, after)) = rest.split_once('{') {
                let (name, after) = after.split_once('}').unwrap();
                let known = finding
                    .advice
                    .values
                    .iter()
                    .any(|(value, _)| *value == name);
                assert!(
                    known,
                    "{}: {key} names {{{name}}} without a value",
                    case.name
                );
                rest = after;
            }
            let log_fail = finding.status == Status::Fail && finding.basis == Basis::Log;
            assert!(
                !(log_fail && disruptive.contains(&key)),
                "{}: {key}",
                case.name
            );
        }
    }
}
