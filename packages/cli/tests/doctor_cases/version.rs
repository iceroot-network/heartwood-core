//! The cases of the version checks: each names the check, the view, the options and the findings it
//! must give. The registry walk requires a case for each status a built check declares.

use heartwood_cli::doctor::report::{Fact, Status, UntrustedSource};
use heartwood_cli::doctor::seams::{FloorFacts, Subject, SupervisorFacts};
use heartwood_cli::doctor::sources::api::{ApiBlock, ApiConfiguration, ApiPeer, ApiStatus};
use heartwood_cli::doctor::view::{Reason, RolePresence, Source, Unknown};
use heartwood_cli::{Role, SupervisedStatus};

use super::Case;
use super::peers::{peer, with_peers};
use super::process::{case, facts, relay_facts, run, view};
use crate::doctor_support::FakeView;

const RELAY: Subject = Subject::Relay;
const FORGER: Subject = Subject::Forger;
const BUILD: &str = "4.3.1-heartwood.0.1.0";

/// The floor's verdict for the build at `height` 500: refused now, or from the heights.
fn floors(refused_now: bool, refused_from: Vec<u32>) -> Result<FloorFacts, Unknown> {
    Ok(FloorFacts {
        refused_now,
        refused_from,
    })
}

/// A view whose relay stands at height 500 and whose floor says `verdict` about the build.
fn floored(verdict: Result<FloorFacts, Unknown>) -> FakeView {
    let mut view = view(SupervisedStatus::Online, None);
    view.api_status = Ok(ApiStatus {
        synced: true,
        now: 500,
        blocks_count: 0,
    });
    view.floors.insert((BUILD.into(), 500), verdict);
    view
}

/// `version.floor` over `view`.
fn floor(name: &'static str, view: FakeView, status: Status, advice: &'static str) -> Case {
    case(name, "version.floor", view, RELAY, status, advice)
}

/// A view whose roles are `relay` and `forger` at the versions given (`None`: not present).
fn running(relay: Option<&str>, forger: Option<&str>) -> FakeView {
    let mut view = FakeView::new();
    view.roles_present = Ok(RolePresence {
        relay: relay.is_some(),
        forger: forger.is_some(),
    });
    for (role, version) in [(Role::Relay, relay), (Role::Forger, forger)] {
        let Some(version) = version else { continue };
        let mut process = facts(SupervisedStatus::Online, 0, 3_600);
        process.row.version = version.into();
        view.processes.insert(role, Ok(process));
    }
    view
}

/// `version.installed` about `subject`.
fn installed(
    name: &'static str,
    view: FakeView,
    subject: Subject,
    status: Status,
    advice: &'static str,
) -> Case {
    case(name, "version.installed", view, subject, status, advice)
}

/// A view whose peers run `versions`.
fn peered(versions: &[Option<&str>]) -> FakeView {
    let peers: Vec<ApiPeer> = versions
        .iter()
        .enumerate()
        .map(|(n, version)| ApiPeer {
            version: version.map(str::to_owned),
            ..peer(&format!("203.0.113.{}", n + 1), Some(100))
        })
        .collect();
    let total = u32::try_from(peers.len()).unwrap_or(0);
    with_peers(peers, total, 100)
}

/// `version.peers` over `view`.
fn peers(name: &'static str, view: FakeView, status: Status, advice: &'static str) -> Case {
    case(name, "version.peers", view, RELAY, status, advice)
}

/// A view that is built with `features`.
fn built(features: &[&'static str]) -> FakeView {
    let mut view = FakeView::new();
    view.build.features = features.to_vec();
    view
}

/// Every case of the area.
pub fn cases() -> Vec<Case> {
    let newer = Some("4.4.0-heartwood.0.1.0");
    let same = Some(BUILD);
    let older = Some("4.2.9-heartwood.0.1.0");
    let mut api_config = floored(floors(false, vec![30_000]));
    api_config.api_configuration = Ok(ApiConfiguration {
        core_version: None,
        nethash: None,
        block_time_s: Some(8),
        active_delegates: None,
    });
    let mut chain_config = floored(floors(false, vec![30_000]));
    chain_config.relay_config = Ok(relay_facts(4001, 4003));
    let mut from_tip = floored(floors(false, Vec::new()));
    from_tip.api_status = Err(Unknown::new(Source::Api, Reason::NotBuilt));
    from_tip.api_last_block = Ok(ApiBlock {
        id: "0".repeat(64),
        height: 500,
        timestamp: None,
        generator_public_key: None,
        username: None,
    });
    let mut not_here = FakeView::new();
    not_here.roles_present = Ok(RolePresence {
        relay: false,
        forger: true,
    });
    let mut by_api = running(Some("4.0.0"), None);
    by_api.api_configuration = Ok(ApiConfiguration {
        core_version: same.map(str::to_owned),
        nethash: None,
        block_time_s: None,
        active_delegates: None,
    });
    let mut recorded = floored(floors(true, Vec::new()));
    recorded
        .processes
        .get_mut(&Role::Relay)
        .and_then(|p| p.as_mut().ok())
        .expect("a process")
        .supervisor = Some(SupervisorFacts {
        started_at_ms: None,
        restarts: 0,
        unstable: 0,
        version: "4.2.0".into(),
    });
    recorded
        .floors
        .insert(("4.2.0".into(), 500), floors(true, Vec::new()));
    let mut stopped = running(same, None);
    stopped
        .processes
        .insert(Role::Relay, Ok(facts(SupervisedStatus::Stopped, 0, 0)));
    vec![
        floor(
            "floor: the version is accepted",
            floored(floors(false, Vec::new())),
            Status::Pass,
            "ok",
        ),
        floor(
            "floor: the version is refused now",
            floored(floors(true, Vec::new())),
            Status::Fail,
            "refused",
        ),
        floor(
            "floor: refused now and later",
            floored(floors(true, vec![30_000])),
            Status::Fail,
            "refused",
        ),
        floor(
            "floor: a later milestone refuses the version",
            api_config,
            Status::Warn,
            "later",
        ),
        floor(
            "floor: later, by the configured block time",
            chain_config,
            Status::Warn,
            "later",
        ),
        floor(
            "floor: later, no block time known",
            floored(floors(false, vec![30_000])),
            Status::Warn,
            "later-height",
        ),
        floor(
            "floor: the height from the last block",
            from_tip,
            Status::Pass,
            "ok",
        ),
        floor(
            "floor: the version the supervisor recorded",
            recorded,
            Status::Fail,
            "refused",
        ),
        floor(
            "floor: the floor is unknown",
            floored(Err(Unknown::new(Source::Config, Reason::Missing))),
            Status::Unknown,
            "unknown",
        ),
        floor(
            "floor: no height known",
            FakeView::new(),
            Status::Unknown,
            "unknown",
        ),
        floor(
            "floor: no relay here",
            not_here,
            Status::Skipped,
            "skipped-absent",
        ),
        installed(
            "installed: the relay runs this build",
            running(same, None),
            RELAY,
            Status::Pass,
            "same",
        ),
        installed(
            "installed: the forger runs this build",
            running(same, same),
            FORGER,
            Status::Pass,
            "same",
        ),
        installed(
            "installed: the relay is an older build",
            running(older, None),
            RELAY,
            Status::Warn,
            "restart-pending",
        ),
        installed(
            "installed: the forger is an older build",
            running(same, older),
            FORGER,
            Status::Warn,
            "restart-pending",
        ),
        installed(
            "installed: the relay is a newer build",
            running(newer, None),
            RELAY,
            Status::Warn,
            "restart-pending",
        ),
        installed(
            "installed: the API's version wins",
            by_api,
            RELAY,
            Status::Pass,
            "same",
        ),
        installed(
            "installed: the relay is stopped",
            stopped,
            RELAY,
            Status::Skipped,
            "skipped-absent",
        ),
        installed(
            "installed: no forger here",
            running(same, None),
            FORGER,
            Status::Skipped,
            "skipped-absent",
        ),
        installed(
            "installed: the roles are not readable",
            FakeView::new(),
            RELAY,
            Status::Unknown,
            "unknown",
        ),
        installed(
            "installed: a version that is not one",
            running(Some("not a version"), None),
            RELAY,
            Status::Unknown,
            "unknown",
        ),
        case(
            "build: a devnet build",
            "version.build",
            built(&["devnet"]),
            Subject::Host,
            Status::Pass,
            "release-build",
        ),
        case(
            "build: a release build",
            "version.build",
            built(&[]),
            Subject::Host,
            Status::Pass,
            "release-build",
        ),
        case(
            "build: a difftest build",
            "version.build",
            built(&["difftest"]),
            Subject::Host,
            Status::Fail,
            "test-build",
        ),
        case(
            "build: a difftest devnet build",
            "version.build",
            built(&["devnet", "difftest"]),
            Subject::Host,
            Status::Fail,
            "test-build",
        ),
        peers(
            "peers: all current",
            peered(&[same, same, older]),
            Status::Pass,
            "current",
        ),
        peers(
            "peers: half newer",
            peered(&[newer, newer, same, older]),
            Status::Pass,
            "current",
        ),
        peers(
            "peers: most newer",
            peered(&[newer, newer, newer, same]),
            Status::Warn,
            "mostly-newer",
        ),
        peers(
            "peers: the only peer is newer",
            peered(&[newer]),
            Status::Warn,
            "mostly-newer",
        ),
        peers(
            "peers: peers that say nothing count in the total",
            peered(&[newer, None, None]),
            Status::Pass,
            "current",
        ),
        peers("peers: no peers", peered(&[]), Status::Unknown, "unknown"),
        peers(
            "peers: peers not readable",
            FakeView::new(),
            Status::Unknown,
            "unknown",
        ),
        peers(
            "peers: no relay here",
            {
                let mut view = FakeView::new();
                view.roles_present = Ok(RolePresence {
                    relay: false,
                    forger: true,
                });
                view
            },
            Status::Skipped,
            "skipped-absent",
        ),
    ]
}

#[test]
fn a_later_milestone_says_when_it_starts() {
    let mut view = floored(floors(false, vec![30_000]));
    view.api_configuration = Ok(ApiConfiguration {
        core_version: None,
        nethash: None,
        block_time_s: Some(8),
        active_delegates: None,
    });
    let findings = run("version.floor", &view);
    let [finding] = findings.as_slice() else {
        panic!("one finding")
    };
    assert!(finding.facts.contains(&("from", Fact::Int(30_000))));
    assert!(finding.facts.contains(&("days", Fact::Int(2))));
    assert!(finding.facts.contains(&("hours", Fact::Int(17))));
}

#[test]
fn the_build_check_names_its_features() {
    let findings = run("version.build", &built(&["devnet"]));
    let [finding] = findings.as_slice() else {
        panic!("one finding")
    };
    assert!(finding.facts.contains(&("devnet", Fact::Bool(true))));
    assert!(finding.facts.contains(&("difftest", Fact::Bool(false))));
}

#[test]
fn a_peer_version_that_is_not_one_is_untrusted_text_and_no_newer_release() {
    let view = peered(&[Some("9.9.9-evil version"), Some(BUILD), Some("")]);
    let findings = run("version.peers", &view);
    let [finding] = findings.as_slice() else {
        panic!("one finding")
    };
    assert_eq!(finding.status, Status::Pass);
    assert_eq!(finding.untrusted.len(), 1);
    assert!(
        finding
            .untrusted
            .iter()
            .all(|t| t.source == UntrustedSource::PeerVersion)
    );
}
