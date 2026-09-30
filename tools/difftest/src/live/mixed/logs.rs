//! The logs of a mixed run counted against the baseline run's.
//!
//! The reference logs are read for the lines that name a Heartwood relay (item by node named); the
//! Heartwood logs for the lines that name a reference peer (item by the node that logged them).
//! A line is counted in the class of the fault window it falls in ([`Window::class`]), a window of
//! either the node the line names or the node that logged it. A line outside every window between
//! two nodes the run's route table kept apart (a node that reaches one relay only, or reaches
//! nothing: `routes.jsonl`) is counted as [`run::UNREACHABLE`]: its discovery's failures are the
//! reference's own, and it is compared with the baseline's count in that class when the baseline
//! kept a reader of the same run apart from the node too, and left out and recorded otherwise. Any
//! other line outside every window is counted as such, and fails.
//!
//! A class the baseline's node never had is compared with the baseline's node 4 restart when it is
//! a restart ([`FALLBACK`]); when the run has no Heartwood relay in node 4's place (run `s`), that
//! allowance is the most any one source (a reader, a peer) gave of node 4's restart in the
//! baseline, once for each source of this run.

use std::collections::BTreeMap;

use super::{SECTION, baseline, guard, heartwood, ip_of, references};
use crate::errors::Result;
use crate::live::bans::{self, OUTSIDE, Ruling, Tally};
use crate::live::nodes::{Event, event_of, read_logs};
use crate::live::run::{self, Reach, Window, window_at};
use crate::live::{Cases, MixedRun};

/// The node whose restart in the baseline is the allowance of a restart the baseline's node in the
/// same place never had.
const FALLBACK: &str = "node4";

/// The class of a line at `at` that `reader` logged of `named`: the class of the window it falls in
/// for either node, else [`run::UNREACHABLE`] when the route table kept the two apart, else
/// [`OUTSIDE`].
fn class_of(
    windows: &[Window],
    reaches: &[Reach],
    reader: &str,
    named: &str,
    at: Option<i64>,
) -> String {
    let Some(t) = at else {
        return OUTSIDE.to_owned();
    };
    [named, reader]
        .iter()
        .filter_map(|n| window_at(windows, n, t))
        .max_by_key(|w| w.start)
        .map(|w| w.class().to_owned())
        .or_else(|| run::kept_apart(reaches, reader, named, t).then(|| run::UNREACHABLE.to_owned()))
        .unwrap_or_else(|| OUTSIDE.to_owned())
}

/// A run's directory, fault windows and route table.
struct Logged<'a> {
    dir: &'a std::path::Path,
    windows: &'a [Window],
    reaches: Vec<Reach>,
}

/// Lines that fail wherever they are: what was logged, how often, and by whom.
type Forbidden = BTreeMap<(String, String), usize>;

/// What the reference nodes `readers` logged of the nodes `named` (name and address) in the run
/// `logged`: the transport lines counted per named node and class, and the lines that fail
/// anywhere.
fn named_by(
    logged: &Logged<'_>,
    readers: &[String],
    named: &[(String, String)],
) -> Result<(Tally, Forbidden)> {
    let mut tally = Tally::default();
    let mut forbidden = Forbidden::new();
    for reader in readers {
        for line in read_logs(&logged.dir.join("logs").join(reader))? {
            let Some(event) = event_of(&line.text) else {
                continue;
            };
            let Some((name, _)) = named.iter().find(|(n, ip)| n != reader && ip == event.ip())
            else {
                continue;
            };
            let class = class_of(logged.windows, &logged.reaches, reader, name, line.at);
            match &event {
                Event::Ban { reason, .. } if bans::is_socket_close(reason) => {
                    tally.add(name, &class, "transport");
                }
                Event::Ban { reason, .. } => {
                    *forbidden
                        .entry((reader.clone(), format!("a ban of {name}: {reason}")))
                        .or_default() += 1;
                }
                Event::UnexpectedReply(_) => {
                    *forbidden
                        .entry((
                            reader.clone(),
                            format!("\"Got unexpected reply\" from {name}"),
                        ))
                        .or_default() += 1;
                }
                Event::ValidationError(_) => {
                    *forbidden
                        .entry((
                            reader.clone(),
                            format!("\"Socket data validation error\" of {name}"),
                        ))
                        .or_default() += 1;
                }
                Event::ResponseError(_) | Event::SocketError(_) => {
                    tally.add(name, &class, "transport");
                }
                Event::Accepted(_) => {}
            }
        }
    }
    Ok((tally, forbidden))
}

/// The findings of a comparison as cases: an excess fails, a count within the baseline's is one
/// passing case, a count left out is recorded. `windows` are the baseline's, with its route
/// table's spans between the same readers and nodes as windows of their own class.
fn rule(cases: &mut Cases, what: &str, tally: &Tally, base: &Tally, windows: &[Window]) {
    let found = tally.compare(base, windows, FALLBACK);
    let mut within = Vec::new();
    for f in &found {
        match f.ruling {
            Ruling::Over(_) => cases.fail(
                SECTION,
                format!("{what}: {} in {}", f.kind, f.class),
                f.describe(),
            ),
            Ruling::Within(n) => within.push(format!("{} {} {} ({n})", f.node, f.class, f.count)),
            Ruling::Left(_) => cases.record(SECTION, format!("{what}: {}", f.describe())),
        }
    }
    if !within.is_empty() {
        cases.pass(
            SECTION,
            format!(
                "{what}: {} counts per node and class, none above the baseline's: {}",
                within.len(),
                within.join(", ")
            ),
        );
    }
}

/// No ban, reply refusal or validation error naming a Heartwood address in the reference logs,
/// and their transport lines per fault window's class no more than the baseline's.
pub(in crate::live) fn reference_logs(run: &MixedRun<'_>, cases: &mut Cases) -> Result<()> {
    let label = "the reference logs";
    guard(cases, SECTION, label, |cases| {
        let m = run.manifest;
        let named: Vec<(String, String)> = heartwood(m)
            .iter()
            .map(|n| (n.name.clone(), n.ip.clone()))
            .collect();
        if named.is_empty() {
            cases.record(
                SECTION,
                "no Heartwood relay in this run: its reference logs are the baseline's",
            );
            return Ok(());
        }
        let base = baseline(run)?
            .ok_or_else(|| crate::errors::Error::Usage("no baseline run given".into()))?;
        let readers: Vec<String> = references(m).iter().map(|n| n.name.clone()).collect();
        let here = Logged {
            dir: run.run_dir,
            windows: run.windows,
            reaches: run::reaches(run.run_dir, m.window_seconds)?,
        };
        let there = Logged {
            dir: &base.dir,
            windows: &base.windows,
            reaches: run::reaches(&base.dir, base.manifest.window_seconds)?,
        };
        let (tally, forbidden) = named_by(&here, &readers, &named)?;
        for reader in &readers {
            let mine: Vec<_> = forbidden.iter().filter(|((r, _), _)| r == reader).collect();
            if mine.is_empty() {
                cases.pass(
                    SECTION,
                    format!("{reader}: no ban, unexpected reply or validation error naming a Heartwood address"),
                );
            }
            for ((_, what), n) in mine {
                cases.fail(
                    SECTION,
                    format!("{reader}: the reference logs"),
                    format!("{n} lines: {what}"),
                );
            }
        }
        // The baseline's own lines about the same nodes, by the same readers.
        let base_named: Vec<(String, String)> = named
            .iter()
            .filter_map(|(n, _)| ip_of(&base.manifest, n).map(|ip| (n.clone(), ip.to_owned())))
            .collect();
        let (mut base_tally, _) = named_by(&there, &readers, &base_named)?;
        if !named.iter().any(|(n, _)| n == FALLBACK)
            && let Some(ip) = ip_of(&base.manifest, FALLBACK)
        {
            // The baseline's node 4 as each of its readers logged it.
            let fallback = [(FALLBACK.to_owned(), ip.to_owned())];
            let mut each = Vec::new();
            for reader in references(&base.manifest)
                .iter()
                .filter(|n| n.name != FALLBACK)
            {
                let one = [reader.name.clone()];
                each.push(named_by(&there, &one, &fallback)?.0);
            }
            base_tally.merge(&Tally::most(&each, readers.len()));
        }
        let names: Vec<String> = named.iter().map(|(n, _)| n.clone()).collect();
        let mut ruled = base.windows.clone();
        ruled.extend(run::reach_windows(&there.reaches, &readers, &names));
        rule(cases, "reference logs", &tally, &base_tally, &ruled);
        Ok(())
    })
}

/// What the Heartwood node `reader` logged in `log` of the reference nodes, in the run `logged`: the
/// socket-close bans counted per reason and class, and the lines that fail anywhere.
fn heartwood_by(
    logged: &Logged<'_>,
    log: &std::path::Path,
    reader: &str,
    peers: &[(String, String)],
    devnet: &[(String, String)],
) -> Result<(Tally, Forbidden)> {
    let mut tally = Tally::default();
    let mut forbidden = Forbidden::new();
    for line in read_logs(log)? {
        if let Some(Event::Ban { ip, reason }) = event_of(&line.text) {
            if let Some((peer, _)) = peers.iter().find(|(_, p)| *p == ip) {
                if bans::is_socket_close(&reason) {
                    let class = class_of(logged.windows, &logged.reaches, reader, peer, line.at);
                    tally.add(reader, &class, &reason);
                } else {
                    *forbidden
                        .entry((
                            reader.to_owned(),
                            format!("a ban of {peer} for \"{reason}\""),
                        ))
                        .or_default() += 1;
                }
            }
            continue;
        }
        for marker in [
            "Protocol violation by peer ",
            "Response validation failed from peer ",
        ] {
            let Some(rest) = line.text.split(marker).nth(1) else {
                continue;
            };
            let ip = rest.split([' ', ':']).next().unwrap_or("");
            if let Some((other, _)) = devnet.iter().find(|(n, p)| p == ip && n != reader) {
                *forbidden
                    .entry((
                        reader.to_owned(),
                        format!("\"{}\" naming {other}", marker.trim_end()),
                    ))
                    .or_default() += 1;
            }
        }
    }
    Ok((tally, forbidden))
}

/// No request-refusal ban of a reference address in the Heartwood logs, the socket-close bans per
/// class no more than the baseline's, and no violation recorded against a devnet address.
pub(in crate::live) fn heartwood_logs(run: &MixedRun<'_>, cases: &mut Cases) -> Result<()> {
    guard(cases, SECTION, "the Heartwood logs", |cases| {
        let m = run.manifest;
        let relays = heartwood(m);
        if relays.is_empty() {
            cases.record(
                SECTION,
                "no Heartwood relay in this run: its logs are the baseline's",
            );
            return Ok(());
        }
        let base = baseline(run)?
            .ok_or_else(|| crate::errors::Error::Usage("no baseline run given".into()))?;
        let peers: Vec<(String, String)> = references(m)
            .iter()
            .map(|n| (n.name.clone(), n.ip.clone()))
            .collect();
        let devnet: Vec<(String, String)> = m
            .nodes
            .iter()
            .map(|n| (n.name.clone(), n.ip.clone()))
            .collect();
        let here = Logged {
            dir: run.run_dir,
            windows: run.windows,
            reaches: run::reaches(run.run_dir, m.window_seconds)?,
        };
        let there = Logged {
            dir: &base.dir,
            windows: &base.windows,
            reaches: run::reaches(&base.dir, base.manifest.window_seconds)?,
        };
        let mut total = Tally::default();
        let mut base_total = Tally::default();
        for relay in &relays {
            let log = run.run_dir.join("nodes").join(&relay.name).join("log");
            let (tally, forbidden) = heartwood_by(&here, &log, &relay.name, &peers, &devnet)?;
            if forbidden.is_empty() {
                cases.pass(
                    SECTION,
                    format!("{}: no request-refusal ban of a reference address and no violation naming a devnet address", relay.name),
                );
            }
            for ((_, what), n) in &forbidden {
                cases.fail(
                    SECTION,
                    format!("{}: the Heartwood logs", relay.name),
                    format!("{n} lines: {what}"),
                );
            }
            total.merge(&tally);
            // The baseline's node of the same name, and the same peers.
            let base_log = base.dir.join("logs").join(&relay.name);
            let base_peers: Vec<(String, String)> = peers
                .iter()
                .filter_map(|(n, _)| ip_of(&base.manifest, n).map(|ip| (n.clone(), ip.to_owned())))
                .collect();
            let (t, _) = heartwood_by(&there, &base_log, &relay.name, &base_peers, &[])?;
            base_total.merge(&t);
        }
        if !heartwood(m).iter().any(|n| n.name == FALLBACK) {
            // The baseline's node 4's bans, peer by peer.
            let log = base.dir.join("logs").join(FALLBACK);
            let mut each = Vec::new();
            for peer in base.manifest.nodes.iter().filter(|n| n.name != FALLBACK) {
                let one = [(peer.name.clone(), peer.ip.clone())];
                each.push(heartwood_by(&there, &log, FALLBACK, &one, &[])?.0);
            }
            base_total.merge(&Tally::most(&each, peers.len()));
        }
        // The counts are keyed by the Heartwood relay that logged them: the route table's spans
        // between a peer and a relay of this run, as windows of the relays.
        let peer_names: Vec<String> = peers.iter().map(|(n, _)| n.clone()).collect();
        let relay_names: Vec<String> = relays.iter().map(|n| n.name.clone()).collect();
        let mut ruled = base.windows.clone();
        ruled.extend(run::reach_windows(
            &there.reaches,
            &peer_names,
            &relay_names,
        ));
        rule(cases, "Heartwood logs", &total, &base_total, &ruled);
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::live::run::Reach;

    fn reach(node: &str, allowed: &[&str], start: i64, end: i64) -> Reach {
        Reach {
            node: node.into(),
            allowed: allowed.iter().map(|n| (*n).to_owned()).collect(),
            start,
            end,
        }
    }

    #[test]
    fn a_line_between_nodes_the_routes_kept_apart_is_unreachable() {
        let reaches = [reach("node5", &["node4"], 1_000, i64::MAX)];
        let window = Window {
            fault: "isolation".into(),
            nodes: vec!["node6".into()],
            start: 5_000,
            end: 9_000,
        };
        let windows = [window];
        // node 5 reaches node 4 only: its lines about node 6 show the route
        assert_eq!(
            class_of(&windows, &reaches, "node5", "node6", Some(2_000)),
            run::UNREACHABLE
        );
        assert_eq!(
            class_of(&windows, &reaches, "node5", "node4", Some(2_000)),
            OUTSIDE
        );
        assert_eq!(
            class_of(&windows, &reaches, "node1", "node6", Some(2_000)),
            OUTSIDE
        );
        // a fault window of either node decides first
        assert_eq!(
            class_of(&windows, &reaches, "node5", "node6", Some(6_000)),
            "isolation"
        );
        assert_eq!(
            class_of(&windows, &reaches, "node5", "node6", None),
            OUTSIDE
        );
    }

    #[test]
    fn unreachable_lines_compare_with_the_baselines_readers_and_nodes() {
        let base_reaches = [reach("node5", &["node4"], 1_000, i64::MAX)];
        let names = |n: &[&str]| n.iter().map(|x| (*x).to_owned()).collect::<Vec<_>>();
        // Run w: node 5 reaches node 4 only in both runs; its lines about node 6 are compared.
        let mut tally = Tally::default();
        let mut base = Tally::default();
        for _ in 0..33 {
            tally.add("node6", run::UNREACHABLE, "transport");
        }
        for _ in 0..37 {
            base.add("node6", run::UNREACHABLE, "transport");
        }
        let ruled = run::reach_windows(
            &base_reaches,
            &names(&["node1", "node2", "node3", "node5"]),
            &names(&["node4", "node6"]),
        );
        let found = tally.compare(&base, &ruled, FALLBACK);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].ruling, Ruling::Within(37));
        tally.add("node6", run::UNREACHABLE, "transport");
        tally.add("node6", run::UNREACHABLE, "transport");
        tally.add("node6", run::UNREACHABLE, "transport");
        tally.add("node6", run::UNREACHABLE, "transport");
        tally.add("node6", run::UNREACHABLE, "transport");
        assert_eq!(
            tally.compare(&base, &ruled, FALLBACK)[0].ruling,
            Ruling::Over(37)
        );
        // Run s: node 4 reaches node 1 only for a while; the baseline's node 4 never did, so its
        // lines about node 2 are left out and recorded.
        let mut s = Tally::default();
        s.add("node2", run::UNREACHABLE, "transport");
        let ruled = run::reach_windows(
            &base_reaches,
            &names(&["node4"]),
            &names(&["node1", "node2", "node3"]),
        );
        assert!(matches!(
            s.compare(&Tally::default(), &ruled, FALLBACK)[0].ruling,
            Ruling::Left(_)
        ));
    }
}
