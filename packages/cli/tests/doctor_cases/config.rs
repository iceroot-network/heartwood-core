//! The cases of the config checks: each names the check, the view, the options and the findings it
//! must give. The registry walk requires a case for each status a built check declares.

use std::path::PathBuf;

use heartwood_cli::doctor::DoctorOptions;
use heartwood_cli::doctor::report::Status::{self, Fail, Pass, Skipped, Unknown as U, Warn};
use heartwood_cli::doctor::report::keys;
use heartwood_cli::doctor::seams::{
    ChainFacts, FileFacts, FileKind, ForgerFacts, Meta, RelayFacts, Subject,
};
use heartwood_cli::doctor::view::Unknown;
use heartwood_cli::doctor::view::{NodeDir, NodeFile, ProcessFacts, Reason, RolePresence, Source};
use heartwood_cli::{Role, StatusRow, SupervisedStatus};
use heartwood_kernel::log::Level;

use super::{Case, Expect};
use crate::doctor_support::FakeView;

/// The doctor's own user in these cases.
const UID: u32 = 1000;

fn meta(kind: FileKind, mode: u32, uid: u32) -> Meta {
    Meta {
        kind,
        mode,
        uid,
        gid: uid,
        size: 64,
        dev: 1,
        ino: 7,
        nlink: 1,
        modified_ms: 0,
    }
}

fn facts(path: &str, own: Meta, target: Option<Meta>) -> FileFacts {
    FileFacts {
        path: PathBuf::from(path),
        own,
        target,
        link_target: target.map(|_| PathBuf::from("/elsewhere/delegates.json")),
    }
}

/// A file of this user with `mode`.
fn file(path: &str, mode: u32) -> FileFacts {
    facts(path, meta(FileKind::File, mode, UID), None)
}

/// A directory of this user with `mode`.
fn dir(path: &str, mode: u32) -> FileFacts {
    facts(path, meta(FileKind::Dir, mode, UID), None)
}

fn chain() -> ChainFacts {
    ChainFacts {
        nethash: "ab".repeat(32),
        epoch_ms: 1_767_225_600_000,
        block_time_s: 8,
        active_delegates: 53,
    }
}

fn relay(warnings: &[&str], level: Level) -> RelayFacts {
    RelayFacts {
        p2p: "127.0.0.1:6002".parse().unwrap(),
        api: Some("127.0.0.1:6003".parse().unwrap()),
        remote_access: Vec::new(),
        minimum_network_reach: 20,
        file_log_level: level,
        warnings: warnings.iter().map(|w| (*w).to_owned()).collect(),
        env_names: vec!["CORE_P2P_PORT".into()],
        chain: chain(),
        store_file: "/nodes/node1/data/database/blockchain.db".into(),
    }
}

fn forger(level: Level) -> ForgerFacts {
    ForgerFacts {
        hosts: vec!["127.0.0.1:6002".parse().unwrap()],
        local_address: None,
        aux_set: false,
        file_log_level: level,
        key_file: "/nodes/node1/config/delegates.json".into(),
        signing_record: "/nodes/node1/data/forger/signing-record.json".into(),
        chain: chain(),
    }
}

/// A process of `role` in `state`, with or without a supervisor's state file.
fn process(state: SupervisedStatus, supervised: bool) -> ProcessFacts {
    let running = matches!(state, SupervisedStatus::Online);
    let row = StatusRow {
        pid: running.then_some(4321),
        name: "solar-testnet".into(),
        version: "4.3.1-heartwood.0.1.0".into(),
        status: state,
        uptime: None,
        restarts: 0,
        cpu_percent: None,
        rss_bytes: None,
    };
    let supervisor = supervised.then(|| heartwood_cli::doctor::seams::SupervisorFacts {
        started_at_ms: None,
        restarts: 0,
        unstable: 0,
        version: "4.3.1-heartwood.0.1.0".into(),
    });
    let state = heartwood_cli::doctor::seams::role_state(state);
    ProcessFacts {
        row,
        state,
        supervisor,
    }
}

/// A view of a host running both roles, as user [`UID`].
fn both() -> FakeView {
    let mut view = FakeView::new();
    view.own_uid = Ok(UID);
    view.roles_present = Ok(RolePresence {
        relay: true,
        forger: true,
    });
    view
}

/// A view of a relay-only host.
fn relay_only() -> FakeView {
    let mut view = both();
    view.roles_present = Ok(RolePresence {
        relay: true,
        forger: false,
    });
    view
}

fn unknown(source: Source, reason: Reason) -> Unknown {
    Unknown::new(source, reason)
}

fn expect(subject: Subject, status: Status, advice: &'static str) -> Expect {
    Expect {
        subject,
        status,
        advice,
    }
}

fn case(name: &'static str, check: &'static str, view: FakeView, expect: Vec<Expect>) -> Case {
    Case {
        name,
        check,
        view,
        options: DoctorOptions::default(),
        expect,
    }
}

const KEY: &str = "/nodes/node1/config/delegates.json";
const RUN: &str = "/nodes/node1/data/run";
const CONFIG: &str = "/nodes/node1/config";
const PARENT: &str = "/nodes/node1";

fn load_cases() -> Vec<Case> {
    use Subject::{Forger as F, Relay as R};
    let mut clean = both();
    clean.relay_config = Ok(relay(&[], Level::Debug));
    clean.forger_config = Ok(forger(Level::Info));
    let mut warned = both();
    warned.relay_config = Ok(relay(&["CORE_API_CACHE is not honoured"], Level::Info));
    warned.forger_config = Ok(forger(Level::Info));
    let mut refused = both();
    refused.relay_config = Err(unknown(Source::Config, Reason::LoaderRefused));
    refused.forger_config = Err(unknown(Source::Config, Reason::LoaderRefused));
    let mut gated = both();
    gated.relay_config = Err(unknown(Source::Config, Reason::KeysDirUnsafe));
    gated.forger_config = Err(unknown(Source::Config, Reason::KeysDirUnsafe));
    let mut absent = relay_only();
    absent.relay_config = Ok(relay(&[], Level::Info));
    let mut not_read = FakeView::new();
    not_read.roles_present = Err(unknown(Source::Process, Reason::Unreadable));
    vec![
        case(
            "both load",
            "config.load",
            clean,
            vec![expect(R, Pass, "loads"), expect(F, Pass, "loads")],
        ),
        case(
            "the relay warns",
            "config.load",
            warned,
            vec![expect(R, Warn, "warnings"), expect(F, Pass, "loads")],
        ),
        case(
            "refused",
            "config.load",
            refused,
            vec![expect(R, Fail, "refused"), expect(F, Fail, "refused")],
        ),
        case(
            "the loaders are gated",
            "config.load",
            gated,
            vec![expect(R, U, keys::UNKNOWN), expect(F, U, keys::UNKNOWN)],
        ),
        case(
            "no forger here",
            "config.load",
            absent,
            vec![
                expect(R, Pass, "loads"),
                expect(F, Skipped, keys::SKIPPED_ABSENT),
            ],
        ),
        case(
            "the roles cannot be read",
            "config.load",
            not_read,
            vec![expect(R, U, keys::UNKNOWN), expect(F, U, keys::UNKNOWN)],
        ),
    ]
}

fn level_cases() -> Vec<Case> {
    use Subject::{Forger as F, Relay as R};
    let mut levels = both();
    levels.relay_config = Ok(relay(&[], Level::Info));
    levels.forger_config = Ok(forger(Level::Notice));
    let mut quiet = both();
    quiet.relay_config = Ok(relay(&[], Level::Error));
    quiet.forger_config = Ok(forger(Level::Debug));
    let mut gated = relay_only();
    gated.relay_config = Err(unknown(Source::Config, Reason::KeysDirUnsafe));
    vec![
        case(
            "info and notice",
            "config.log-level",
            levels,
            vec![expect(R, Pass, "detailed"), expect(F, Warn, "too-high")],
        ),
        case(
            "error and debug",
            "config.log-level",
            quiet,
            vec![expect(R, Warn, "too-high"), expect(F, Pass, "detailed")],
        ),
        case(
            "gated, no forger",
            "config.log-level",
            gated,
            vec![
                expect(R, U, keys::UNKNOWN),
                expect(F, Skipped, keys::SKIPPED_ABSENT),
            ],
        ),
    ]
}

fn run_dir_cases() -> Vec<Case> {
    let host = Subject::Host;
    let with = |facts: Result<Option<FileFacts>, Unknown>| {
        let mut view = both();
        view.files.insert(NodeFile::RunDir, facts);
        view
    };
    let other = facts(RUN, meta(FileKind::Dir, 0o700, UID + 1), None);
    let linked = facts(
        RUN,
        meta(FileKind::Symlink, 0o777, UID),
        Some(meta(FileKind::Dir, 0o700, UID)),
    );
    vec![
        case(
            "private",
            "config.run-dir",
            with(Ok(Some(dir(RUN, 0o700)))),
            vec![expect(host, Pass, "private")],
        ),
        case(
            "a private link target",
            "config.run-dir",
            with(Ok(Some(linked))),
            vec![expect(host, Pass, "private")],
        ),
        case(
            "absent",
            "config.run-dir",
            with(Ok(None)),
            vec![expect(host, Pass, "absent")],
        ),
        case(
            "group readable",
            "config.run-dir",
            with(Ok(Some(dir(RUN, 0o750)))),
            vec![expect(host, Warn, "open")],
        ),
        case(
            "another user's",
            "config.run-dir",
            with(Ok(Some(other))),
            vec![expect(host, Fail, "other-user")],
        ),
        case(
            "unreadable",
            "config.run-dir",
            with(Err(unknown(Source::Files, Reason::Denied))),
            vec![expect(host, U, keys::UNKNOWN)],
        ),
    ]
}

fn key_file_cases() -> Vec<Case> {
    let forger = Subject::Forger;
    let with = |facts: Result<Option<FileFacts>, Unknown>| {
        let mut view = both();
        view.files.insert(NodeFile::KeyFile, facts);
        view
    };
    let key = |mode| Ok(Some(file(KEY, mode)));
    let link = facts(
        KEY,
        meta(FileKind::Symlink, 0o777, UID),
        Some(meta(FileKind::File, 0o600, UID)),
    );
    let open_link = facts(
        KEY,
        meta(FileKind::Symlink, 0o777, UID),
        Some(meta(FileKind::File, 0o644, UID)),
    );
    let dangling = facts(KEY, meta(FileKind::Symlink, 0o777, UID), None);
    let other = facts(KEY, meta(FileKind::File, 0o600, 0), None);
    let directory = dir(KEY, 0o700);
    let mut no_forger = relay_only();
    no_forger.files.insert(NodeFile::KeyFile, Ok(None));
    let mut no_uid = with(key(0o600));
    no_uid.own_uid = Err(unknown(Source::Proc, Reason::Denied));
    let one = |status, advice| vec![expect(forger, status, advice)];
    vec![
        case("0600", "keys.file", with(key(0o600)), one(Pass, "private")),
        case("0400", "keys.file", with(key(0o400)), one(Pass, "private")),
        case(
            "0600, own user unknown",
            "keys.file",
            no_uid,
            one(Pass, "private"),
        ),
        case(
            "a link to a private file",
            "keys.file",
            with(Ok(Some(link))),
            one(Warn, "link"),
        ),
        case(
            "an execute bit for its user",
            "keys.file",
            with(key(0o700)),
            one(Warn, "exec"),
        ),
        case(
            "group readable",
            "keys.file",
            with(key(0o640)),
            one(Fail, "open"),
        ),
        case(
            "group writable",
            "keys.file",
            with(key(0o620)),
            one(Fail, "open"),
        ),
        case(
            "other writable",
            "keys.file",
            with(key(0o602)),
            one(Fail, "open"),
        ),
        case(
            "other executable",
            "keys.file",
            with(key(0o601)),
            one(Fail, "open"),
        ),
        case(
            "a link to an open file",
            "keys.file",
            with(Ok(Some(open_link))),
            one(Fail, "open"),
        ),
        case(
            "a dangling link",
            "keys.file",
            with(Ok(Some(dangling))),
            one(Fail, "not-file"),
        ),
        case(
            "a directory",
            "keys.file",
            with(Ok(Some(directory))),
            one(Fail, "not-file"),
        ),
        case(
            "another user's",
            "keys.file",
            with(Ok(Some(other))),
            one(Fail, "other-user"),
        ),
        case("missing", "keys.file", with(Ok(None)), one(Fail, "missing")),
        case(
            "unreadable",
            "keys.file",
            with(Err(unknown(Source::KeyFiles, Reason::Denied))),
            one(U, keys::UNKNOWN),
        ),
        case(
            "no forger here",
            "keys.file",
            no_forger,
            one(Skipped, keys::SKIPPED_ABSENT),
        ),
    ]
}

fn key_dir_cases() -> Vec<Case> {
    let host = Subject::Host;
    let with = |config: Result<Option<FileFacts>, Unknown>, parent: FileFacts| {
        let mut view = both();
        view.files.insert(NodeFile::Dir(NodeDir::Config), config);
        view.files.insert(NodeFile::ConfigParent, Ok(Some(parent)));
        view
    };
    let one = |status, advice| vec![expect(host, status, advice)];
    let config = |mode| Ok(Some(dir(CONFIG, mode)));
    vec![
        case(
            "0700 in 0755",
            "keys.dir",
            with(config(0o700), dir(PARENT, 0o755)),
            one(Pass, "private"),
        ),
        case(
            "0755 in a sticky 1777",
            "keys.dir",
            with(config(0o755), dir(PARENT, 0o1777)),
            one(Pass, "private"),
        ),
        case(
            "no directory yet",
            "keys.dir",
            with(Ok(None), dir(PARENT, 0o700)),
            one(Pass, "private"),
        ),
        case(
            "group writable",
            "keys.dir",
            with(config(0o770), dir(PARENT, 0o755)),
            one(Fail, "open"),
        ),
        case(
            "other writable",
            "keys.dir",
            with(config(0o702), dir(PARENT, 0o700)),
            one(Fail, "open"),
        ),
        case(
            "a sticky but open directory",
            "keys.dir",
            with(config(0o1777), dir(PARENT, 0o700)),
            one(Fail, "open"),
        ),
        case(
            "an open parent",
            "keys.dir",
            with(config(0o700), dir(PARENT, 0o777)),
            one(Fail, "open"),
        ),
        case(
            "unreadable",
            "keys.dir",
            with(
                Err(unknown(Source::Files, Reason::Denied)),
                dir(PARENT, 0o700),
            ),
            one(U, keys::UNKNOWN),
        ),
    ]
}

fn stray_cases() -> Vec<Case> {
    let relay = Subject::Relay;
    let with = |key: Option<FileFacts>, forger: Option<ProcessFacts>| {
        let mut view = relay_only();
        view.files.insert(NodeFile::KeyFile, Ok(key));
        let forger = forger.ok_or(unknown(Source::Process, Reason::Unreadable));
        view.processes.insert(Role::Forger, forger);
        view
    };
    let one = |status, advice| vec![expect(relay, status, advice)];
    let stopped = || Some(process(SupervisedStatus::Stopped, false));
    let key = || Some(file(KEY, 0o600));
    let mut forger_only = both();
    forger_only.roles_present = Ok(RolePresence {
        relay: false,
        forger: true,
    });
    vec![
        case(
            "no key file",
            "keys.stray",
            with(None, stopped()),
            one(Pass, "none"),
        ),
        case(
            "a running forger",
            "keys.stray",
            with(key(), Some(process(SupervisedStatus::Online, true))),
            one(Pass, "forger"),
        ),
        case(
            "a forger that ran",
            "keys.stray",
            with(key(), Some(process(SupervisedStatus::Stopped, true))),
            one(Pass, "forger"),
        ),
        case(
            "a stray key",
            "keys.stray",
            with(key(), stopped()),
            one(Warn, "stray"),
        ),
        case(
            "the process table unreadable",
            "keys.stray",
            with(key(), None),
            one(U, keys::UNKNOWN),
        ),
        case(
            "no relay here",
            "keys.stray",
            forger_only,
            one(Skipped, keys::SKIPPED_ABSENT),
        ),
    ]
}

/// Every case of the area.
pub fn cases() -> Vec<Case> {
    [
        load_cases(),
        level_cases(),
        run_dir_cases(),
        key_file_cases(),
        key_dir_cases(),
        stray_cases(),
    ]
    .into_iter()
    .flatten()
    .collect()
}
