//! The network checks: the P2P listener, the API's exposure, the forger's silence, `remoteAccess`,
//! the host firewall and the reachability the doctor cannot prove. They read the configuration and
//! the socket table, never the network.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use crate::doctor::DoctorProfile;
use crate::doctor::registry::CheckInput;
use crate::doctor::report::{Advice, Basis, Fact, Finding, Status, keys};
use crate::doctor::seams::{AccessEntry, RelayFacts, Subject, subject_of};
use crate::doctor::sources::probes::ServiceState;
use crate::doctor::sources::procfs::{SocketRow, TcpState};
use crate::doctor::view::{Reason, Unknown, View};
use crate::seams::Role;

/// The values of a finding: its facts, and the names its advice template fills in.
pub(super) type Values = Vec<(&'static str, Fact)>;

/// What a check's body gives: the status, the advice key and the values.
pub(super) type Verdict = (Status, &'static str, Values);

/// Runs a check's `body` about `role`'s subject (the host without a role), whose finding rests on
/// `basis`: `skipped` when `role` is not on this host, `unknown` (or `skipped` for a role that does not run) when `body` meets a
/// source that gave nothing.
pub(super) fn run(
    input: &CheckInput<'_>,
    (role, basis): (Option<Role>, Basis),
    body: impl FnOnce() -> Result<Verdict, Unknown>,
) -> Vec<Finding> {
    let subject = role.map_or(Subject::Host, subject_of);
    let skipped = || Finding::skipped(input.id(), subject, keys::SKIPPED_ABSENT);
    if role.is_some_and(|r| input.view.roles_present().is_ok_and(|p| !p.has(r))) {
        return vec![skipped()];
    }
    vec![match body() {
        Ok((status, key, values)) => {
            let mut finding = Finding::new(input.id(), subject, status, basis, Advice::key(key));
            (finding.facts, finding.advice.values) = (values.clone(), values);
            finding
        }
        Err(unknown) if matches!(unknown.reason, Reason::NotRunning | Reason::NotConfigured) => {
            skipped()
        }
        Err(unknown) => Finding::unknown(input.id(), subject, &unknown),
    }]
}

/// The status and advice key of a measure with a fail and a warn threshold.
pub(super) fn graded(fail: bool, warn: bool) -> (Status, &'static str) {
    match (fail, warn) {
        (true, _) => (Status::Fail, "low"),
        (_, true) => (Status::Warn, "low"),
        _ => (Status::Pass, "ok"),
    }
}

/// An integer fact, clamped.
pub(super) fn int<T: TryInto<i64>>(n: T) -> Fact {
    Fact::Int(n.try_into().unwrap_or(i64::MAX))
}

/// The listening sockets of `role`.
fn listeners(view: &dyn View, role: Role) -> Result<Vec<SocketRow>, Unknown> {
    let inodes = view.role_socket_inodes(role)?;
    let own = |row: &&SocketRow| row.state == TcpState::Listen && inodes.contains(&row.inode);
    Ok(view.sockets()?.iter().filter(own).copied().collect())
}

/// Whether `ip` is neither loopback, private, link-local nor shared address space.
fn is_public(ip: IpAddr) -> bool {
    match ip.to_canonical() {
        IpAddr::V4(a) => !matches!(
            a.octets(),
            [0 | 10 | 127, ..]
                | [100, 64..=127, ..]
                | [169, 254, ..]
                | [172, 16..=31, ..]
                | [192, 168, ..]
        ),
        IpAddr::V6(a) => !(a.is_loopback() || a.is_unique_local() || a.is_unspecified()),
    }
}

/// The plain addresses of the relay's `remoteAccess`.
fn listed(relay: &RelayFacts) -> Vec<IpAddr> {
    let plain = |entry: &AccessEntry| match entry {
        AccessEntry::Address(ip) => Some(ip.to_canonical()),
        _ => None,
    };
    relay.remote_access.iter().filter_map(plain).collect()
}

/// Whether (`Some`), and from where (`Some(Some(address))`), a forger uses `relay`: an established connection to the P2P port from a
/// listed address, or the forger of this host, whose configuration names the relay.
fn forger_of(view: &dyn View, relay: &RelayFacts) -> Result<Option<Option<IpAddr>>, Unknown> {
    let (port, listed) = (relay.p2p.port(), listed(relay));
    let from_listed = |row: &SocketRow| {
        let joined = row.state == TcpState::Established && row.local.port() == port;
        joined && listed.contains(&row.remote.ip().to_canonical())
    };
    if view
        .sockets()
        .is_ok_and(|rows| rows.iter().any(from_listed))
    {
        return Ok(Some(None));
    }
    if view
        .roles_present()
        .is_ok_and(|presence| !presence.has(Role::Forger))
    {
        return Ok(None);
    }
    let forger = view.forger_config()?;
    let Some(host) = forger.hosts.iter().find(|host| host.port() == port) else {
        return Ok(None);
    };
    let loopback = || match host.ip().to_canonical() {
        IpAddr::V4(_) => IpAddr::V4(Ipv4Addr::LOCALHOST),
        IpAddr::V6(_) => IpAddr::V6(Ipv6Addr::LOCALHOST),
    };
    let source = forger
        .local_address
        .or_else(|| host.ip().is_loopback().then(loopback));
    Ok(Some(source.map(|ip| ip.to_canonical())))
}

/// The relay's P2P listener: on its port, and on its address.
pub fn p2p_listen(input: &CheckInput<'_>) -> Vec<Finding> {
    run(input, (Some(Role::Relay), Basis::Socket), || {
        let config = input.view.relay_config()?;
        let rows = listeners(input.view, Role::Relay)?;
        let (address, port) = (config.p2p.ip().to_canonical(), config.p2p.port());
        let on_port: Vec<&SocketRow> = rows.iter().filter(|r| r.local.port() == port).collect();
        let here = |r: &&SocketRow| {
            let bound = r.local.ip().to_canonical();
            bound == address || (bound.is_unspecified() && address.is_unspecified())
        };
        let (status, key) = match on_port.iter().any(here) {
            _ if on_port.is_empty() => (Status::Fail, "not-listening"),
            true => (Status::Pass, "listening"),
            false => (Status::Warn, "other-address"),
        };
        Ok((status, key, vec![("port", Fact::Port(port))]))
    })
}

/// The API's exposure: public on a relay a forger uses is a warn (a fact under the onboarding
/// profile, which asks the operator to open it).
pub fn api_exposure(input: &CheckInput<'_>) -> Vec<Finding> {
    run(input, (Some(Role::Relay), Basis::Socket), || {
        let config = input.view.relay_config()?;
        let Some(api) = config.api else {
            return Ok((Status::Pass, "api-off", Vec::new()));
        };
        let rows = listeners(input.view, Role::Relay)?;
        let bound = rows.iter().filter(|row| row.local.port() == api.port());
        let mut ips: Vec<IpAddr> = bound.map(|row| row.local.ip()).collect();
        ips.extend(ips.is_empty().then_some(api.ip()));
        let values = vec![("port", Fact::Port(api.port()))];
        let (status, key) = if ips.iter().all(|ip| ip.to_canonical().is_loopback()) {
            (Status::Pass, "loopback")
        } else if input.options.profile == DoctorProfile::Onboarding {
            (Status::Pass, "exposed-onboarding")
        } else if forger_of(input.view, config)?.is_none() {
            (Status::Pass, "no-forger")
        } else {
            (Status::Warn, "exposed")
        };
        Ok((status, key, values))
    })
}

/// The forger opens no port, in every mode.
pub fn forger_no_listener(input: &CheckInput<'_>) -> Vec<Finding> {
    run(input, (Some(Role::Forger), Basis::Socket), || {
        Ok(match listeners(input.view, Role::Forger)?.first() {
            None => (Status::Pass, "none", Vec::new()),
            Some(row) => (
                Status::Fail,
                "listens",
                vec![("port", Fact::Port(row.local.port()))],
            ),
        })
    })
}

/// `remoteAccess`: exact addresses only, and the forger's own listed.
pub fn remote_access(input: &CheckInput<'_>) -> Vec<Finding> {
    run(input, (Some(Role::Relay), Basis::Config), || {
        let config = input.view.relay_config()?;
        let forger = forger_of(input.view, config)?;
        let (entries, plain) = (config.remote_access.len(), listed(config));
        let unlisted = matches!(forger, Some(Some(source)) if !plain.contains(&source));
        let (status, key) = if forger.is_some() && entries == 0 {
            (Status::Fail, "empty")
        } else if unlisted {
            (Status::Fail, "no-match")
        } else if entries > plain.len() {
            (Status::Warn, "unmatched-entry")
        } else if plain.iter().any(|ip| is_public(*ip)) {
            (Status::Warn, "public-address")
        } else if entries == 0 {
            (Status::Pass, "none-needed")
        } else {
            (Status::Pass, "listed")
        };
        Ok((
            status,
            key,
            vec![("entries", int(entries.saturating_sub(plain.len())))],
        ))
    })
}

/// A host firewall service is active.
pub fn firewall(input: &CheckInput<'_>) -> Vec<Finding> {
    run(input, (None, Basis::System), || {
        let services = input.view.firewall()?;
        let word = |state| {
            let words = ["active", "activating", "inactive", "failed", "other"];
            words.get(state as usize).copied().unwrap_or("other")
        };
        let disabled = matches!(input.view.ufw_conf(), Ok(Some(conf)) if !conf.enabled);
        let on = |state| state == ServiceState::Active;
        let active =
            (on(services.ufw) && !disabled) || on(services.nftables) || on(services.firewalld);
        let values = vec![
            ("ufw", Fact::Word(word(services.ufw))),
            ("nftables", Fact::Word(word(services.nftables))),
            ("firewalld", Fact::Word(word(services.firewalld))),
        ];
        Ok(match active {
            true => (Status::Pass, "active", values),
            false => (Status::Warn, "none-active", values),
        })
    })
}

/// Reachability from outside cannot be seen from inside: `unknown`, with where not to test from.
pub fn reachable(input: &CheckInput<'_>) -> Vec<Finding> {
    run(input, (Some(Role::Relay), Basis::System), || {
        let port = input.view.relay_config()?.p2p.port();
        Ok((
            Status::Unknown,
            "unverifiable",
            vec![("port", Fact::Port(port))],
        ))
    })
}
