//! The cases of the peer checks: each names the check, the view, the options and the findings it
//! must give. The registry walk requires a case for each status a built check declares.

use std::net::IpAddr;

use heartwood_cli::doctor::report::{Fact, Status};
use heartwood_cli::doctor::seams::Subject;
use heartwood_cli::doctor::sources::api::{ApiPeer, ApiPeers, ApiStatus};
use heartwood_cli::doctor::sources::logs::LineKind;
use heartwood_cli::doctor::view::{Reason, Source, Unknown};
use heartwood_cli::{Role, SupervisedStatus};
use heartwood_kernel::log::Level;

use super::Case;
use super::process::{case, log, no_relay, relay_facts, run, view};
use crate::doctor_support::FakeView;

const RELAY: Subject = Subject::Relay;

/// A peer at `ip` with `height`.
pub fn peer(ip: &str, height: Option<u32>) -> ApiPeer {
    let ip: IpAddr = ip.parse().expect("an address");
    ApiPeer {
        ip,
        port: 4001,
        height,
        version: None,
    }
}

/// A view whose relay runs, has `peers` (and reports `total` of them) and stands at `now`.
pub fn with_peers(peers: Vec<ApiPeer>, total: u32, now: u32) -> FakeView {
    let mut view = view(SupervisedStatus::Online, None);
    view.api_peers = Ok(ApiPeers {
        total_count: total,
        peers,
    });
    view.api_status = Ok(ApiStatus {
        synced: true,
        now,
        blocks_count: 0,
    });
    view.relay_config = Ok(relay_facts(4001, 4003));
    view
}

/// A view whose one peer stands at `median` while the relay stands at `now`.
fn at(median: u32, now: u32) -> FakeView {
    with_peers(vec![peer("203.0.113.5", Some(median))], 1, now)
}

/// `peers.count` over `view`.
fn count(name: &'static str, view: FakeView, status: Status, advice: &'static str) -> Case {
    case(name, "peers.count", view, RELAY, status, advice)
}

/// `peers.height` over `view`.
fn height(name: &'static str, view: FakeView, status: Status, advice: &'static str) -> Case {
    case(name, "peers.height", view, RELAY, status, advice)
}

/// `peers.diversity` over `n` peers, each at one of the addresses.
fn spread(name: &'static str, ips: &[&str], status: Status, advice: &'static str) -> Case {
    let peers = ips.iter().map(|ip| peer(ip, Some(100))).collect();
    let view = with_peers(peers, u32::try_from(ips.len()).unwrap_or(0), 100);
    case(name, "peers.diversity", view, RELAY, status, advice)
}

/// A view whose relay is far behind, with `kinds` in its log.
fn far(kinds: Vec<LineKind>) -> FakeView {
    let mut view = at(1_000, 100);
    view.logs.insert(Role::Relay, Ok(log(kinds)));
    view
}

/// Every case of the area.
pub fn cases() -> Vec<Case> {
    let mut low_level = far(vec![LineKind::InSync]);
    let mut window = log(vec![LineKind::InSync]);
    window.file_level = Some(Level::Warning);
    low_level.logs.insert(Role::Relay, Ok(window));
    let mut no_config = with_peers(Vec::new(), 30, 100);
    no_config.relay_config = Err(Unknown::new(Source::Config, Reason::NotBuilt));
    let mut no_status = at(100, 100);
    no_status.api_status = Err(Unknown::new(Source::Api, Reason::NotBuilt));
    let unheight = with_peers(vec![peer("203.0.113.5", None)], 1, 100);
    let restarted = || {
        let kinds = vec![LineKind::InSync, LineKind::RelayStarted];
        far(kinds)
    };
    vec![
        count(
            "count: no peers",
            with_peers(Vec::new(), 0, 100),
            Status::Fail,
            "none",
        ),
        count(
            "count: 19 of 20 wanted",
            with_peers(Vec::new(), 19, 100),
            Status::Warn,
            "few",
        ),
        count(
            "count: 20 of 20 wanted",
            with_peers(Vec::new(), 20, 100),
            Status::Pass,
            "enough",
        ),
        count(
            "count: 1 peer",
            with_peers(Vec::new(), 1, 100),
            Status::Warn,
            "few",
        ),
        count(
            "count: no configuration to say how many",
            no_config,
            Status::Unknown,
            "unknown",
        ),
        count(
            "count: peers not readable",
            FakeView::new(),
            Status::Unknown,
            "unknown",
        ),
        count(
            "count: no relay here",
            no_relay(),
            Status::Skipped,
            "skipped-absent",
        ),
        height(
            "height: no relay here",
            no_relay(),
            Status::Skipped,
            "skipped-absent",
        ),
        case(
            "diversity: peers not readable",
            "peers.diversity",
            FakeView::new(),
            RELAY,
            Status::Unknown,
            "unknown",
        ),
        height(
            "height: level with the peers",
            at(100, 100),
            Status::Pass,
            "close",
        ),
        height("height: 2 behind", at(102, 100), Status::Pass, "close"),
        height("height: 2 ahead", at(98, 100), Status::Pass, "close"),
        height("height: 3 behind", at(103, 100), Status::Warn, "behind"),
        height("height: 3 ahead", at(97, 100), Status::Warn, "ahead"),
        height("height: 52 behind", at(152, 100), Status::Warn, "behind"),
        height(
            "height: 53 behind, synced once",
            far(vec![LineKind::InSync]),
            Status::Fail,
            "far-behind",
        ),
        height(
            "height: 53 behind, never synced",
            far(Vec::new()),
            Status::Warn,
            "first-sync",
        ),
        height(
            "height: in sync before the last start only",
            restarted(),
            Status::Warn,
            "first-sync",
        ),
        height(
            "height: 53 behind, no log",
            at(153, 100),
            Status::Warn,
            "behind",
        ),
        height(
            "height: 53 behind, the log skips info",
            low_level,
            Status::Warn,
            "behind",
        ),
        height(
            "height: no peer reports a height",
            unheight,
            Status::Unknown,
            "unknown",
        ),
        height(
            "height: the relay's height is unknown",
            no_status,
            Status::Unknown,
            "unknown",
        ),
        spread(
            "diversity: fewer than 4 public peers",
            &["203.0.113.1", "203.0.113.2", "198.51.100.1"],
            Status::Skipped,
            "skipped-few",
        ),
        spread(
            "diversity: 3 in one network of 4",
            &["203.0.113.1", "203.0.113.2", "203.0.113.3", "198.51.100.1"],
            Status::Warn,
            "shared",
        ),
        spread(
            "diversity: half in one network",
            &["203.0.113.1", "203.0.113.2", "198.51.100.1", "192.0.2.1"],
            Status::Pass,
            "spread",
        ),
        spread(
            "diversity: loopback and private peers do not count",
            &[
                "127.0.0.1",
                "10.0.0.1",
                "192.168.1.1",
                "172.16.0.1",
                "203.0.113.1",
            ],
            Status::Skipped,
            "skipped-few",
        ),
        spread(
            "diversity: shared and link-local peers do not count",
            &[
                "100.64.0.1",
                "169.254.1.1",
                "0.0.0.0",
                "::1",
                "fd00::1",
                "fe80::1",
            ],
            Status::Skipped,
            "skipped-few",
        ),
        spread(
            "diversity: an IPv6 network groups by its /48",
            &[
                "2001:db8:1::1",
                "2001:db8:1:2::1",
                "2001:db8:1:3::1",
                "2001:db8:2::1",
            ],
            Status::Warn,
            "shared",
        ),
        spread(
            "diversity: an IPv4-mapped address groups as IPv4",
            &[
                "::ffff:203.0.113.1",
                "203.0.113.2",
                "198.51.100.1",
                "192.0.2.1",
            ],
            Status::Pass,
            "spread",
        ),
    ]
}

#[test]
fn the_diversity_groups_come_before_any_redaction() {
    let ips = ["203.0.113.1", "203.0.113.2", "203.0.113.3", "198.51.100.1"];
    let peers = ips.iter().map(|ip| peer(ip, Some(100))).collect();
    let findings = run("peers.diversity", &with_peers(peers, 4, 100));
    let [finding] = findings.as_slice() else {
        panic!("one finding")
    };
    assert!(finding.facts.contains(&("largest", Fact::Int(3))));
    assert!(finding.facts.contains(&("public", Fact::Int(4))));
}

#[test]
fn a_private_peer_counts_in_the_total_and_not_in_the_public_ones() {
    let ips = ["10.0.0.1", "203.0.113.1", "198.51.100.1"];
    let peers = ips.iter().map(|ip| peer(ip, Some(100))).collect();
    let findings = run("peers.diversity", &with_peers(peers, 3, 100));
    assert!(
        findings
            .iter()
            .all(|finding| finding.facts.contains(&("total", Fact::Int(3))))
    );
    assert!(
        findings
            .iter()
            .all(|finding| finding.facts.contains(&("public", Fact::Int(2))))
    );
}

#[test]
fn the_first_sync_is_not_a_failure_however_far_behind() {
    let findings = run("peers.height", &far(Vec::new()));
    assert!(
        findings
            .iter()
            .all(|finding| finding.status == Status::Warn)
    );
}
