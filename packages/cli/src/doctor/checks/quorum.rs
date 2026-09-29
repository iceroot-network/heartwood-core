//! Attestation and quorum: the forger's refusals at our slots, the relay's attestation of its
//! forger, and (later) the relay's reach.
//!
//! In this release the doctor sees the quorum as the relay reports it to the forger: the forger
//! logs a refusal at each of our slots where the relay's network state forbids forging. A relay
//! attests only a forger that reaches it over loopback, and the forger does not poll a relay it
//! does not reach over loopback at all, so that case shows only in the socket table.

use std::net::IpAddr;

use heartwood_kernel::log::Level;

use super::forger::{linked, loopback};
use super::slots::{self, about, counted, int};
use crate::doctor::registry::CheckInput;
use crate::doctor::report::Basis::{Log, Socket};
use crate::doctor::report::Status::{Fail, Pass, Warn};
use crate::doctor::report::{Fact, Finding, keys};
use crate::doctor::seams::Subject;
use crate::doctor::sources::logs::{LineKind as K, LogEvent};
use crate::doctor::sources::procfs::TcpState;
use crate::doctor::view::View;
use crate::seams::Role;

/// The slot length assumed when the chain definition cannot be read.
const DEFAULT_SLOT_MS: i64 = 8_000;
/// The active delegates assumed when the chain definition cannot be read.
const DEFAULT_ACTIVE: u32 = 53;

/// `quorum.forging-allowed`.
pub fn forging_allowed(input: &CheckInput<'_>) -> Vec<Finding> {
    about(input, |out| {
        let view = input.view;
        let log = view.log(Role::Forger)?;
        log.shows(Level::Info)?;
        let chain = slots::chain(view).ok();
        let slot_ms = chain.map_or(DEFAULT_SLOT_MS, slots::slot_ms);
        let slot = |at: i64| chain.map_or(at / slot_ms, |chain| slots::slot_at(chain, at));
        // A cold start in the relay's first round after its start is by design.
        let round_ms = slot_ms * i64::from(chain.map_or(DEFAULT_ACTIVE, |c| c.active_delegates));
        let relay = view.process(Role::Relay).ok();
        let relay_start = relay.and_then(|facts| facts.supervisor.as_ref()?.started_at_ms);
        let settled = |at: i64| relay_start.is_none_or(|start| at > start.saturating_add(round_ms));
        // Our slots, each with its latest refusal: the forger acts only at its own slots.
        let mut ours: Vec<(i64, Option<&LogEvent>)> = Vec::new();
        for event in &log.events {
            let refused = match event.kind {
                K::NotEnoughQuorum | K::NotPeeredEnough | K::NoNetworkState => true,
                K::ColdStartSkip => settled(event.at_ms),
                K::Forged { .. } | K::SendsAgain { .. } | K::TooLittleTime { .. } => false,
                K::AlreadyNextSlot { .. } | K::WillNotSign { .. } => false,
                K::ForgedOnAnotherNode { .. } | K::AlreadyForgedThisRound { .. } => false,
                _ => continue,
            };
            let (at, refusal) = (slot(event.at_ms), refused.then_some(event));
            match ours.last_mut() {
                Some((last, latest)) if *last == at => *latest = refusal.or(*latest),
                _ => ours.push((at, refusal)),
            }
        }
        let refusals: Vec<&LogEvent> = ours.iter().filter_map(|(_, refusal)| *refusal).collect();
        let Some(last) = refusals.last() else {
            let warned = log.events.iter().rev().find_map(|event| match event.kind {
                K::OverheightHeaders { .. } => Some(("overheight", event.at_ms)),
                K::HeightsOutOfSync { .. } => Some(("out-of-sync", event.at_ms)),
                _ => None,
            });
            return match warned {
                Some((key, at)) => out.one(Warn, Log, key, vec![("time", Fact::Time(at))]),
                None => {
                    super::running(view, Role::Forger)?;
                    out.one(Pass, Log, "none", vec![("slots", int(ours.len()))])
                }
            };
        };
        let last_two = ours.len() >= 2 && ours.iter().rev().take(2).all(|(_, r)| r.is_some());
        let key = match last.kind {
            K::NotEnoughQuorum => "quorum",
            K::NotPeeredEnough => peered(view),
            K::ColdStartSkip => "cold-start",
            _ => "no-network-state",
        };
        let status = if last_two { Fail } else { Warn };
        out.one(status, Log, key, counted(refusals.len(), last.at_ms))
    })
}

/// The advice for "not peered with enough delegates", by `peers.count`'s rule: too few peers is
/// this relay's problem; with enough peers, validators are offline across the network.
fn peered(view: &dyn View) -> &'static str {
    let peers = view.api_peers().map(|peers| peers.total_count);
    let minimum = view.relay_config().map(|relay| relay.minimum_network_reach);
    match (peers, minimum) {
        (Ok(peers), Ok(minimum)) if peers < minimum => "not-peered-few-peers",
        (Ok(_), Ok(_)) => "not-peered-network",
        _ => "not-peered",
    }
}

/// `quorum.attestation`.
pub fn attestation(input: &CheckInput<'_>) -> Vec<Finding> {
    about(input, |out| {
        let view = input.view;
        let mut findings = Vec::new();
        let hosts = &view.forger_config()?.hosts;
        match linked(view, hosts)?.first() {
            None => findings.push(out.to(Fail, Socket, "no-link", Vec::new())),
            Some((_, host)) if !loopback(host.ip()) => {
                let values = vec![("address", Fact::Address(host.ip()))];
                findings.push(out.to(Warn, Socket, "remote-relay", values));
            }
            Some(_) => {}
        }
        let events = |role| view.log(role).map_or(&[][..], |log| &log.events);
        let refusing = |e: &&LogEvent| matches!(e.kind, K::NotAttesting { .. });
        let refused: Vec<&LogEvent> = events(Role::Forger).iter().filter(refusing).collect();
        if let Some(last) = refused.last() {
            let values = counted(refused.len(), last.at_ms);
            findings.push(out.to(Warn, Log, "not-attesting", values));
        }
        let started = events(Role::Forger)
            .iter()
            .filter(|e| e.kind == K::ForgerStarted);
        let starts = started.count();
        let replaced: Vec<(IpAddr, IpAddr)> = (events(Role::Relay).iter())
            .filter_map(|event| match event.kind {
                K::AttestationSessionReplaced { new, old } => Some((new, old)),
                _ => None,
            })
            .collect();
        // Sessions that take turns: A replaces B, then B replaces A.
        let turns = replaced.windows(2).find_map(|pair| match pair {
            [(a, b), (c, d)] if a == d && b == c => Some((*a, *b)),
            _ => None,
        });
        let relay = out.about(Subject::Relay);
        if let Some((a, b)) = turns {
            let values = vec![("first", Fact::Address(a)), ("second", Fact::Address(b))];
            findings.push(match both_connected(view, a, b) {
                true => relay.to(Fail, Socket, "alternating", values),
                false => relay.to(Warn, Log, "alternating-logged", values),
            });
        } else if replaced.len() > starts {
            let values = vec![("count", int(replaced.len())), ("starts", int(starts))];
            findings.push(relay.to(Warn, Log, "replaced", values));
        }
        if findings.is_empty() {
            findings.push(out.to(Pass, Socket, "attested", Vec::new()));
        }
        Ok(findings)
    })
}

/// Whether the socket table shows established connections from both loopback addresses `a` and
/// `b` to this relay's P2P port: two forgers on this host taking turns.
fn both_connected(view: &dyn View, a: IpAddr, b: IpAddr) -> bool {
    let (Ok(relay), Ok(rows)) = (view.relay_config(), view.sockets()) else {
        return false;
    };
    let from = |ip: IpAddr| {
        loopback(ip)
            && rows.iter().any(|row| {
                row.state == TcpState::Established
                    && row.local.port() == relay.p2p.port()
                    && row.remote.ip().to_canonical() == ip.to_canonical()
            })
    };
    a != b && from(a) && from(b)
}

/// `quorum.reach`: the relay's reach and quorum come with the health file of a later release.
pub fn reach(input: &CheckInput<'_>) -> Vec<Finding> {
    vec![Finding::skipped(
        input.id(),
        Subject::Relay,
        keys::SKIPPED_RELEASE,
    )]
}
