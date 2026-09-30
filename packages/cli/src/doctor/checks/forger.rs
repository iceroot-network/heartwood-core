//! The forger's link and failover: its relays, its connection, its link failures, one key on two
//! forgers, its signing record and its start errors.
//!
//! A log line is evidence that something was logged, not that it happened: a relay can put text
//! into the forger's log. So `forger.one-key-one-forger` fails only when the chain agrees (a block
//! signed with our key in that slot while the forger's signing record holds that slot with another
//! block); a line nothing confirms is a warn that says to confirm first.

use std::net::{IpAddr, SocketAddr};

use heartwood_kernel::log::Level;

use super::slots::{self, Ours, Values, about, counted, int};
use crate::doctor::registry::CheckInput;
use crate::doctor::report::Basis::{Chain, Config, Log, Socket, System};
use crate::doctor::report::Status::{Fail, Pass, Warn};
use crate::doctor::report::{Fact, Finding, Id, IdKind};
use crate::doctor::seams::{ChainFacts, RoleState, StoreWindow};
use crate::doctor::sources::fs::SigningRecordFacts as Record;
use crate::doctor::sources::logs::{LineKind as K, LogEvent, LogWindow, StartError as E};
use crate::doctor::sources::procfs::TcpState;
use crate::doctor::view::{NodeFile, Reason, Unknown, View};
use crate::seams::Role;

/// How recent a link failure must be to fail together with no connection now.
const RECENT_MS: i64 = 5 * 60 * 1_000;

/// The relays the forger is connected to, with their place in its list.
pub(super) type Links = Vec<(usize, SocketAddr)>;

/// Whether `ip` is a loopback address, an IPv4-mapped one included.
pub(super) fn loopback(ip: IpAddr) -> bool {
    ip.to_canonical().is_loopback()
}

/// An address as a fact.
fn address(host: &SocketAddr) -> (&'static str, Fact) {
    ("address", Fact::Address(host.ip()))
}

/// The relays of `hosts` the running forger holds an established connection to, with their
/// place in the list (0 is the first).
pub(super) fn linked(view: &dyn View, hosts: &[SocketAddr]) -> Result<Links, Unknown> {
    let (inodes, rows) = (view.role_socket_inodes(Role::Forger)?, view.sockets()?);
    let to = |host: &SocketAddr| {
        rows.iter().any(|row| {
            row.state == TcpState::Established
                && inodes.contains(&row.inode)
                && row.remote.ip().to_canonical() == host.ip().to_canonical()
                && row.remote.port() == host.port()
        })
    };
    let listed = hosts.iter().copied().enumerate();
    Ok(listed.filter(|(_, host)| to(host)).collect())
}

/// `forger.hosts`.
pub fn hosts(input: &CheckInput<'_>) -> Vec<Finding> {
    about(input, |out| {
        let hosts = &input.view.forger_config()?.hosts;
        let count = vec![("count", int(hosts.len()))];
        let mut findings = Vec::new();
        if let Some(first) = hosts.first().filter(|host| !loopback(host.ip())) {
            findings.push(out.to(Warn, Config, "first-not-loopback", vec![address(first)]));
        }
        match hosts.len() {
            0 | 1 => findings.push(out.to(Warn, Config, "one-relay", count)),
            4.. => findings.push(out.to(Warn, Config, "too-many", count)),
            _ if findings.is_empty() => {
                // A later relay off loopback is a fact on the pass line, not a warn.
                let later = hosts.iter().position(|host| !loopback(host.ip()));
                findings.push(match later {
                    Some(at) => out.to(Pass, Config, "pass-remote", vec![("relay", int(at + 1))]),
                    None => out.to(Pass, Config, "pass", count),
                });
            }
            _ => {}
        }
        Ok(findings)
    })
}

/// `forger.link`.
pub fn link(input: &CheckInput<'_>) -> Vec<Finding> {
    about(input, |out| {
        let hosts = &input.view.forger_config()?.hosts;
        match linked(input.view, hosts)?.first() {
            Some((0, host)) => out.one(Pass, Socket, "first", vec![address(host)]),
            Some((at, host)) => {
                let values = vec![("relay", int(at + 1)), address(host)];
                out.one(Warn, Socket, "failover", values)
            }
            None => out.one(Fail, Socket, "none", vec![("count", int(hosts.len()))]),
        }
    })
}

/// `forger.link-history`.
pub fn link_history(input: &CheckInput<'_>) -> Vec<Finding> {
    about(input, |out| {
        let view = input.view;
        let log = view.log(Role::Forger)?;
        // "Waiting for relay to become ready" is an info line.
        log.shows(Level::Info)?;
        // Waiting for the relay counts only once the relay says it is not syncing.
        let ready = view.api_syncing().is_ok_and(|syncing| !syncing.syncing);
        let failures: Vec<&LogEvent> = (log.events.iter())
            .filter(|event| match event.kind {
                K::WaitingForHost | K::NoOpenSocket | K::RelayRequestFailed => true,
                K::HostsDidNotRespond | K::SessionError | K::SyncCheckFailed => true,
                K::BroadcastFailed | K::EmitFailed => true,
                K::WaitingForRelayReady => ready,
                _ => false,
            })
            .collect();
        let Some(last) = failures.last() else {
            super::running(view, Role::Forger)?;
            return out.one(Pass, Log, "none", Vec::new());
        };
        // The forger broadcasts only the block it forged at its own slot.
        let broadcast = failures.iter().rev().find(|e| e.kind == K::BroadcastFailed);
        if let Some(event) = broadcast {
            let at = vec![("time", Fact::Time(event.at_ms))];
            return out.one(Fail, Log, "broadcast", at);
        }
        let values = counted(failures.len(), last.at_ms);
        let hosts = view.forger_config().map(|forger| forger.hosts.as_slice());
        let unlinked = hosts.and_then(|hosts| linked(view, hosts));
        let recent = last.at_ms >= view.now_ms().saturating_sub(RECENT_MS);
        match recent && unlinked.is_ok_and(|links| links.is_empty()) {
            true => out.one(Fail, Socket, "down", values),
            false => out.one(Warn, Log, "failures", values),
        }
    })
}

/// What confirms or clears a claim: our delegates, the chain's slots, the store window and the
/// signing record; `None` when one of them cannot be read, so nothing is confirmed.
type Evidence<'a> = Option<(Ours, &'a ChainFacts, StoreWindow, Option<&'a Record>)>;

/// The evidence of `input`'s view.
fn evidence<'a>(input: &CheckInput<'a>) -> Evidence<'a> {
    let view = input.view;
    let ours = slots::ours(view, input.options);
    let chain = slots::chain(view).ok()?;
    let store = slots::store(view, &ours, chain).ok()?;
    Some((ours, chain, store, view.signing_record().ok()?))
}

/// A claim's verdict: `Ok(Some)` confirmed, with the block's values; `Ok(None)` cleared;
/// `Err` not confirmed.
type Confirmed = Result<Option<Values>, ()>;

/// The block that confirms the claim `event` about `delegate`: the chain's block in that slot,
/// signed with our key, while the signing record holds that slot with another block. `Ok(None)`
/// when the chain's block is this forger's own (it restarted inside a slot it had signed), by the
/// record or by the forger's own "forged" line for its height; `Err` when nothing confirms the
/// line. A missing "forged" line confirms nothing: the record may have moved on since, and the
/// file log level may hide info lines.
fn confirm(evidence: &Evidence<'_>, log: &LogWindow, at_ms: i64, delegate: &Id) -> Confirmed {
    let (ours, chain, store, record) = evidence.as_ref().ok_or(())?;
    let slot = slots::slot_at(chain, at_ms);
    let block = store.blocks.iter().find(|block| {
        slots::slot_of(chain, block.timestamp) == slot
            && (block.username.as_deref() == Some(delegate.as_str()) || ours.holds(block))
    });
    let block = block.ok_or(())?;
    let entries = record.map(|r| r.entries.as_slice()).unwrap_or_default();
    let key = &block.generator_public_key;
    let forged = |e: &LogEvent| slots::forged(&e.kind).is_some_and(|f| f.1 == block.height);
    let logged = log.shows(Level::Info).is_ok() && log.events.iter().any(forged);
    let signed_here = match entries.iter().find(|entry| entry.public_key == *key) {
        Some(entry) if entry.slot == slot => entry.block_id == block.id,
        _ if logged => true,
        _ => return Err(()),
    };
    let delegate = ("delegate", Fact::Id(delegate.clone()));
    let mut values = vec![("height", int(block.height)), delegate];
    values.extend(Id::new(IdKind::Hex64, &block.id).map(|id| ("block", Fact::Id(id))));
    Ok((!signed_here).then_some(values))
}

/// `forger.one-key-one-forger`.
pub fn one_key_one_forger(input: &CheckInput<'_>) -> Vec<Finding> {
    about(input, |out| {
        let log = input.view.log(Role::Forger)?;
        log.shows(Level::Warning)?;
        let claims: Vec<(&LogEvent, &Id)> = (log.events.iter())
            .filter_map(|event| match &event.kind {
                K::ForgedOnAnotherNode { username } | K::AlreadyForgedThisRound { username } => {
                    Some((event, username))
                }
                _ => None,
            })
            .collect();
        let evidence = (!claims.is_empty()).then(|| evidence(input)).flatten();
        let (mut findings, mut logged, mut own) = (Vec::<Finding>::new(), Vec::new(), 0);
        for (event, delegate) in claims {
            match confirm(&evidence, log, event.at_ms, delegate) {
                Ok(Some(values)) if !findings.iter().any(|f| f.facts.first() == values.first()) => {
                    findings.push(out.to(Fail, Chain, "another-forger", values));
                }
                Ok(Some(_)) => {}
                Ok(None) => own += 1,
                Err(()) => logged.push((event, delegate)),
            }
        }
        if let Some((event, delegate)) = logged.last() {
            let mut values = counted(logged.len(), event.at_ms);
            values.push(("delegate", Fact::Id((*delegate).clone())));
            findings.push(out.to(Warn, Log, "logged-only", values));
        }
        match (findings.is_empty(), own) {
            (false, _) => Ok(findings),
            (true, 0) => out.one(Pass, Log, "none", Vec::new()),
            (true, _) => out.one(Pass, Chain, "own-block", vec![("count", int(own))]),
        }
    })
}

/// `forger.signing-record`.
pub fn signing_record(input: &CheckInput<'_>) -> Vec<Finding> {
    about(input, |out| {
        let view = input.view;
        let absent = || out.one(Pass, System, "absent", Vec::new());
        let Some(file) = view.file(NodeFile::SigningRecord)? else {
            return absent();
        };
        let path = ("path", Fact::Path(file.path.clone()));
        let mode = file.target.unwrap_or(file.own).mode;
        let dir = view.file(NodeFile::SigningRecordDir)?;
        let dir = dir.map_or(0, |dir| dir.target.unwrap_or(dir.own).mode);
        if (mode | dir) & 0o077 != 0 {
            let (mode, dir) = (Fact::Mode(mode), Fact::Mode(dir));
            let values = vec![path, ("mode", mode), ("dirMode", dir)];
            return out.one(Fail, System, "wide-mode", values);
        }
        let record = match view.signing_record() {
            Ok(Some(record)) if record.known_format => record,
            Ok(None) => return absent(),
            Ok(Some(_)) => return out.one(Fail, System, "unparsed", vec![path]),
            Err(unknown) if unknown.reason == Reason::Unparsed => {
                return out.one(Fail, System, "unparsed", vec![path]);
            }
            Err(unknown) => return Err(unknown),
        };
        let chain = &view.forger_config()?.chain;
        if record.nethash != chain.nethash {
            return out.one(Fail, Chain, "other-network", vec![path]);
        }
        let now = slots::slot_at(chain, view.now_ms());
        match record.entries.iter().find(|entry| entry.slot > now) {
            Some(entry) => {
                let values = vec![("slot", int(entry.slot)), ("current", int(now))];
                out.one(Warn, Chain, "slot-ahead", values)
            }
            None => out.one(Pass, System, "pass", vec![path]),
        }
    })
}

/// `forger.start-errors`.
pub fn start_errors(input: &CheckInput<'_>) -> Vec<Finding> {
    about(input, |out| {
        let view = input.view;
        let capture = view.error_capture(Role::Forger)?;
        let state = view.process(Role::Forger)?.state;
        let down = matches!(state, RoleState::Stopped | RoleState::Errored);
        let Some(error) = capture.errors.last().filter(|_| down) else {
            return out.one(Pass, Log, "none", Vec::new());
        };
        let key = match error {
            E::KeyFileMissing => "key-file-missing",
            E::NoKeys => "no-keys",
            E::KeyFileTooOpen { .. } => "key-file-open",
            E::KeyFileSecrets => "key-file-secrets",
            E::KeyFileInvalid => "key-file-invalid",
            E::SigningRecordOtherNetwork => "record-other-network",
            E::SigningRecordInvalid => "record-invalid",
            E::ForgerOption => "option",
            E::Config => "config",
            _ => "other",
        };
        let mut values = Vec::new();
        if let E::KeyFileTooOpen { mode } = error {
            values.push(("mode", Fact::Mode(*mode)));
        }
        let path = NodeFile::KeyFile
            .path(view.scope())
            .filter(|_| key.starts_with("key-file"));
        values.extend(path.map(|path| ("path", Fact::Path(path))));
        let mut finding = out.to(Fail, Log, key, values);
        let skip = capture.excerpts.len().saturating_sub(3);
        finding.untrusted = capture.excerpts.iter().skip(skip).cloned().collect();
        Ok(vec![finding])
    })
}
