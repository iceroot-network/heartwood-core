//! The reference nodes' acceptance of the Heartwood relays: each reference node that is not the
//! fresh one logged "Accepted new peer" for the relay's P2P address and listed the relay in every
//! peer poll outside the fault windows of either, with its API port the devnet's.

use std::collections::BTreeSet;

use super::{Poll, SECTION, fresh_node, guard, heartwood, polls, references, step};
use crate::errors::Result;
use crate::live::nodes::read_logs;
use crate::live::run::{self, Node, Window};
use crate::live::{Cases, MixedRun};

/// The relay's peer-to-peer port.
const P2P_PORT: u16 = 6002;

/// The API port a peer list must give.
const API_PORT: u64 = 6003;

/// The times a log shows "Accepted new peer <ip>:6002".
fn accepted_at(reader_log: &[crate::live::nodes::LogLine], ip: &str) -> Vec<Option<i64>> {
    let want = format!("{ip}:{P2P_PORT}");
    reader_log
        .iter()
        .filter(|l| {
            l.text
                .strip_prefix("Accepted new peer ")
                .and_then(|rest| rest.split_whitespace().next())
                == Some(want.as_str())
        })
        .map(|l| l.at)
        .collect()
}

/// Whether `t` is inside a window that concerns `a` or `b`.
fn in_window(windows: &[Window], a: &str, b: &str, t: i64) -> bool {
    windows
        .iter()
        .any(|w| (w.concerns(a) || w.concerns(b)) && w.contains(t))
}

/// Every reference node that reaches a Heartwood relay accepted it and lists it outside the fault
/// windows of both, with its API port recorded.
pub(in crate::live) fn acceptance(run: &MixedRun<'_>, cases: &mut Cases) -> Result<()> {
    guard(
        cases,
        SECTION,
        "the reference nodes' acceptance of the Heartwood relays",
        |cases| {
            let m = run.manifest;
            let relays = heartwood(m);
            if relays.is_empty() {
                cases.record(
                    SECTION,
                    "no Heartwood relay in this run: no acceptance to check",
                );
                return Ok(());
            }
            let fresh = fresh_node(m);
            let readers: Vec<&Node> = references(m)
                .into_iter()
                .filter(|n| Some(n.name.as_str()) != fresh)
                .collect();
            let polls = polls(run.run_dir)?;
            let steps = run::steps(run.run_dir)?;
            let paused = step(&steps, "pause").and_then(|s| s.start);
            // The final pause's window is not followed by any poll.
            let closing = |w: &Window| w.fault == "pause" || paused.is_some_and(|p| w.start >= p);
            for reader in readers {
                let mine: Vec<&Poll> = polls.iter().filter(|p| p.node == reader.name).collect();
                let log = read_logs(&run.run_dir.join("logs").join(&reader.name))?;
                for relay in &relays {
                    let label = format!("{} accepts {}", reader.name, relay.name);
                    let mut problems: Vec<String> = Vec::new();
                    if mine.is_empty() {
                        problems.push(format!("no poll of {}'s peer list", reader.name));
                    }
                    let accepted = accepted_at(&log, &relay.ip);
                    if accepted.is_empty() {
                        problems.push(format!(
                            "never logged \"Accepted new peer {}:{P2P_PORT}\"",
                            relay.ip
                        ));
                    }
                    let mut outside = 0usize;
                    let mut wrong_ports: BTreeSet<u64> = BTreeSet::new();
                    for p in &mine {
                        for e in p.peers.iter().filter(|e| e.ip == relay.ip) {
                            wrong_ports.extend(e.api.iter().filter(|a| **a != API_PORT));
                        }
                        if in_window(run.windows, &reader.name, &relay.name, p.at) {
                            continue;
                        }
                        outside += 1;
                        if !p.lists(&relay.ip) {
                            problems.push(format!(
                                "the poll at {} does not list {}",
                                run::format_time(p.at),
                                relay.name
                            ));
                        }
                    }
                    for port in wrong_ports {
                        problems.push(format!(
                            "{} lists {}'s API on port {port}, not {API_PORT}",
                            reader.name, relay.name
                        ));
                    }
                    for w in run.windows.iter().filter(|w| {
                        (w.concerns(&reader.name) || w.concerns(&relay.name)) && !closing(w)
                    }) {
                        let followed = mine.iter().any(|p| p.at >= w.end && p.lists(&relay.ip));
                        if !followed {
                            problems.push(format!(
                                "no poll after the {} window of {} lists {}",
                                w.fault,
                                run::format_time(w.start),
                                relay.name
                            ));
                        }
                        let missing_inside =
                            mine.iter().any(|p| w.contains(p.at) && !p.lists(&relay.ip));
                        let back = accepted.iter().any(|a| a.is_some_and(|t| t >= w.start));
                        if missing_inside && !back {
                            problems.push(format!(
                            "{} did not list {} in the {} window of {} and logged no acceptance after its start",
                            reader.name,
                            relay.name,
                            w.fault,
                            run::format_time(w.start)
                        ));
                        }
                    }
                    if problems.is_empty() {
                        cases.pass(
                        SECTION,
                        format!(
                            "{label}: accepted on {P2P_PORT}, listed in {outside} polls outside the windows, API port {API_PORT}"
                        ),
                    );
                    } else {
                        for p in problems.iter().take(6) {
                            cases.fail(SECTION, label.clone(), p.clone());
                        }
                    }
                }
            }
            Ok(())
        },
    )
}
