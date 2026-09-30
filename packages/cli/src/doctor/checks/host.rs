//! The host: its size against the planning estimates, its free memory and the user running the
//! doctor.

use crate::doctor::checks::process::{either, fin, guard, int, single};
use crate::doctor::registry::CheckInput;
use crate::doctor::report::Basis::System;
use crate::doctor::report::Status::{Fail, Pass, Warn};
use crate::doctor::report::{Fact, Finding};
use crate::doctor::seams::Subject::Host;
use crate::doctor::view::{NodeDir, NodeFile, Reason, Source, Unknown};
use crate::seams::Role;

/// The planning estimates, shipped as data.
const PLANNING: &str = include_str!("../docs/planning.json");
/// The share of the estimated memory a host must show: the kernel keeps some of what is installed.
const MEMORY_PERCENT: u64 = 90;
/// The bytes in a GiB.
const GIB: u64 = 1 << 30;

/// A role's planning estimate: its CPUs and its memory in GiB.
fn estimate(planning: &serde_json::Value, role: Role) -> Option<(u64, u64)> {
    let entry = planning.get("roles")?.get(role.name())?;
    Some((
        entry.get("vcpus")?.as_u64()?,
        entry.get("memory_gb")?.as_u64()?,
    ))
}

/// `host.resources`: never fails; the estimates are for planning.
pub fn resources(input: &CheckInput<'_>) -> Vec<Finding> {
    single(input, Host, |view| {
        let cpus = view
            .host()
            .cpus
            .ok_or(Unknown::new(Source::Proc, Reason::Unparsed))?;
        let (memory, presence) = (view.meminfo()?.total, view.roles_present()?);
        let unparsed = Unknown::new(Source::Build, Reason::Unparsed);
        let planning: serde_json::Value = serde_json::from_str(PLANNING).map_err(|_| unparsed)?;
        let present: Vec<Role> = Role::ALL
            .into_iter()
            .filter(|role| presence.has(*role))
            .collect();
        let mut need = (0, 0);
        for role in &present {
            let (vcpus, gb) = estimate(&planning, *role).ok_or(unparsed)?;
            need = (need.0 + vcpus, need.1 + gb);
        }
        let roles = match present.as_slice() {
            [] => return Err(Unknown::new(Source::Process, Reason::NotConfigured)),
            [Role::Relay] => "relay",
            [_] => "forger",
            _ => "relay-and-forger",
        };
        let wanted = u128::from(need.1) * u128::from(GIB * MEMORY_PERCENT);
        let enough = u64::from(cpus) >= need.0 && u128::from(memory) * 100 >= wanted;
        let facts = vec![
            ("cpus", int(cpus)),
            ("memory_bytes", Fact::Bytes(memory)),
            ("roles", Fact::Word(roles)),
            ("need_cpus", int(need.0)),
            ("need_memory_bytes", Fact::Bytes(need.1.saturating_mul(GIB))),
        ];
        Ok((
            System,
            either(enough, (Pass, "enough"), (Warn, "below-estimate")),
            facts,
        ))
    })
}

/// `host.memory`: low available memory, and swap in use by a role.
pub fn memory(input: &CheckInput<'_>) -> Vec<Finding> {
    guard(input, Host, || {
        let view = input.view;
        let mem = view.meminfo()?;
        if mem.total == 0 {
            return Err(Unknown::new(Source::Proc, Reason::Unparsed));
        }
        // In u128, so that no value of /proc/meminfo overflows.
        let (available, total) = (u128::from(mem.available) * 100, u128::from(mem.total));
        let low = match () {
            _ if available < total * 2 => Some((Fail, "exhausted")),
            _ if available <= total * 10 => Some((Warn, "low")),
            _ => None,
        };
        let done = |verdict, facts| Ok(vec![fin(input, Host, (System, verdict, facts))]);
        let percent = vec![("percent", int(available / total))];
        if let Some(verdict) = low {
            return done(verdict, percent);
        }
        for role in Role::ALL {
            match view.role_status(role) {
                Ok(status) if status.swap_bytes.unwrap_or(0) > 0 => {
                    return done((Warn, "swap"), vec![("role", Fact::Word(role.name()))]);
                }
                Err(unknown) if unknown.reason != Reason::NotRunning => return Err(unknown),
                _ => {}
            }
        }
        done((Pass, "enough"), percent)
    })
}

/// `host.user`.
pub fn user(input: &CheckInput<'_>) -> Vec<Finding> {
    single(input, Host, |view| {
        let uid = view.own_uid()?;
        let data = view.file(NodeFile::Dir(NodeDir::Data))?;
        let holder = data
            .ok_or(Unknown::new(Source::Files, Reason::Missing))?
            .own
            .uid;
        let verdict = match (uid, holder) {
            (0, _) => (Warn, "root"),
            (uid, holder) if uid != holder => (Warn, "other-user"),
            _ => (Pass, "node-user"),
        };
        Ok((
            System,
            verdict,
            vec![("uid", int(uid)), ("holder", int(holder))],
        ))
    })
}
