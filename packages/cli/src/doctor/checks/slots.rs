//! Slots: whether our delegates are active, and the slots they missed with their likely causes.
//!
//! This file also holds what the forger and quorum checks share: our delegates, the chain's slot
//! arithmetic, the store window and the finding builder. The store counts time in seconds since
//! the network epoch (the milestones' `epoch`), never the Unix epoch, and the window is bounded by
//! height, the store's indexed column: a store second is converted to Unix milliseconds before
//! any log time is compared with it.

use std::collections::BTreeSet;

use heartwood_crypto::crypto::slots;
use heartwood_kernel::log::Level;

use crate::doctor::DoctorOptions;
use crate::doctor::registry::{CheckId, CheckInput};
use crate::doctor::report::Basis::{Chain, Log};
use crate::doctor::report::Status::{Fail, Pass, Warn};
use crate::doctor::report::{Advice, Basis, Fact, Finding, Id, IdKind, Status, keys};
use crate::doctor::seams::{ChainFacts, MissedSlot, RoleState, StoreWindow, StoredBlock, Subject};
use crate::doctor::sources::logs::{LineKind as K, LogWindow};
use crate::doctor::view::{Reason, Source, Unknown, View};
use crate::seams::Role;

/// A check body's answer: its findings, or the source that gave none.
pub(super) type Found = Result<Vec<Finding>, Unknown>;
/// The store window, or the source that gave none.
pub(super) type Stored = Result<StoreWindow, Unknown>;
/// A finding's typed values, by name: its facts and its advice's values.
pub(super) type Values = Vec<(&'static str, Fact)>;

/// The missed slots named one by one, the latest.
const MAX_LISTED: usize = 10;
/// Our slots a window must hold before the missed share counts: 1% of fewer is one slot.
const RATE_SLOTS: usize = 100;
/// The most signing-record keys looked up through the API, for its request budget.
const MAX_LOOKUPS: usize = 4;
/// The most blocks a window asks the store for: below the store's cap of 5,000 rows, which
/// keeps the oldest rows, with room for the blocks forged while the doctor runs. A longer window
/// is judged on its latest blocks.
pub const MAX_WINDOW_BLOCKS: u32 = 4_900;

/// The findings of one check about one subject.
#[derive(Debug, Clone, Copy)]
pub(super) struct Out {
    id: CheckId,
    subject: Subject,
}

impl Out {
    /// The same check about `subject`.
    pub fn about(self, subject: Subject) -> Out {
        Out { subject, ..self }
    }

    /// A finding whose facts are its advice's values.
    pub fn to(self, status: Status, basis: Basis, key: &'static str, values: Values) -> Finding {
        let mut advice = Advice::key(key);
        advice.values = values.clone();
        let mut finding = Finding::new(self.id, self.subject, status, basis, advice);
        finding.facts = values;
        finding
    }

    /// That finding alone.
    pub fn one(self, status: Status, basis: Basis, key: &'static str, values: Values) -> Found {
        Ok(vec![self.to(status, basis, key, values)])
    }
}

/// The findings of a check about the forger: skipped when no forger is on this host, `unknown`
/// when a source the body needs gave nothing.
pub(super) fn about(input: &CheckInput<'_>, body: impl FnOnce(Out) -> Found) -> Vec<Finding> {
    let (id, subject) = (input.id(), Subject::Forger);
    let out = Out { id, subject };
    let answer = match input.view.roles_present() {
        Ok(present) if !present.has(Role::Forger) => {
            return vec![Finding::skipped(out.id, out.subject, keys::SKIPPED_ABSENT)];
        }
        Ok(_) => body(out),
        Err(unknown) => Err(unknown),
    };
    answer.unwrap_or_else(|unknown| match unknown.reason {
        Reason::NoDelegates => vec![out.to(Status::Unknown, Log, "no-delegates", Vec::new())],
        _ => vec![Finding::unknown(out.id, out.subject, &unknown)],
    })
}

/// A count as a fact.
pub(super) fn int(count: impl TryInto<i64>) -> Fact {
    Fact::Int(count.try_into().unwrap_or(i64::MAX))
}

/// A count and the time of the latest.
pub(super) fn counted(count: usize, at_ms: i64) -> Values {
    vec![("count", int(count)), ("time", Fact::Time(at_ms))]
}

/// A delegate's name as the fact `delegate`.
pub(super) fn named_as(text: &str) -> (&'static str, Fact) {
    ("delegate", name(text))
}

/// A delegate's name as a fact (names reach here only after passing the username grammar).
pub(super) fn name(text: &str) -> Fact {
    Id::new(IdKind::Username, text).map_or(Fact::Word("a delegate"), Fact::Id)
}

/// Our delegates: their names, and the public keys the forger's signing record holds.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct Ours {
    /// The names, sorted.
    pub names: Vec<String>,
    /// The signing record's public keys.
    pub keys: Vec<String>,
}

impl Ours {
    /// Whether `block` was signed with one of our keys.
    pub fn holds(&self, block: &StoredBlock) -> bool {
        self.keys.contains(&block.generator_public_key) || self.named(block.username.as_deref())
    }

    /// Whether `username` is one of ours.
    pub fn named(&self, username: Option<&str>) -> bool {
        username.is_some_and(|name| self.names.iter().any(|ours| ours == name))
    }
}

/// Our delegates, in this order: the `--delegate` names; the names of the forger's "Loaded"
/// and "active forging position" lines since its last start, before the window included; the
/// signing record's keys, looked up through the API.
pub(super) fn ours(view: &dyn View, options: &DoctorOptions) -> Ours {
    let valid = |name: &String| Id::new(IdKind::Username, name).is_some();
    let mut names = BTreeSet::new();
    names.extend(options.delegates.iter().filter(|n| valid(n)).cloned());
    if let Some(log) = view.log(Role::Forger).ok().filter(|_| names.is_empty()) {
        for event in log.roster.iter().chain(&log.events) {
            names.extend(listed(&event.kind).iter().map(|id| id.as_str().to_owned()));
        }
    }
    let mut keys = Vec::new();
    if let Ok(Some(record)) = view.signing_record() {
        keys.extend(record.entries.iter().map(|entry| entry.public_key.clone()));
    }
    let lookups = if names.is_empty() { MAX_LOOKUPS } else { 0 };
    for key in keys.iter().take(lookups) {
        let wallet = view.api_wallet(key).ok().flatten();
        names.extend(wallet.and_then(|wallet| wallet.username).filter(valid));
    }
    let names = names.into_iter().collect();
    Ours { names, keys }
}

/// The names a "Loaded" or "active forging position" line lists.
fn listed(kind: &K) -> &[Id] {
    match kind {
        K::DelegatesLoaded { names, .. } | K::ActivePosition { names, .. } => names,
        _ => &[],
    }
}

/// The delegate and height of a "forged a new block" line.
pub(super) fn forged(kind: &K) -> Option<(&str, u32)> {
    match kind {
        K::Forged {
            username, height, ..
        } => Some((username.as_str(), *height)),
        _ => None,
    }
}

/// The chain definition: the relay's, else the forger's.
pub(super) fn chain(view: &dyn View) -> Result<&ChainFacts, Unknown> {
    match view.relay_config() {
        Ok(relay) => Ok(&relay.chain),
        Err(_) => view.forger_config().map(|forger| &forger.chain),
    }
}

/// A slot's length in milliseconds.
pub(super) fn slot_ms(chain: &ChainFacts) -> i64 {
    i64::from(chain.block_time_s.max(1)).saturating_mul(1_000)
}

/// The slot the Unix time `unix_ms` falls in.
pub(super) fn slot_at(chain: &ChainFacts, unix_ms: i64) -> i64 {
    let since = slots::epoch_time(unix_ms, chain.epoch_ms);
    slots::slot_number(since, u64::from(chain.block_time_s))
}

/// The slot of a store timestamp, in seconds since the network epoch.
pub(super) fn slot_of(chain: &ChainFacts, epoch_s: u32) -> i64 {
    slots::slot_number(i64::from(epoch_s), u64::from(chain.block_time_s))
}

/// A store timestamp, in seconds since the network epoch, as Unix milliseconds.
pub(super) fn unix_ms(chain: &ChainFacts, epoch_s: u32) -> i64 {
    let since = i64::from(epoch_s).saturating_mul(1_000);
    chain.epoch_ms.saturating_add(since)
}

/// The store from the height the window reaches back to (the tip less the window's blocks, at
/// most [`MAX_WINDOW_BLOCKS`]), with our delegates' missed rows. Every check asks with the same
/// bounds, so the store is read once.
pub(super) fn store(view: &dyn View, ours: &Ours, chain: &ChainFacts) -> Stored {
    let tip = match view.api_last_block() {
        Ok(block) => block.height,
        Err(_) => view.api_status()?.now,
    };
    let window = view.window();
    let back = window.to_ms.saturating_sub(window.from_ms) / slot_ms(chain);
    let back = u32::try_from(back.saturating_add(1)).unwrap_or(u32::MAX);
    view.store_window(&ours.names, tip.saturating_sub(back.min(MAX_WINDOW_BLOCKS)))
}

/// `slots.active`.
pub fn active(input: &CheckInput<'_>) -> Vec<Finding> {
    about(input, |out| {
        let log = input.view.log(Role::Forger)?;
        log.shows(Level::Info)?;
        let ours = ours(input.view, input.options);
        if ours.names.is_empty() {
            return out.one(Status::Skipped, Log, "no-delegates", Vec::new());
        }
        let (mut findings, mut count) = (Vec::new(), 0);
        for delegate in &ours.names {
            let named = |names: &[Id]| names.iter().any(|name| name.as_str() == delegate);
            // The latest line that shows the delegate in or out of the active set.
            let lines = log.roster.iter().chain(&log.events);
            let said = lines.rev().find_map(|event| match &event.kind {
                K::DelegatesLoaded { active, .. } => named(listed(&event.kind)).then_some(*active),
                K::ActivePosition { entered, .. } => named(listed(&event.kind)).then_some(*entered),
                K::DueToForge { username, .. } | K::Forged { username, .. } => {
                    (username.as_str() == delegate).then_some(true)
                }
                _ => None,
            });
            match said {
                Some(true) => count += 1,
                Some(false) => findings.push(out.to(Warn, Log, "left", vec![named_as(delegate)])),
                None => {}
            }
        }
        match (findings.is_empty(), count) {
            (false, _) => Ok(findings),
            (true, 0) => out.one(Status::Unknown, Log, "no-evidence", Vec::new()),
            (true, _) => out.one(Pass, Log, "active", vec![("count", int(count))]),
        }
    })
}

/// `slots.missed`.
pub fn missed(input: &CheckInput<'_>) -> Vec<Finding> {
    about(input, |out| {
        let view = input.view;
        let ours = ours(view, input.options);
        if ours.names.is_empty() {
            return Err(Unknown::new(Source::ForgerLog, Reason::NoDelegates));
        }
        let chain = chain(view)?;
        let store = store(view, &ours, chain)?;
        // The cap keeps the oldest rows: the latest slots, which the rules judge, are cut.
        if store.capped {
            return Err(Unknown::new(Source::Store, Reason::TooLarge));
        }
        let log = view.log(Role::Forger).ok();
        let ours_only = |slot: &&MissedSlot| ours.named(Some(&slot.username));
        let missed: Vec<&MissedSlot> = store.missed.iter().filter(ours_only).collect();
        // Our slots in time order: the missed ones and the blocks our keys signed.
        let mut taken = Vec::new();
        taken.extend(missed.iter().map(|slot| (slot.timestamp, true)));
        let forged = store.blocks.iter().filter(|block| ours.holds(block));
        taken.extend(forged.map(|block| (block.timestamp, false)));
        taken.sort_unstable();
        let rounds_s = 5 * i64::from(chain.active_delegates) * i64::from(chain.block_time_s);
        let since = slots::epoch_time(view.now_ms(), chain.epoch_ms).saturating_sub(rounds_s);
        let recent = missed.iter().filter(|m| i64::from(m.timestamp) >= since);
        let recent = recent.count();
        let last_two = taken.len() >= 2 && taken.iter().rev().take(2).all(|(_, gone)| *gone);
        let share = taken.len() >= RATE_SLOTS && missed.len() * 100 >= taken.len();
        let status = match (last_two || recent >= 3, recent > 0 || share) {
            (true, _) => Fail,
            (false, true) => Warn,
            _ => Pass,
        };
        let listed = missed.iter().skip(missed.len().saturating_sub(MAX_LISTED));
        let caused = listed.map(|slot| cause(view, chain, log, slot).to(out, status, chain, slot));
        let mut findings: Vec<Finding> = caused.collect();
        if findings.is_empty() {
            findings.push(out.to(status, Chain, "none", vec![("slots", int(taken.len()))]));
        }
        findings.extend(log.and_then(|log| lost(out, &ours, &store, log)));
        Ok(findings)
    })
}

/// A missed slot's cause: its advice key, and the height of the block the forger signed in it.
struct Cause(&'static str, Option<u32>);

impl Cause {
    /// The finding of `slot` missed for this cause.
    fn to(self, out: Out, status: Status, chain: &ChainFacts, slot: &MissedSlot) -> Finding {
        let at = Fact::Time(unix_ms(chain, slot.timestamp));
        let mut values = vec![("time", at), named_as(&slot.username)];
        values.extend(self.1.map(|height| ("height", int(height))));
        out.to(status, Chain, self.0, values)
    }
}

/// The likely cause of a missed slot: the forger's own line inside the slot's seconds, else
/// whether it ran then.
fn cause(view: &dyn View, chain: &ChainFacts, log: Option<&LogWindow>, slot: &MissedSlot) -> Cause {
    let Some(log) = log else {
        return Cause("cause-unread", None);
    };
    let from = unix_ms(chain, slot.timestamp);
    let to = from.saturating_add(slot_ms(chain));
    let inside = log.events.iter().filter(|e| (from..to).contains(&e.at_ms));
    let ranked = inside.filter_map(|event| match &event.kind {
        K::Forged { .. } => Some((0, "cause-not-on-chain", forged(&event.kind).map(|f| f.1))),
        K::NotEnoughQuorum | K::NotPeeredEnough | K::ColdStartSkip | K::NoNetworkState => {
            Some((1, "cause-quorum", None))
        }
        K::TooLittleTime { .. } | K::AlreadyNextSlot { .. } | K::WillNotSign { .. } => {
            Some((2, "cause-late", None))
        }
        K::ForgerStarted | K::ForgerManagerStarted => Some((3, "cause-not-running", None)),
        _ => None,
    });
    if let Some((_, key, height)) = ranked.min_by_key(|(rank, ..)| *rank) {
        return Cause(key, height);
    }
    // No line: not running when it has stopped since before the slot, or started inside it.
    let process = view.process(Role::Forger).ok();
    let started = process.and_then(|facts| facts.supervisor.as_ref()?.started_at_ms);
    let stopped = process.is_some_and(|facts| {
        matches!(facts.state, RoleState::Stopped | RoleState::Errored)
            && log.covered.last_ms.is_none_or(|last| last < from)
    });
    match stopped || started.is_some_and(|at| (from..=to).contains(&at)) {
        true => Cause("cause-not-running", None),
        false => Cause("cause-none", None),
    }
}

/// The blocks the forger logged as forged whose height the chain holds with another generator,
/// or not at all: a forger signing outside its slot, or blocks the network refused. One is a
/// warn, two or more a fail. The chain only fails to hold them, which is true of every height
/// this forger did not sign, so the finding rests on the log lines alone.
fn lost(out: Out, ours: &Ours, store: &StoreWindow, log: &LogWindow) -> Option<Finding> {
    let heights = store.blocks.iter().map(|block| block.height);
    let (low, high) = (heights.clone().min()?, heights.max()?);
    let blocks = &store.blocks;
    let held = |height: &u32| blocks.iter().any(|b| b.height == *height && ours.holds(b));
    let mut lost = Vec::new();
    for (name, height) in log.events.iter().filter_map(|event| forged(&event.kind)) {
        if ours.named(Some(name)) && (low..=high).contains(&height) && !held(&height) {
            lost.push(height);
        }
    }
    let status = if lost.len() > 1 { Fail } else { Warn };
    let values = vec![("count", int(lost.len())), ("height", int(*lost.last()?))];
    Some(out.to(status, Log, "not-on-chain", values))
}
