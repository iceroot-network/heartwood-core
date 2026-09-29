//! The reference nodes' acceptance of the Heartwood relays: each reference node that can reach a
//! relay logged "Accepted new peer" for the relay's P2P address and listed the relay in every peer
//! poll outside the fault windows of either (each window extended by one discovery interval), with
//! its API port the devnet's.
//!
//! Which relays a node can reach is the run's route table: node 5 of runs `w0`, `w` and `p`
//! reaches node 4 alone for the whole run; run `s`'s node 4 reaches node 1 alone until its routes
//! are opened (the step `open-routes-node4`), and every node after that.

use std::collections::BTreeSet;

use super::{Poll, SECTION, guard, heartwood, polls, references, step};
use crate::errors::Result;
use crate::live::nodes::read_logs;
use crate::live::run::{self, Node, Step, Window};
use crate::live::{Cases, MixedRun};

/// The relay's peer-to-peer port.
const P2P_PORT: u16 = 6002;

/// The API port a peer list must give.
const API_PORT: u64 = 6003;

/// The discovery interval by which each window is extended, in milliseconds.
const DISCOVERY_MS: i64 = 30_000;

/// The time a node has to list the nodes its opened routes let it reach (one minute), in
/// milliseconds.
const LISTING_MS: i64 = 60_000;

/// Whether a reference node can reach a relay, by the run's route table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Reach {
    /// For the whole run.
    Always,
    /// Never: the node has no route to the relay.
    Never,
    /// From the time its routes were opened; `None` when the run records no such step.
    From(Option<i64>),
}

/// Whether `reader` can reach `relay` in run `run`, whose steps are `steps`.
pub(super) fn reach(run: &str, steps: &[Step], reader: &str, relay: &str) -> Reach {
    match (run, reader) {
        ("w0" | "w" | "p", "node5") if relay == "node4" => Reach::Always,
        ("w0" | "w" | "p", "node5") => Reach::Never,
        ("s", "node4") if relay == "node1" => Reach::Always,
        ("s", "node4") => Reach::From(step(steps, "open-routes-node4").and_then(|s| s.start)),
        _ => Reach::Always,
    }
}

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

/// Whether `w` concerns `a` or `b`.
fn concerns(w: &Window, a: &str, b: &str) -> bool {
    w.concerns(a) || w.concerns(b)
}

/// Whether `t` is inside a window that concerns `a` or `b`, extended by one discovery interval.
fn in_window(windows: &[Window], a: &str, b: &str, t: i64) -> bool {
    windows
        .iter()
        .any(|w| concerns(w, a, b) && t >= w.start && t <= w.end.saturating_add(DISCOVERY_MS))
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
            let readers: Vec<&Node> = references(m);
            let polls = polls(run.run_dir)?;
            let steps = run::steps(run.run_dir)?;
            let paused = step(&steps, "pause").and_then(|s| s.start);
            // The final pause's window is not followed by any poll.
            let closing = |w: &Window| w.fault == "pause" || paused.is_some_and(|p| w.start >= p);
            let mut api_seen: BTreeSet<&str> = BTreeSet::new();
            for reader in readers {
                let log = read_logs(&run.run_dir.join("logs").join(&reader.name))?;
                for relay in &relays {
                    let label = format!("{} accepts {}", reader.name, relay.name);
                    let from = match reach(&m.run, &steps, &reader.name, &relay.name) {
                        Reach::Never => continue,
                        Reach::Always => i64::MIN,
                        Reach::From(Some(t)) => t,
                        Reach::From(None) => {
                            cases.fail(
                                SECTION,
                                label,
                                "the run records no open-routes-node4 step: when the routes to it opened is not known",
                            );
                            continue;
                        }
                    };
                    let listing = from.saturating_add(LISTING_MS);
                    let mine: Vec<&Poll> = polls
                        .iter()
                        .filter(|p| p.node == reader.name && p.at >= listing)
                        .collect();
                    let mut problems: Vec<String> = Vec::new();
                    if mine.is_empty() {
                        problems.push(format!("no poll of {}'s peer list", reader.name));
                    }
                    let accepted: Vec<Option<i64>> = accepted_at(&log, &relay.ip)
                        .into_iter()
                        .filter(|t| t.is_none_or(|t| t >= from))
                        .collect();
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
                            if !e.api.is_empty() {
                                api_seen.insert(relay.name.as_str());
                            }
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
                        concerns(w, &reader.name, &relay.name) && !closing(w) && w.end >= from
                    }) {
                        let after = w.end.saturating_add(DISCOVERY_MS);
                        let followed = mine.iter().any(|p| p.at > after && p.lists(&relay.ip));
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
            for relay in &relays {
                if !api_seen.contains(relay.name.as_str()) {
                    cases.fail(
                        SECTION,
                        format!("{}'s API port in the peer lists", relay.name),
                        format!(
                            "no poll that lists {} records its ports: its API port {API_PORT} is not shown",
                            relay.name
                        ),
                    );
                }
            }
            Ok(())
        },
    )
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn opened(at: i64) -> Step {
        Step {
            name: "open-routes-node4".into(),
            node: "node4".into(),
            ok: true,
            start: Some(at),
            record: json!({"step": "open-routes-node4"}),
        }
    }

    #[test]
    fn the_route_table_decides_who_can_reach_a_relay() {
        for run in ["w0", "w", "p"] {
            assert_eq!(reach(run, &[], "node5", "node4"), Reach::Always);
            assert_eq!(reach(run, &[], "node5", "node6"), Reach::Never);
            assert_eq!(reach(run, &[], "node1", "node6"), Reach::Always);
        }
        let steps = [opened(5_000)];
        assert_eq!(reach("s", &steps, "node4", "node1"), Reach::Always);
        assert_eq!(
            reach("s", &steps, "node4", "node2"),
            Reach::From(Some(5_000))
        );
        assert_eq!(reach("s", &[], "node4", "node3"), Reach::From(None));
    }

    #[test]
    fn a_window_is_extended_by_one_discovery_interval() {
        let w = Window {
            fault: "sigint".into(),
            nodes: vec!["node4".into()],
            start: 1_000,
            end: 2_000,
        };
        let all = [w];
        assert!(in_window(&all, "node1", "node4", 2_000 + DISCOVERY_MS));
        assert!(!in_window(&all, "node1", "node4", 2_001 + DISCOVERY_MS));
        assert!(!in_window(&all, "node1", "node6", 1_500));
    }
}
