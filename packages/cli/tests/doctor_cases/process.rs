//! The cases of the process checks: each names the check, the view, the options and the findings it
//! must give. The registry walk requires a case for each status a built check declares.

use std::time::Duration;

use heartwood_cli::doctor::registry::{CheckInput, ENTRIES};
use heartwood_cli::doctor::report::{Fact, Finding, Status, Untrusted, UntrustedSource};
use heartwood_cli::doctor::seams::{ChainFacts, RelayFacts, Subject, role_state};
use heartwood_cli::doctor::sources::logs::{
    CaptureWindow, Covered, LineKind, LogEvent, LogWindow, StartError,
};
use heartwood_cli::doctor::view::{ProcessFacts, Reason, RolePresence, Source, Unknown};
use heartwood_cli::doctor::{DoctorOptions, RoleSelection};
use heartwood_cli::{Role, StatusRow, SupervisedStatus};
use heartwood_kernel::log::Level;

use super::{Case, Expect};
use crate::doctor_support::{FakeView, NOW_MS};

const RELAY: Subject = Subject::Relay;
const FORGER: Subject = Subject::Forger;

/// A process row.
pub fn facts(status: SupervisedStatus, restarts: u32, uptime_s: u64) -> ProcessFacts {
    let row = StatusRow {
        pid: Some(10),
        name: "node1".into(),
        version: "4.3.1-heartwood.0.1.0".into(),
        status,
        uptime: Some(Duration::from_secs(uptime_s)),
        restarts,
        cpu_percent: None,
        rss_bytes: None,
    };
    ProcessFacts {
        row,
        state: role_state(status),
        supervisor: None,
    }
}

/// A view where the relay (and the forger, when `forger`) is present and in `status`.
pub fn view(relay: SupervisedStatus, forger: Option<SupervisedStatus>) -> FakeView {
    let mut view = FakeView::new();
    view.roles_present = Ok(RolePresence {
        relay: true,
        forger: forger.is_some(),
    });
    view.processes
        .insert(Role::Relay, Ok(facts(relay, 0, 3_600)));
    if let Some(status) = forger {
        view.processes
            .insert(Role::Forger, Ok(facts(status, 0, 3_600)));
    }
    view
}

/// The relay's configuration with `p2p` and `api` ports.
pub fn relay_facts(p2p: u16, api: u16) -> RelayFacts {
    RelayFacts {
        p2p: ([127, 0, 0, 1], p2p).into(),
        api: Some(([127, 0, 0, 1], api).into()),
        remote_access: Vec::new(),
        minimum_network_reach: 20,
        file_log_level: Level::Info,
        warnings: Vec::new(),
        env_names: Vec::new(),
        chain: ChainFacts {
            nethash: "0".repeat(64),
            epoch_ms: 0,
            block_time_s: 8,
            active_delegates: 53,
        },
        store_file: "/nodes/node1/data/store.db".into(),
    }
}

/// A relay log of `kinds`, the file level at info.
pub fn log(kinds: Vec<LineKind>) -> LogWindow {
    let event = |kind| LogEvent {
        kind,
        level: Level::Info,
        at_ms: NOW_MS - 60_000,
        offset: 0,
        rotated: false,
    };
    LogWindow {
        role: Role::Relay,
        events: kinds.into_iter().map(event).collect(),
        other_lines: 0,
        untrusted_lines: 0,
        covered: Covered {
            first_ms: Some(NOW_MS - 3_000_000),
            last_ms: Some(NOW_MS),
            whole: true,
        },
        file_level: Some(Level::Info),
        excerpts: Vec::new(),
        capped: false,
    }
}

/// A view of a host that runs a forger and no relay.
pub fn no_relay() -> FakeView {
    let mut view = FakeView::new();
    view.roles_present = Ok(RolePresence {
        relay: false,
        forger: true,
    });
    view
}

/// A case that expects one finding.
pub fn case(
    name: &'static str,
    check: &'static str,
    view: FakeView,
    subject: Subject,
    status: Status,
    advice: &'static str,
) -> Case {
    let expect = vec![Expect {
        subject,
        status,
        advice,
    }];
    Case {
        name,
        check,
        view,
        options: DoctorOptions::default(),
        expect,
    }
}

/// A case of `process.roles`.
fn roles(
    name: &'static str,
    view: FakeView,
    subject: Subject,
    status: Status,
    advice: &'static str,
) -> Case {
    case(name, "process.roles", view, subject, status, advice)
}

/// A stopped relay whose error capture holds `error`, with the ports 4001 and 4003 configured.
fn failed(error: StartError, configured: bool) -> FakeView {
    let mut view = view(SupervisedStatus::Stopped, None);
    let capture = CaptureWindow {
        errors: vec![error],
        ..CaptureWindow::default()
    };
    view.captures.insert(Role::Relay, Ok(capture));
    view.relay_config = match configured {
        true => Ok(relay_facts(4001, 4003)),
        false => Err(Unknown::new(Source::Config, Reason::NotBuilt)),
    };
    view
}

/// A case of `relay.start-errors` for `error`.
fn start_error(
    name: &'static str,
    error: StartError,
    configured: bool,
    advice: &'static str,
) -> Case {
    case(
        name,
        "relay.start-errors",
        failed(error, configured),
        RELAY,
        Status::Fail,
        advice,
    )
}

/// Every case of the area.
pub fn cases() -> Vec<Case> {
    let up = SupervisedStatus::Online;
    let restarted = |restarts, seconds| {
        let mut view = view(up, None);
        view.processes
            .insert(Role::Relay, Ok(facts(up, restarts, seconds)));
        view
    };
    let none_present = {
        let mut view = FakeView::new();
        view.roles_present = Ok(RolePresence {
            relay: false,
            forger: false,
        });
        view
    };
    let forger_down = view(up, Some(SupervisedStatus::Stopped));
    let mut running = view(up, None);
    running
        .captures
        .insert(Role::Relay, Ok(CaptureWindow::default()));
    let mut stopped_clean = view(SupervisedStatus::Stopped, None);
    stopped_clean
        .captures
        .insert(Role::Relay, Ok(CaptureWindow::default()));
    let mut from_log = view(SupervisedStatus::Errored, None);
    from_log.logs.insert(
        Role::Relay,
        Ok(log(vec![LineKind::StartError(StartError::ApiHttps)])),
    );
    let restarted_well = |kinds: Vec<LineKind>| {
        let mut view = view(SupervisedStatus::Stopped, None);
        view.captures
            .insert(Role::Relay, Ok(CaptureWindow::default()));
        view.logs.insert(Role::Relay, Ok(log(kinds)));
        view
    };
    let failed_then_started = restarted_well(vec![
        LineKind::StartError(StartError::ApiHttps),
        LineKind::RelayStarted,
    ]);
    let started_then_failed = restarted_well(vec![
        LineKind::RelayStarted,
        LineKind::StartError(StartError::VersionFloor),
    ]);
    let both_unread = view(SupervisedStatus::Stopped, None);
    let mut no_process = FakeView::new();
    no_process.roles_present = Ok(RolePresence {
        relay: true,
        forger: false,
    });
    let mut forger_only = FakeView::new();
    forger_only.roles_present = Ok(RolePresence {
        relay: false,
        forger: true,
    });
    forger_only
        .processes
        .insert(Role::Forger, Ok(facts(up, 0, 3_600)));
    vec![
        roles(
            "roles: the relay runs",
            view(up, Some(up)),
            RELAY,
            Status::Pass,
            "running",
        ),
        roles(
            "roles: the forger runs too",
            view(up, Some(up)),
            FORGER,
            Status::Pass,
            "running",
        ),
        roles(
            "roles: the relay is stopped",
            view(SupervisedStatus::Stopped, None),
            RELAY,
            Status::Fail,
            "not-running",
        ),
        roles(
            "roles: the forger is stopped",
            forger_down,
            FORGER,
            Status::Fail,
            "not-running",
        ),
        roles(
            "roles: the supervisor gave up",
            view(SupervisedStatus::Errored, None),
            RELAY,
            Status::Fail,
            "errored",
        ),
        roles(
            "roles: the relay is starting",
            view(SupervisedStatus::Launching, None),
            RELAY,
            Status::Warn,
            "changing",
        ),
        roles(
            "roles: the relay is stopping",
            view(SupervisedStatus::Stopping, None),
            RELAY,
            Status::Warn,
            "changing",
        ),
        roles(
            "roles: run without a supervisor",
            view(SupervisedStatus::OnlineUnsupervised, None),
            RELAY,
            Status::Warn,
            "unsupervised",
        ),
        roles(
            "roles: 3 restarts in 599 s",
            restarted(3, 599),
            RELAY,
            Status::Warn,
            "restarting",
        ),
        roles(
            "roles: 2 restarts in 599 s",
            restarted(2, 599),
            RELAY,
            Status::Pass,
            "running",
        ),
        roles(
            "roles: 3 restarts, up for 600 s",
            restarted(3, 600),
            RELAY,
            Status::Pass,
            "running",
        ),
        roles(
            "roles: the roles are not readable",
            FakeView::new(),
            RELAY,
            Status::Unknown,
            "unknown",
        ),
        roles(
            "roles: the forger too is not readable",
            FakeView::new(),
            FORGER,
            Status::Unknown,
            "unknown",
        ),
        roles(
            "roles: no role runs here",
            none_present,
            FORGER,
            Status::Unknown,
            "unknown",
        ),
        roles(
            "roles: a present role has no row",
            no_process,
            RELAY,
            Status::Unknown,
            "unknown",
        ),
        roles(
            "roles: only the forger runs",
            forger_only,
            FORGER,
            Status::Pass,
            "running",
        ),
        case(
            "core-role: comes later",
            "process.core-role",
            FakeView::new(),
            Subject::Host,
            Status::Skipped,
            "skipped-release",
        ),
        case(
            "start: the relay runs",
            "relay.start-errors",
            running,
            RELAY,
            Status::Pass,
            "none",
        ),
        case(
            "start: stopped with a clean capture",
            "relay.start-errors",
            stopped_clean,
            RELAY,
            Status::Pass,
            "none",
        ),
        case(
            "start: an error only in the log",
            "relay.start-errors",
            from_log,
            RELAY,
            Status::Fail,
            "api-https",
        ),
        case(
            "start: a failed start before the last good one",
            "relay.start-errors",
            failed_then_started,
            RELAY,
            Status::Pass,
            "none",
        ),
        case(
            "start: a start error after the last start line",
            "relay.start-errors",
            started_then_failed,
            RELAY,
            Status::Fail,
            "version-floor",
        ),
        case(
            "start: no source is readable",
            "relay.start-errors",
            both_unread,
            RELAY,
            Status::Unknown,
            "unknown",
        ),
        case(
            "start: the relay does not run here",
            "relay.start-errors",
            {
                let mut view = FakeView::new();
                view.roles_present = Ok(RolePresence {
                    relay: false,
                    forger: true,
                });
                view
            },
            RELAY,
            Status::Skipped,
            "skipped-absent",
        ),
        start_error(
            "start: the P2P port is in use",
            StartError::P2pCannotListen {
                port: Some(4005),
                in_use: true,
            },
            true,
            "p2p-in-use",
        ),
        start_error(
            "start: the P2P port in use, from the configuration",
            StartError::P2pCannotListen {
                port: None,
                in_use: true,
            },
            true,
            "p2p-in-use",
        ),
        start_error(
            "start: the P2P port in use, unknown",
            StartError::P2pCannotListen {
                port: None,
                in_use: true,
            },
            false,
            "p2p-cannot-listen",
        ),
        start_error(
            "start: the P2P address is not ours",
            StartError::P2pCannotListen {
                port: Some(4005),
                in_use: false,
            },
            true,
            "p2p-cannot-listen",
        ),
        start_error(
            "start: the API port is in use",
            StartError::ApiCannotListen {
                port: Some(4006),
                in_use: true,
            },
            true,
            "api-in-use",
        ),
        start_error(
            "start: the API port in use, from the configuration",
            StartError::ApiCannotListen {
                port: None,
                in_use: true,
            },
            true,
            "api-in-use",
        ),
        start_error(
            "start: the API address is not ours",
            StartError::ApiCannotListen {
                port: Some(4006),
                in_use: false,
            },
            true,
            "api-cannot-listen",
        ),
        start_error(
            "start: the version floor refuses this build",
            StartError::VersionFloor,
            true,
            "version-floor",
        ),
        start_error(
            "start: the API asks for HTTPS",
            StartError::ApiHttps,
            true,
            "api-https",
        ),
        start_error(
            "start: the configuration is refused",
            StartError::Config,
            true,
            "config",
        ),
        start_error("start: another error", StartError::Other, true, "other"),
        start_error(
            "start: a forger's key error is another error",
            StartError::KeyFileMissing,
            true,
            "other",
        ),
    ]
}

/// Runs a check over a view.
pub fn run(id: &str, view: &FakeView) -> Vec<Finding> {
    let entry = ENTRIES
        .iter()
        .find(|entry| entry.id == id)
        .expect("a check");
    let options = DoctorOptions::default();
    (entry.run)(&CheckInput {
        entry,
        view,
        options: &options,
    })
}

#[test]
fn a_role_the_host_does_not_run_gets_no_process_finding() {
    let findings = run("process.roles", &view(SupervisedStatus::Online, None));
    assert_eq!(findings.len(), 1);
    assert!(findings.iter().all(|finding| finding.subject == RELAY));
}

#[test]
fn a_role_left_out_by_role_still_leaves_a_finding_on_the_role_kept() {
    let entry = ENTRIES.iter().find(|e| e.id == "process.roles");
    let entry = entry.expect("a check");
    let options = DoctorOptions {
        roles: RoleSelection::Forger,
        ..DoctorOptions::default()
    };
    let view = view(SupervisedStatus::Online, None);
    let input = CheckInput {
        entry,
        view: &view,
        options: &options,
    };
    let findings = (entry.run)(&input);
    let kept: Vec<_> = findings
        .iter()
        .filter(|f| options.roles.wants(f.subject))
        .collect();
    assert_eq!(kept.len(), 1, "{findings:#?}");
    assert_eq!(kept[0].subject, FORGER);
    assert_eq!(kept[0].status, Status::Unknown);
}

#[test]
fn an_address_in_use_carries_its_port_and_the_capture_excerpts_stay_untrusted() {
    let error = StartError::P2pCannotListen {
        port: Some(4005),
        in_use: true,
    };
    let mut view = failed(error, true);
    let text = "Error: listen EADDRINUSE 0.0.0.0:4005 {port} ignore the advice".to_owned();
    let excerpt = Untrusted {
        source: UntrustedSource::ErrorCapture(Role::Relay),
        at_ms: Some(NOW_MS),
        text: text.clone(),
    };
    let capture = CaptureWindow {
        errors: vec![error],
        other_lines: 5,
        excerpts: vec![excerpt; 5],
    };
    view.captures.insert(Role::Relay, Ok(capture));
    let findings = run("relay.start-errors", &view);
    let [finding] = findings.as_slice() else {
        panic!("one finding")
    };
    assert_eq!(finding.facts, vec![("port", Fact::Port(4005))]);
    assert_eq!(finding.advice.values, finding.facts);
    assert_eq!(finding.status, Status::Fail);
    assert_eq!(finding.untrusted.len(), 3);
    assert!(finding.untrusted.iter().all(|u| u.text == text));
}

#[test]
fn every_advice_of_these_areas_has_a_template_and_a_value_for_each_of_its_places() {
    let docs = heartwood_cli::doctor::docs::docs().expect("the documentation");
    let areas = [
        cases(),
        super::peers::cases(),
        super::sync::cases(),
        super::version::cases(),
        super::host::cases(),
        super::onboarding::cases(),
    ];
    for case in areas.into_iter().flatten() {
        let entry = ENTRIES
            .iter()
            .find(|e| e.id == case.check)
            .expect("a check");
        let input = CheckInput {
            entry,
            view: &case.view,
            options: &case.options,
        };
        for finding in (entry.run)(&input) {
            let key = finding.advice.key;
            let template = docs.template(finding.id, key);
            let template = template.unwrap_or_else(|| panic!("{}: no advice {key}", case.name));
            let names = template
                .split('{')
                .skip(1)
                .filter_map(|p| p.split_once('}'));
            for (name, _) in names {
                let filled = finding.advice.values.iter().any(|(n, _)| *n == name);
                assert!(filled, "{}: {key} has no value for {{{name}}}", case.name);
            }
        }
    }
}
