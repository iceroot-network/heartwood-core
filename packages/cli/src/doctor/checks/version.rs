//! Versions: the version floor, the installed and running versions, this build's features and the
//! peers' versions.

use crate::doctor::checks::process::{
    Facts, absent, either, fin, guard, int, on_relay, relay, unknown_roles,
};
use crate::doctor::registry::CheckInput;
use crate::doctor::report::Basis::{Chain, Config, Process};
use crate::doctor::report::Status::{Fail, Pass, Warn};
use crate::doctor::report::{Fact, Finding, Id, IdKind, Untrusted, UntrustedSource};
use crate::doctor::seams::Subject::{Host, Relay};
use crate::doctor::seams::{RoleState, subject_of};
use crate::doctor::view::{Reason, Source, Unknown, View};
use crate::seams::Role;

/// The most peer versions kept as untrusted text.
const EXCERPTS: usize = 3;

/// A valid version, as a fact.
fn version(text: &str) -> Option<Id> {
    Id::new(IdKind::Version, text)
}

/// The API's `core.version`, when it is a valid version.
fn core_version(view: &dyn View) -> Option<String> {
    let config = view.api_configuration().ok()?;
    config
        .core_version
        .clone()
        .filter(|text| version(text).is_some())
}

/// The version the relay runs: the API's, else the supervisor's record, else this build's.
fn relay_version(view: &dyn View) -> String {
    let process = view.process(Role::Relay).ok();
    let recorded = process
        .and_then(|p| p.supervisor.as_ref())
        .map(|s| s.version.clone())
        .filter(|text| version(text).is_some());
    core_version(view)
        .or(recorded)
        .unwrap_or_else(|| view.build().version.into())
}

/// `version.floor`: the tip's height from the API, else from the store (the relay's API may be off).
pub fn floor(input: &CheckInput<'_>) -> Vec<Finding> {
    relay(input, |view| {
        let text = relay_version(view);
        let height = match view.api_status() {
            Ok(status) => status.now,
            Err(_) => (view.api_last_block().map(|block| block.height))
                .or_else(|_| super::slots::store_tip(view, input.options))?,
        };
        let verdict = view.floor(&text, height)?;
        let mut facts: Facts = vec![("height", int(height))];
        facts.extend(version(&text).map(|v| ("version", Fact::Id(v))));
        if verdict.refused_now {
            return Ok((Config, (Fail, "refused"), facts));
        }
        let Some(from) = verdict.refused_from.iter().min() else {
            return Ok((Config, (Pass, "ok"), facts));
        };
        facts.push(("from", int(*from)));
        let seconds = view.api_configuration().ok().and_then(|c| c.block_time_s);
        let configured = view.relay_config().ok().map(|c| c.chain.block_time_s);
        let Some(seconds) = seconds.or(configured) else {
            return Ok((Config, (Warn, "later-height"), facts));
        };
        let secs = u64::from(from.saturating_sub(height)) * u64::from(seconds);
        facts.extend([
            ("days", int(secs / 86_400)),
            ("hours", int(secs % 86_400 / 3_600)),
        ]);
        Ok((Config, (Warn, "later"), facts))
    })
}

/// `version.installed`: each running role against the installed build. A relay and a forger that
/// differ from each other differ from the build too, so the per-role comparison covers that.
pub fn installed(input: &CheckInput<'_>) -> Vec<Finding> {
    let (view, installed) = (input.view, input.view.build().version);
    let presence = match view.roles_present() {
        Ok(presence) => presence,
        Err(cause) => {
            return unknown_roles(input, cause);
        }
    };
    let judge = |role: Role| {
        let subject = subject_of(role);
        guard(input, subject, || {
            if !presence.has(role) {
                return Ok(absent(input, subject));
            }
            let process = view.process(role)?;
            if process.state == RoleState::Stopped {
                return Ok(absent(input, subject));
            }
            let api = core_version(view).filter(|_| role == Role::Relay);
            let text = api.unwrap_or_else(|| process.row.version.clone());
            let bad = |source| Unknown::new(source, Reason::Unparsed);
            let running = version(&text).ok_or(bad(Source::Process))?;
            let latest = version(installed).ok_or(bad(Source::Build))?;
            let verdict = either(text == installed, (Pass, "same"), (Warn, "restart-pending"));
            let (role, running) = (Fact::Word(role.name()), Fact::Id(running));
            let facts = vec![("role", role), ("running", running)];
            let facts = [facts, vec![("installed", Fact::Id(latest))]].concat();
            Ok(vec![fin(input, subject, (Process, verdict, facts))])
        })
    };
    Role::ALL.into_iter().flat_map(judge).collect()
}

/// `version.build`: a build with test-only code fails; a devnet build is acceptable.
pub fn build(input: &CheckInput<'_>) -> Vec<Finding> {
    let features = input.view.build().features;
    let has = |name| features.contains(&name);
    let verdict = either(
        has("difftest"),
        (Fail, "test-build"),
        (Pass, "release-build"),
    );
    let facts = vec![
        ("devnet", Fact::Bool(has("devnet"))),
        ("difftest", Fact::Bool(has("difftest"))),
    ];
    vec![fin(input, Host, (Config, verdict, facts))]
}

/// The digit runs of a version, for comparing releases by number.
fn digits(text: &str) -> Vec<u64> {
    let runs = text.split(|c: char| !c.is_ascii_digit());
    runs.filter(|run| !run.is_empty())
        .map(|run| run.parse().unwrap_or(0))
        .collect()
}

/// `version.peers`.
pub fn peers(input: &CheckInput<'_>) -> Vec<Finding> {
    on_relay(input, || {
        let view = input.view;
        let peers = &view.api_peers()?.peers;
        if peers.is_empty() {
            return Err(Unknown::new(Source::Api, Reason::Missing));
        }
        let (ours, mut newer, mut odd) = (digits(view.build().version), 0, Vec::new());
        for text in peers
            .iter()
            .map(|peer| peer.version.as_deref().unwrap_or_default())
        {
            match version(text) {
                Some(_) if digits(text) > ours => newer += 1,
                Some(_) => {}
                None => odd.push(text),
            }
        }
        let verdict = either(
            newer * 2 > peers.len(),
            (Warn, "mostly-newer"),
            (Pass, "current"),
        );
        let facts = vec![("newer", int(newer)), ("total", int(peers.len()))];
        let mut found = fin(input, Relay, (Chain, verdict, facts));
        let at_ms = Some(view.now_ms());
        let source = UntrustedSource::PeerVersion;
        let odd = odd
            .into_iter()
            .filter(|text| !text.is_empty())
            .take(EXCERPTS);
        found.untrusted = odd
            .map(|t| Untrusted {
                source,
                at_ms,
                text: t.to_owned(),
            })
            .collect();
        Ok(vec![found])
    })
}
