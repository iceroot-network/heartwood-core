//! The cases of the net checks: each names the check, the view, the options and the findings it
//! must give. The registry walk requires a case for each status a built check declares. The
//! builders at the top are shared by the clock and disk cases.

use std::collections::BTreeSet;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;

use heartwood_cli::Role;
use heartwood_cli::doctor::DoctorOptions;
use heartwood_cli::doctor::DoctorProfile;
use heartwood_cli::doctor::docs::docs;
use heartwood_cli::doctor::registry::{CheckInput, ENTRIES};
use heartwood_cli::doctor::report::Status;
use heartwood_cli::doctor::seams::{
    AccessEntry, ChainFacts, FileFacts, FileKind, ForgerFacts, Meta, RelayFacts, Subject,
};
use heartwood_cli::doctor::sources::probes::{FirewallServices, ServiceState, UfwConf};
use heartwood_cli::doctor::sources::procfs::{SocketRow, TcpState};
use heartwood_cli::doctor::view::{Reason, RolePresence, Source, Unknown};
use heartwood_kernel::log::Level;

use super::{Case, Expect};
use crate::doctor_support::FakeView;

/// The network epoch of the cases, in Unix milliseconds.
pub const EPOCH_MS: i64 = 1_700_000_000_000;

/// A required finding.
pub fn ex(subject: Subject, status: Status, advice: &'static str) -> Expect {
    Expect {
        subject,
        status,
        advice,
    }
}

/// A socket address.
pub fn addr(text: &str) -> SocketAddr {
    text.parse().unwrap()
}

/// An IP address.
pub fn ip(text: &str) -> IpAddr {
    text.parse().unwrap()
}

/// A chain definition with the block time.
pub fn chain(block_time_s: u32) -> ChainFacts {
    ChainFacts {
        nethash: "0".repeat(64),
        epoch_ms: EPOCH_MS,
        block_time_s,
        active_delegates: 53,
    }
}

/// A relay's configuration.
pub fn relay(p2p: &str, api: Option<&str>, access: Vec<AccessEntry>) -> RelayFacts {
    RelayFacts {
        p2p: addr(p2p),
        api: api.map(addr),
        remote_access: access,
        minimum_network_reach: 20,
        file_log_level: Level::Info,
        warnings: Vec::new(),
        env_names: Vec::new(),
        chain: chain(8),
        store_file: PathBuf::from("/nodes/node1/data/database/blockchain.db"),
    }
}

/// A forger's configuration.
pub fn forger(hosts: &[&str], local: Option<&str>) -> ForgerFacts {
    ForgerFacts {
        hosts: hosts.iter().map(|host| addr(host)).collect(),
        local_address: local.map(ip),
        aux_set: false,
        file_log_level: Level::Info,
        key_file: PathBuf::from("/nodes/node1/config/delegates.json"),
        signing_record: PathBuf::from("/nodes/node1/data/forger/signing-record.json"),
        chain: chain(8),
    }
}

/// The roles present.
pub fn presence(relay: bool, forger: bool) -> RolePresence {
    RolePresence { relay, forger }
}

/// A view of a host that runs both roles, every other fact unset.
pub fn base() -> FakeView {
    let mut view = FakeView::new();
    view.roles_present = Ok(presence(true, true));
    view
}

/// A source that is not readable by this user.
pub fn denied(source: Source) -> Unknown {
    Unknown::new(source, Reason::Denied)
}

/// The options of a profile.
pub fn options(profile: DoctorProfile) -> DoctorOptions {
    DoctorOptions {
        profile,
        ..DoctorOptions::default()
    }
}

/// A case with the default options.
pub fn case(name: &'static str, check: &'static str, view: FakeView, expect: Vec<Expect>) -> Case {
    Case {
        name,
        check,
        view,
        options: DoctorOptions::default(),
        expect,
    }
}

/// The metadata of a regular file of `size` bytes.
pub fn file_of(size: u64) -> FileFacts {
    FileFacts {
        path: PathBuf::from("/nodes/node1/data/database/blockchain.db"),
        own: Meta {
            kind: FileKind::File,
            mode: 0o644,
            uid: 1000,
            gid: 1000,
            size,
            dev: 1,
            ino: 2,
            nlink: 1,
            modified_ms: 0,
        },
        target: None,
        link_target: None,
    }
}

/// The placeholders of an advice template.
fn placeholders(template: &str) -> BTreeSet<&str> {
    let mut names = BTreeSet::new();
    let mut rest = template;
    while let Some((_, after)) = rest.split_once('{') {
        let Some((name, tail)) = after.split_once('}') else {
            break;
        };
        names.insert(name);
        rest = tail;
    }
    names
}

/// Every finding the cases give has an advice template whose placeholders its values fill, and
/// every template of the area's checks is given by some case.
pub fn resolve(area: &str, cases: Vec<Case>) {
    let docs = docs().unwrap();
    let mut given: BTreeSet<(&str, &str)> = BTreeSet::new();
    for case in &cases {
        let entry = ENTRIES.iter().find(|entry| entry.id == case.check).unwrap();
        let input = CheckInput {
            entry,
            view: &case.view,
            options: &case.options,
        };
        for finding in (entry.run)(&input) {
            let key = finding.advice.key;
            let Some(template) = docs.template(entry.check_id(), key) else {
                panic!("{}: no template {key} for {}", case.name, entry.id);
            };
            for name in placeholders(template) {
                let has = finding
                    .advice
                    .values
                    .iter()
                    .any(|(value, _)| *value == name);
                assert!(
                    has,
                    "{}: {key} names {{{name}}}, which no value fills",
                    case.name
                );
            }
            given.insert((entry.id, key));
        }
    }
    for doc in docs.checks.iter().filter(|doc| doc.area == area) {
        for key in doc.advice.keys() {
            let used = given.contains(&(doc.id.as_str(), key.as_str()));
            assert!(used, "{}: no case gives the advice {key}", doc.id);
        }
    }
}

/// A socket row.
fn row(local: &str, remote: &str, state: TcpState, inode: u64) -> SocketRow {
    SocketRow {
        local: addr(local),
        remote: addr(remote),
        state,
        uid: 1000,
        inode,
    }
}

/// A listener of `inode`.
fn listener(local: &str, inode: u64) -> SocketRow {
    row(local, "0.0.0.0:0", TcpState::Listen, inode)
}

/// A relay view: it listens as `rows` say, and its own sockets are inodes 10 and 11.
fn relay_view(config: RelayFacts, rows: Vec<SocketRow>) -> FakeView {
    let mut view = base();
    view.relay_config = Ok(config);
    view.sockets = Ok(rows);
    view.role_inodes.insert(Role::Relay, Ok(vec![10, 11]));
    view.role_inodes.insert(Role::Forger, Ok(vec![20]));
    view
}

const RELAY: Subject = Subject::Relay;
const P2P: &str = "0.0.0.0:4001";

/// The relay's P2P listener.
fn p2p_listen() -> Vec<Case> {
    let id = "net.p2p-listen";
    let config = || relay(P2P, Some("127.0.0.1:4003"), Vec::new());
    let mut cases = Vec::new();
    let mut add = |name, rows: Vec<SocketRow>, status, key| {
        let view = relay_view(config(), rows);
        cases.push(case(name, id, view, vec![ex(RELAY, status, key)]));
    };
    let pass = Status::Pass;
    add("p2p listens", vec![listener(P2P, 10)], pass, "listening");
    add(
        "p2p listens on a mapped address",
        vec![listener("[::ffff:0.0.0.0]:4001", 10)],
        pass,
        "listening",
    );
    add(
        "p2p listens on every address of both families",
        vec![listener("[::]:4001", 11)],
        pass,
        "listening",
    );
    add(
        "p2p listens on another address",
        vec![listener("127.0.0.1:4001", 10)],
        Status::Warn,
        "other-address",
    );
    add(
        "p2p listens nowhere",
        Vec::new(),
        Status::Fail,
        "not-listening",
    );
    add(
        "p2p port held by another process",
        vec![listener(P2P, 99)],
        Status::Fail,
        "not-listening",
    );
    add(
        "relay listens on another port only",
        vec![listener("0.0.0.0:4002", 10)],
        Status::Fail,
        "not-listening",
    );
    add(
        "an established socket is no listener",
        vec![row(P2P, "10.0.0.7:5000", TcpState::Established, 10)],
        Status::Fail,
        "not-listening",
    );
    let mut view = relay_view(config(), Vec::new());
    view.sockets = Err(denied(Source::Sockets));
    cases.push(case(
        "p2p sockets unreadable",
        id,
        view,
        vec![ex(RELAY, Status::Unknown, "unknown")],
    ));
    let mut view = base();
    view.relay_config = Err(denied(Source::Config));
    cases.push(case(
        "p2p config unreadable",
        id,
        view,
        vec![ex(RELAY, Status::Unknown, "unknown")],
    ));
    let mut view = relay_view(config(), Vec::new());
    view.roles_present = Ok(presence(false, true));
    cases.push(case(
        "p2p without a relay",
        id,
        view,
        vec![ex(RELAY, Status::Skipped, "skipped-absent")],
    ));
    cases
}

/// A relay whose API listens on `bound` and whose forger uses port 4001.
fn api_view(api: Option<&str>, bound: &[&str], forger_here: bool) -> FakeView {
    let rows = bound.iter().map(|local| listener(local, 10)).collect();
    let mut view = relay_view(relay(P2P, api, Vec::new()), rows);
    view.roles_present = Ok(presence(true, forger_here));
    view.forger_config = Ok(forger(&["127.0.0.1:4001"], None));
    view
}

/// The API's exposure.
fn api_exposure() -> Vec<Case> {
    let id = "net.api-exposure";
    let public = "0.0.0.0:4003";
    let mut cases = Vec::new();
    let mut add = |name, view, profile, status, key| {
        let expect = vec![ex(RELAY, status, key)];
        cases.push(Case {
            options: options(profile),
            ..case(name, id, view, expect)
        });
    };
    let default = DoctorProfile::Default;
    add(
        "api switched off",
        api_view(None, &[], true),
        default,
        Status::Pass,
        "api-off",
    );
    let view = api_view(Some("127.0.0.1:4003"), &["127.0.0.1:4003"], true);
    add("api on loopback", view, default, Status::Pass, "loopback");
    let view = api_view(Some("[::1]:4003"), &["[::1]:4003"], true);
    add(
        "api on the v6 loopback",
        view,
        default,
        Status::Pass,
        "loopback",
    );
    let view = api_view(Some(public), &[public], true);
    add(
        "public api under a forger",
        view,
        default,
        Status::Warn,
        "exposed",
    );
    let view = api_view(Some("127.0.0.1:4003"), &[public], true);
    add(
        "bound wider than configured",
        view,
        default,
        Status::Warn,
        "exposed",
    );
    let view = api_view(Some(public), &[], true);
    add(
        "public api not yet bound",
        view,
        default,
        Status::Warn,
        "exposed",
    );
    let view = api_view(Some(public), &[public], true);
    add(
        "public api under the onboarding profile",
        view,
        DoctorProfile::Onboarding,
        Status::Pass,
        "exposed-onboarding",
    );
    let view = api_view(Some(public), &[public], false);
    add(
        "public api with no forger",
        view,
        default,
        Status::Pass,
        "no-forger",
    );
    let mut view = api_view(Some(public), &[public], true);
    view.forger_config = Ok(forger(&["10.0.0.9:4101"], None));
    add(
        "public api, forger on another relay",
        view,
        default,
        Status::Pass,
        "no-forger",
    );
    let mut view = api_view(Some(public), &[public], false);
    let access = vec![AccessEntry::Address(ip("10.0.0.5"))];
    view.relay_config = Ok(relay(P2P, Some(public), access));
    view.sockets = Ok(vec![
        listener(public, 10),
        row("10.0.0.1:4001", "10.0.0.5:52000", TcpState::Established, 10),
    ]);
    add(
        "public api, forger connected from a listed address",
        view,
        default,
        Status::Warn,
        "exposed",
    );
    let mut view = api_view(Some(public), &[public], true);
    view.forger_config = Err(denied(Source::Config));
    add(
        "public api, forger configuration unreadable",
        view,
        default,
        Status::Unknown,
        "unknown",
    );
    let mut view = api_view(Some(public), &[public], true);
    view.relay_config = Err(denied(Source::Config));
    add(
        "api config unreadable",
        view,
        default,
        Status::Unknown,
        "unknown",
    );
    let mut view = api_view(Some(public), &[public], true);
    view.roles_present = Ok(presence(false, true));
    add(
        "api without a relay",
        view,
        default,
        Status::Skipped,
        "skipped-absent",
    );
    cases
}

/// The forger listens on nothing.
fn forger_no_listener() -> Vec<Case> {
    let id = "net.forger-no-listener";
    let forger = Subject::Forger;
    let mut cases = Vec::new();
    let mut add = |name, rows: Vec<SocketRow>, status, key| {
        let view = relay_view(relay(P2P, None, Vec::new()), rows);
        cases.push(case(name, id, view, vec![ex(forger, status, key)]));
    };
    add(
        "forger opens no port",
        vec![listener("0.0.0.0:4001", 10), listener("0.0.0.0:9000", 30)],
        Status::Pass,
        "none",
    );
    add(
        "forger with only an outgoing connection",
        vec![row(
            "127.0.0.1:50000",
            "127.0.0.1:4001",
            TcpState::Established,
            20,
        )],
        Status::Pass,
        "none",
    );
    add(
        "forger listens",
        vec![listener("0.0.0.0:9000", 20)],
        Status::Fail,
        "listens",
    );
    add(
        "forger listens on loopback too",
        vec![listener("127.0.0.1:9000", 20)],
        Status::Fail,
        "listens",
    );
    let mut view = relay_view(relay(P2P, None, Vec::new()), Vec::new());
    view.role_inodes.remove(&Role::Forger);
    cases.push(case(
        "forger sockets unreadable",
        id,
        view,
        vec![ex(forger, Status::Unknown, "unknown")],
    ));
    let mut view = relay_view(relay(P2P, None, Vec::new()), Vec::new());
    view.roles_present = Ok(presence(true, false));
    cases.push(case(
        "no forger here",
        id,
        view,
        vec![ex(forger, Status::Skipped, "skipped-absent")],
    ));
    cases
}

/// A relay with `access`, a forger of this host that reaches it from `local`.
fn access_view(access: Vec<AccessEntry>, hosts: &[&str], local: Option<&str>) -> FakeView {
    let mut view = relay_view(relay(P2P, None, access), Vec::new());
    view.forger_config = Ok(forger(hosts, local));
    view
}

/// `remoteAccess`.
fn remote_access() -> Vec<Case> {
    let id = "net.remote-access";
    let here = ["127.0.0.1:4001"];
    let far = ["10.0.0.1:4001"];
    let address = |text| AccessEntry::Address(ip(text));
    let prefix = || AccessEntry::Prefix {
        address: ip("10.0.0.0"),
        bits: 24,
    };
    let mut cases = Vec::new();
    let mut add = |name, view, status, key| {
        cases.push(case(name, id, view, vec![ex(RELAY, status, key)]));
    };
    add(
        "forger listed by its address",
        access_view(vec![address("127.0.0.1")], &here, None),
        Status::Pass,
        "listed",
    );
    add(
        "forger listed by localAddress",
        access_view(vec![address("10.0.0.5")], &far, Some("10.0.0.5")),
        Status::Pass,
        "listed",
    );
    add(
        "forger's address not known",
        access_view(vec![address("10.0.0.9")], &far, None),
        Status::Pass,
        "listed",
    );
    add(
        "a prefix matches nothing",
        access_view(vec![prefix()], &far, Some("10.0.0.5")),
        Status::Fail,
        "no-match",
    );
    add(
        "a host name matches nothing",
        access_view(vec![AccessEntry::Other], &far, Some("10.0.0.5")),
        Status::Fail,
        "no-match",
    );
    add(
        "another address does not match",
        access_view(vec![address("10.0.0.6")], &far, Some("10.0.0.5")),
        Status::Fail,
        "no-match",
    );
    add(
        "empty list and a local forger",
        access_view(Vec::new(), &here, None),
        Status::Fail,
        "empty",
    );
    add(
        "empty list, forger on a far host",
        access_view(Vec::new(), &far, Some("10.0.0.5")),
        Status::Fail,
        "empty",
    );
    add(
        "empty list, forger on another relay",
        access_view(Vec::new(), &["10.0.0.9:4101"], None),
        Status::Pass,
        "none-needed",
    );
    let entries = vec![address("127.0.0.1"), prefix()];
    add(
        "a prefix beside the forger's address",
        access_view(entries, &here, None),
        Status::Warn,
        "unmatched-entry",
    );
    let entries = vec![address("127.0.0.1"), AccessEntry::Other, prefix()];
    add(
        "two entries that match nothing",
        access_view(entries, &here, None),
        Status::Warn,
        "unmatched-entry",
    );
    let entries = vec![address("127.0.0.1"), address("203.0.113.9")];
    add(
        "a public address listed",
        access_view(entries, &here, None),
        Status::Warn,
        "public-address",
    );
    let mut view = access_view(vec![address("203.0.113.9")], &here, None);
    view.roles_present = Ok(presence(true, false));
    add(
        "a public address listed, no forger",
        view,
        Status::Warn,
        "public-address",
    );
    let mut view = access_view(Vec::new(), &here, None);
    view.roles_present = Ok(presence(true, false));
    add("empty list, no forger", view, Status::Pass, "none-needed");
    let mut view = access_view(vec![address("10.0.0.5")], &here, None);
    view.roles_present = Ok(presence(true, false));
    view.sockets = Ok(vec![row(
        "10.0.0.1:4001",
        "10.0.0.5:52000",
        TcpState::Established,
        10,
    )]);
    add(
        "a listed address is connected",
        view,
        Status::Pass,
        "listed",
    );
    let mut view = access_view(Vec::new(), &here, None);
    view.forger_config = Err(denied(Source::Config));
    add(
        "forger configuration unreadable",
        view,
        Status::Unknown,
        "unknown",
    );
    let mut view = base();
    view.relay_config = Err(denied(Source::Config));
    add(
        "remote access config unreadable",
        view,
        Status::Unknown,
        "unknown",
    );
    let mut view = access_view(Vec::new(), &here, None);
    view.roles_present = Ok(presence(false, true));
    add(
        "remote access without a relay",
        view,
        Status::Skipped,
        "skipped-absent",
    );
    cases
}

/// A firewall view.
fn firewall_view(ufw: ServiceState, nft: ServiceState, fwd: ServiceState) -> FakeView {
    let mut view = base();
    view.firewall = Ok(FirewallServices {
        ufw,
        nftables: nft,
        firewalld: fwd,
    });
    view
}

/// The host firewall.
fn firewall() -> Vec<Case> {
    use ServiceState::{Activating, Active, Failed, Inactive, Other};
    let id = "net.firewall";
    let host = Subject::Host;
    let mut cases = Vec::new();
    let mut add = |name, view, status, key| {
        cases.push(case(name, id, view, vec![ex(host, status, key)]));
    };
    add(
        "ufw active",
        firewall_view(Active, Inactive, Inactive),
        Status::Pass,
        "active",
    );
    add(
        "nftables active",
        firewall_view(Inactive, Active, Inactive),
        Status::Pass,
        "active",
    );
    add(
        "firewalld active",
        firewall_view(Inactive, Inactive, Active),
        Status::Pass,
        "active",
    );
    add(
        "nothing active",
        firewall_view(Inactive, Inactive, Inactive),
        Status::Warn,
        "none-active",
    );
    add(
        "services failed",
        firewall_view(Failed, Failed, Failed),
        Status::Warn,
        "none-active",
    );
    add(
        "services activating",
        firewall_view(Activating, Activating, Activating),
        Status::Warn,
        "none-active",
    );
    add(
        "services in another state",
        firewall_view(Other, Other, Other),
        Status::Warn,
        "none-active",
    );
    let mut view = firewall_view(Active, Inactive, Inactive);
    view.ufw_conf = Ok(Some(UfwConf { enabled: true }));
    add("ufw active and enabled", view, Status::Pass, "active");
    let mut view = firewall_view(Active, Inactive, Inactive);
    view.ufw_conf = Ok(Some(UfwConf { enabled: false }));
    add(
        "ufw active but disabled in its configuration",
        view,
        Status::Warn,
        "none-active",
    );
    let mut view = firewall_view(Active, Active, Inactive);
    view.ufw_conf = Ok(Some(UfwConf { enabled: false }));
    add(
        "ufw disabled, nftables active",
        view,
        Status::Pass,
        "active",
    );
    let mut view = firewall_view(Active, Inactive, Inactive);
    view.ufw_conf = Ok(None);
    add(
        "ufw active, no configuration file",
        view,
        Status::Pass,
        "active",
    );
    let mut view = base();
    view.firewall = Err(Unknown::new(Source::Probes, Reason::ProbeMissing));
    add(
        "firewall services unreadable",
        view,
        Status::Unknown,
        "unknown",
    );
    cases
}

/// The reachability nothing on the host can prove.
fn reachable() -> Vec<Case> {
    let id = "net.reachable";
    let mut cases = Vec::new();
    let view = relay_view(relay(P2P, None, Vec::new()), Vec::new());
    cases.push(case(
        "reachability is unknown",
        id,
        view,
        vec![ex(RELAY, Status::Unknown, "unverifiable")],
    ));
    let mut view = relay_view(relay(P2P, None, Vec::new()), vec![listener(P2P, 10)]);
    view.sockets = Ok(vec![listener(P2P, 10)]);
    let onboarding = Case {
        options: options(DoctorProfile::Onboarding),
        ..case(
            "reachability is unknown, onboarding",
            id,
            view,
            vec![ex(RELAY, Status::Unknown, "unverifiable")],
        )
    };
    cases.push(onboarding);
    let mut view = base();
    view.relay_config = Err(denied(Source::Config));
    cases.push(case(
        "reachability config unreadable",
        id,
        view,
        vec![ex(RELAY, Status::Unknown, "unknown")],
    ));
    let mut view = relay_view(relay(P2P, None, Vec::new()), Vec::new());
    view.roles_present = Ok(presence(false, true));
    cases.push(case(
        "reachability without a relay",
        id,
        view,
        vec![ex(RELAY, Status::Skipped, "skipped-absent")],
    ));
    cases
}

/// Every case of the area.
pub fn cases() -> Vec<Case> {
    [
        p2p_listen(),
        api_exposure(),
        forger_no_listener(),
        remote_access(),
        firewall(),
        reachable(),
    ]
    .into_iter()
    .flatten()
    .collect()
}

#[test]
fn every_net_advice_has_its_template_and_its_values() {
    resolve("net", cases());
}

#[test]
fn the_reachability_advice_names_where_not_to_test_from() {
    let docs = docs().unwrap();
    let id = heartwood_cli::doctor::registry::CheckId::parse("net.reachable").unwrap();
    let text = docs.template(id, "unverifiable").unwrap();
    assert!(text.contains("no connection to this relay"), "{text}");
    assert!(text.contains("forger's host"), "{text}");
    assert!(text.contains("other relays or sentries"), "{text}");
    assert!(text.contains("10 seconds"), "{text}");
}
