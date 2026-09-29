//! The process checks: whether the roles run, and why the relay did not start. The helpers at the
//! top are shared by the other checks of this group.

use std::time::Duration;

use crate::doctor::registry::CheckInput;
use crate::doctor::report::Basis::{self, Process};
use crate::doctor::report::Status::{self, Fail, Pass, Warn};
use crate::doctor::report::{Advice, Fact, Finding, keys};
use crate::doctor::seams::Subject::{self, Host, Relay};
use crate::doctor::seams::{RoleState, subject_of};
use crate::doctor::sources::logs::{LineKind, StartError};
use crate::doctor::view::{Reason, Source, Unknown, View};
use crate::seams::Role;

/// The input of a check.
pub(crate) type In<'a> = &'a CheckInput<'a>;
/// A finding's typed facts, by name.
pub(crate) type Facts = Vec<(&'static str, Fact)>;
/// What a check's body gives: its findings, or the source it could not read.
pub(crate) type Out = Result<Vec<Finding>, Unknown>;
/// A status and the key of its advice.
pub(crate) type Verdict = (Status, &'static str);
/// What a check about one subject decides: the basis, the verdict and the facts.
pub(crate) type Row = (Basis, Verdict, Facts);
/// A check about one subject: its row, or the source it could not read.
pub(crate) type Res = Result<Row, Unknown>;

/// Restarts from which a role that has run for less than [`RESTART_WINDOW`] warns.
const RESTARTS: u32 = 3;
/// How long a role must have run for its restarts not to count.
const RESTART_WINDOW: Duration = Duration::from_secs(600);
/// The most excerpts of the error capture a start error carries.
const EXCERPTS: usize = 3;

/// Integer facts, each named after its variable.
macro_rules! ints {
    ($($name:ident),*) => {
        vec![$((stringify!($name), $crate::doctor::checks::process::int($name))),*]
    };
}
pub(crate) use ints;

/// A finding whose advice values are its facts.
pub(crate) fn fin(input: In<'_>, subject: Subject, row: Row) -> Finding {
    let (basis, (status, key), facts) = row;
    let advice = Advice {
        key,
        values: facts.clone(),
    };
    let found = Finding::new(input.id(), subject, status, basis, advice);
    Finding { facts, ..found }
}

/// An integer fact; a value that does not fit is the largest one.
pub(crate) fn int<T: TryInto<i64>>(n: T) -> Fact {
    Fact::Int(n.try_into().unwrap_or(i64::MAX))
}

/// The `unknown` finding about `subject`.
pub(crate) fn unknown(input: In<'_>, subject: Subject, cause: Unknown) -> Vec<Finding> {
    vec![Finding::unknown(input.id(), subject, &cause)]
}

/// The `unknown` findings about every role.
pub(crate) fn unknown_roles(input: In<'_>, cause: Unknown) -> Vec<Finding> {
    Role::ALL
        .map(|r| unknown(input, subject_of(r), cause))
        .concat()
}

/// The `skipped` finding about a role that does not run here.
pub(crate) fn absent(input: In<'_>, subject: Subject) -> Vec<Finding> {
    vec![Finding::skipped(input.id(), subject, keys::SKIPPED_ABSENT)]
}

/// `body`'s findings, or the `unknown` finding about `subject` for the source it could not read.
pub(crate) fn guard(input: In<'_>, subject: Subject, body: impl FnOnce() -> Out) -> Vec<Finding> {
    body().unwrap_or_else(|cause| unknown(input, subject, cause))
}

/// The one finding about `subject` that `body` decides.
pub(crate) fn single(
    input: In<'_>,
    subject: Subject,
    body: impl FnOnce(&dyn View) -> Res,
) -> Vec<Finding> {
    let found = || Ok(vec![fin(input, subject, body(input.view)?)]);
    guard(input, subject, found)
}

/// [`guard`] for a check about the relay, `skipped` when the host does not run one.
pub(crate) fn on_relay(input: In<'_>, body: impl FnOnce() -> Out) -> Vec<Finding> {
    let here = input
        .view
        .roles_present()
        .map_or(true, |p| p.has(Role::Relay));
    if here {
        guard(input, Relay, body)
    } else {
        absent(input, Relay)
    }
}

/// [`single`] about the relay, `skipped` when the host does not run one.
pub(crate) fn relay(input: In<'_>, body: impl FnOnce(&dyn View) -> Res) -> Vec<Finding> {
    on_relay(input, || Ok(single(input, Relay, body)))
}

/// `process.roles`.
pub fn roles(input: &CheckInput<'_>) -> Vec<Finding> {
    let presence = match input.view.roles_present() {
        Ok(presence) => presence,
        Err(cause) => return unknown_roles(input, cause),
    };
    let present = Role::ALL.into_iter().filter(|role| presence.has(*role));
    let found: Vec<Finding> = present.map(|role| role_finding(input, role)).collect();
    match found.is_empty() {
        true => unknown_roles(input, Unknown::new(Source::Process, Reason::NotConfigured)),
        false => found,
    }
}

/// The finding about one role that this host runs.
fn role_finding(input: In<'_>, role: Role) -> Finding {
    let subject = subject_of(role);
    let facts = match input.view.process(role) {
        Ok(facts) => facts,
        Err(cause) => return Finding::unknown(input.id(), subject, &cause),
    };
    let (row, mut all) = (&facts.row, vec![("role", Fact::Word(role.name()))]);
    let recent = row.uptime.is_some_and(|up| up < RESTART_WINDOW);
    let verdict = match facts.state {
        RoleState::Errored => (Fail, "errored"),
        RoleState::Stopped => (Fail, "not-running"),
        RoleState::Starting | RoleState::Stopping => (Warn, "changing"),
        RoleState::Running { supervised: false } => (Warn, "unsupervised"),
        RoleState::Running { .. } if row.restarts >= RESTARTS && recent => {
            let minutes = row.uptime.map_or(0, |up| up.as_secs() / 60);
            all.extend([("restarts", int(row.restarts)), ("minutes", int(minutes))]);
            (Warn, "restarting")
        }
        RoleState::Running { .. } => (Pass, "running"),
    };
    fin(input, subject, (Process, verdict, all))
}

/// `process.core-role`: the role is not in this release.
pub fn core_role(input: &CheckInput<'_>) -> Vec<Finding> {
    vec![Finding::skipped(input.id(), Host, keys::SKIPPED_RELEASE)]
}

/// The last start error the error capture or the log shows.
fn last_error(view: &dyn View) -> Result<Option<StartError>, Unknown> {
    let (capture, log) = (view.error_capture(Role::Relay), view.log(Role::Relay));
    let captured = capture.iter().flat_map(|c| c.errors.iter().copied());
    let events = log.iter().flat_map(|window| window.events.iter());
    let mut logged = events.filter_map(|e| match e.kind {
        LineKind::StartError(error) => Some(error),
        _ => None,
    });
    match (captured.last().or(logged.next_back()), capture, log) {
        (None, Err(cause), Err(_)) => Err(cause),
        (found, ..) => Ok(found),
    }
}

/// `relay.start-errors`.
pub fn relay_start_errors(input: &CheckInput<'_>) -> Vec<Finding> {
    on_relay(input, || {
        let view = input.view;
        let state = view.process(Role::Relay)?.state;
        let stopped = matches!(state, RoleState::Stopped | RoleState::Errored);
        let error = if stopped { last_error(view)? } else { None };
        let Some(error) = error else {
            let none = (Process, (Pass, "none"), Vec::new());
            return Ok(vec![fin(input, Relay, none)]);
        };
        let config = view.relay_config().ok();
        let api = config.and_then(|c| c.api).map(|a| a.port());
        let (verdict, facts) = describe(error, (config.map(|c| c.p2p.port()), api));
        let mut found = fin(input, Relay, (Process, verdict, facts));
        let capture = view.error_capture(Role::Relay);
        let excerpts = capture.iter().flat_map(|c| c.excerpts.iter());
        found.untrusted = excerpts.take(EXCERPTS).cloned().collect();
        Ok(vec![found])
    })
}

/// The verdict and the advice values of a start error; `ports` are the configured P2P and API
/// ports, for a line that names none.
fn describe(error: StartError, ports: (Option<u16>, Option<u16>)) -> (Verdict, Facts) {
    let known = |key, general, port: Option<u16>| match port {
        Some(port) => (key, vec![("port", Fact::Port(port))]),
        None => (general, Vec::new()),
    };
    let (key, facts) = match error {
        StartError::P2pCannotListen { port, in_use: true } => {
            known("p2p-in-use", "p2p-cannot-listen", port.or(ports.0))
        }
        StartError::P2pCannotListen { .. } => ("p2p-cannot-listen", Vec::new()),
        StartError::ApiCannotListen { port, in_use: true } => {
            known("api-in-use", "api-cannot-listen", port.or(ports.1))
        }
        StartError::ApiCannotListen { .. } => ("api-cannot-listen", Vec::new()),
        StartError::VersionFloor => ("version-floor", Vec::new()),
        StartError::ApiHttps => ("api-https", Vec::new()),
        StartError::Config => ("config", Vec::new()),
        _ => ("other", Vec::new()),
    };
    ((Fail, key), facts)
}
