//! The cases of the quorum checks: each names the check, the view, the options and the findings it
//! must give. The registry walk requires a case for each status a built check declares.

use std::net::IpAddr;

use heartwood_cli::Role;
use heartwood_cli::doctor::DoctorOptions;
use heartwood_cli::doctor::report::{Basis, Status};
use heartwood_cli::doctor::seams::{RoleState, Subject};
use heartwood_cli::doctor::sources::api::ApiPeers;
use heartwood_cli::doctor::sources::logs::LineKind;
use heartwood_cli::doctor::sources::procfs::{SocketRow, TcpState};
use heartwood_cli::doctor::view::{Reason, Source};
use heartwood_kernel::log::Level;

use super::Case;
use super::forger::linked;
use super::slots::{
    OURS, RUNNING, absent, addr, at, case, forger, log, process, run, unknown, username, view,
    with_log,
};
use crate::doctor_support::FakeView;

const F: Subject = Subject::Forger;
const R: Subject = Subject::Relay;

/// A line inside the slot `back` slots ago.
fn inside(back: i64, kind: LineKind) -> (i64, LineKind) {
    (at(back) + 1_000, kind)
}

fn forged(back: i64) -> (i64, LineKind) {
    let kind = LineKind::Forged {
        username: username(OURS),
        height: 9_000,
        transactions: 0,
    };
    inside(back, kind)
}

fn refused(back: i64) -> (i64, LineKind) {
    inside(back, LineKind::NotEnoughQuorum)
}

fn peers(total: u32) -> FakeView {
    let mut view = with_log(vec![forged(63), inside(10, LineKind::NotPeeredEnough)]);
    view.api_peers = Ok(ApiPeers {
        total_count: total,
        peers: Vec::new(),
    });
    view
}

fn ip(text: &str) -> IpAddr {
    text.parse().unwrap()
}

/// "A new attestation session from A replaces the session from B", for each pair.
fn sessions(pairs: &[(&str, &str)], starts: usize) -> FakeView {
    let started = (0..starts).map(|n| (at(300) + n as i64, LineKind::ForgerStarted));
    let mut view = linked(with_log(started.collect()), &["127.0.0.1:6002"]);
    let replaced = pairs.iter().zip(1..).map(|((new, old), n)| {
        let kind = LineKind::AttestationSessionReplaced {
            new: ip(new),
            old: ip(old),
        };
        (at(200) + n, kind)
    });
    view.logs
        .insert(Role::Relay, Ok(log(Role::Relay, replaced.collect())));
    view
}

/// Connections from `remotes` to the relay's P2P port, added to the socket table.
fn connected(mut view: FakeView, remotes: &[&str]) -> FakeView {
    if let Ok(rows) = &mut view.sockets {
        rows.extend(remotes.iter().zip(50..).map(|(remote, inode)| SocketRow {
            local: addr("127.0.0.1:6002"),
            remote: addr(remote),
            state: TcpState::Established,
            uid: 1000,
            inode,
        }));
    }
    view
}

const TURNS: [(&str, &str); 2] = [("127.0.0.2", "127.0.0.3"), ("127.0.0.3", "127.0.0.2")];

/// Every case of the area.
pub fn cases() -> Vec<Case> {
    let check = "quorum.forging-allowed";
    let mut early = with_log(vec![inside(10, LineKind::ColdStartSkip)]);
    early
        .processes
        .insert(Role::Relay, Ok(process(RUNNING, at(30))));
    let mut hidden = view();
    let mut level = log(Role::Forger, vec![refused(10)]);
    level.file_level = Some(Level::Warning);
    hidden.logs.insert(Role::Forger, Ok(level));
    let mut cases = vec![
        absent("forging: no forger on this host", check),
        case(
            "forging: no refusal",
            check,
            with_log(vec![forged(63), forged(10)]),
            &[(F, Status::Pass, "none")],
        ),
        case(
            "forging: one refusal",
            check,
            with_log(vec![refused(63), forged(10)]),
            &[(F, Status::Warn, "quorum")],
        ),
        case(
            "forging: refusals, not at the last 2 slots",
            check,
            with_log(vec![refused(116), refused(63), forged(10)]),
            &[(F, Status::Warn, "quorum")],
        ),
        case(
            "forging: refusals at our last 2 slots",
            check,
            with_log(vec![forged(116), refused(63), refused(10)]),
            &[(F, Status::Fail, "quorum")],
        ),
        case(
            "forging: not peered, too few peers",
            check,
            peers(19),
            &[(F, Status::Warn, "not-peered-few-peers")],
        ),
        case(
            "forging: not peered, enough peers",
            check,
            peers(20),
            &[(F, Status::Warn, "not-peered-network")],
        ),
        case(
            "forging: not peered, peers unknown",
            check,
            with_log(vec![inside(10, LineKind::NotPeeredEnough)]),
            &[(F, Status::Warn, "not-peered")],
        ),
        case(
            "forging: a cold start long after the relay started",
            check,
            with_log(vec![inside(10, LineKind::ColdStartSkip)]),
            &[(F, Status::Warn, "cold-start")],
        ),
        case(
            "forging: a cold start in the relay's first round",
            check,
            early,
            &[(F, Status::Pass, "none")],
        ),
        case(
            "forging: no network state",
            check,
            with_log(vec![inside(10, LineKind::NoNetworkState)]),
            &[(F, Status::Warn, "no-network-state")],
        ),
        case(
            "forging: over-height headers",
            check,
            with_log(vec![inside(10, LineKind::OverheightHeaders { count: 2 })]),
            &[(F, Status::Warn, "overheight")],
        ),
        case(
            "forging: heights out of sync",
            check,
            with_log(vec![inside(
                10,
                LineKind::HeightsOutOfSync {
                    ours: 9,
                    theirs: 10,
                },
            )]),
            &[(F, Status::Warn, "out-of-sync")],
        ),
        case(
            "forging: the log level hides info",
            check,
            hidden,
            &[(F, Status::Unknown, "unknown")],
        ),
    ];
    for (name, events, status, key) in [
        (
            "forging: nothing logged, the forger does not run",
            Vec::new(),
            Status::Unknown,
            "unknown",
        ),
        (
            "forging: a refusal logged before the forger stopped",
            vec![refused(63)],
            Status::Warn,
            "quorum",
        ),
    ] {
        let mut stopped = with_log(events);
        let facts = process(RoleState::Stopped, at(400));
        stopped.processes.insert(Role::Forger, Ok(facts));
        cases.push(case(name, check, stopped, &[(F, status, key)]));
    }

    // quorum.attestation
    let check = "quorum.attestation";
    let mut remote = view();
    remote.forger_config = Ok(forger(&["203.0.113.7:6002", "127.0.0.1:6002"]));
    let remote = linked(remote, &["203.0.113.7:6002"]);
    let not_attesting = LineKind::NotAttesting { far_slot: true };
    let mut stopped = view();
    stopped.role_inodes.insert(
        Role::Forger,
        Err(unknown(Source::Sockets, Reason::NotRunning)),
    );
    cases.extend([
        absent("attestation: no forger on this host", check),
        case(
            "attestation: linked on loopback",
            check,
            linked(view(), &["127.0.0.1:6002"]),
            &[(F, Status::Pass, "attested")],
        ),
        case(
            "attestation: linked to another host's relay",
            check,
            remote,
            &[(F, Status::Warn, "remote-relay")],
        ),
        case(
            "attestation: no link",
            check,
            linked(view(), &[]),
            &[(F, Status::Fail, "no-link")],
        ),
        case(
            "attestation: not attesting",
            check,
            linked(with_log(vec![(at(40), not_attesting)]), &["127.0.0.1:6002"]),
            &[(F, Status::Warn, "not-attesting")],
        ),
        case(
            "attestation: one replacement per start",
            check,
            sessions(&[("127.0.0.2", "127.0.0.1")], 1),
            &[(F, Status::Pass, "attested")],
        ),
        case(
            "attestation: more replacements than starts",
            check,
            sessions(&[("127.0.0.2", "127.0.0.1"), ("127.0.0.3", "127.0.0.2")], 1),
            &[(R, Status::Warn, "replaced")],
        ),
        case(
            "attestation: taking turns, logged only",
            check,
            sessions(&TURNS, 0),
            &[(R, Status::Warn, "alternating-logged")],
        ),
        case(
            "attestation: taking turns, one of them connected",
            check,
            connected(sessions(&TURNS, 0), &["127.0.0.2:50100"]),
            &[(R, Status::Warn, "alternating-logged")],
        ),
        case(
            "attestation: taking turns, both connected",
            check,
            connected(sessions(&TURNS, 0), &["127.0.0.2:50100", "127.0.0.3:50200"]),
            &[(R, Status::Fail, "alternating")],
        ),
        case(
            "attestation: the forger does not run",
            check,
            stopped,
            &[(F, Status::Unknown, "unknown")],
        ),
        case(
            "reach: in a later release",
            "quorum.reach",
            view(),
            &[(R, Status::Skipped, "skipped-release")],
        ),
    ]);
    cases
}

#[test]
fn sessions_taking_turns_are_a_warn_on_the_log_until_the_socket_table_shows_both_forgers() {
    let options = DoctorOptions::default();
    let logged = run("quorum.attestation", &sessions(&TURNS, 0), &options);
    let relay: Vec<_> = logged.iter().filter(|f| f.subject == R).collect();
    assert_eq!(relay.len(), 1);
    assert_eq!(
        (relay[0].status, relay[0].basis),
        (Status::Warn, Basis::Log)
    );
    let both = connected(sessions(&TURNS, 0), &["127.0.0.2:50100", "127.0.0.3:50200"]);
    let confirmed = run("quorum.attestation", &both, &options);
    let relay: Vec<_> = confirmed.iter().filter(|f| f.subject == R).collect();
    assert_eq!(relay.len(), 1);
    assert_eq!(
        (relay[0].status, relay[0].basis),
        (Status::Fail, Basis::Socket)
    );
    // A peer's connections from other hosts confirm nothing.
    let public = connected(
        sessions(&TURNS, 0),
        &["198.51.100.2:50100", "198.51.100.3:50200"],
    );
    let findings = run("quorum.attestation", &public, &options);
    assert!(
        findings.iter().all(|f| f.status != Status::Fail),
        "{findings:#?}"
    );
}
