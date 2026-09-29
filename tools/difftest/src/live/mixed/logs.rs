//! The logs of a mixed run counted against the baseline run's.
//!
//! The reference logs are read for the lines that name a Heartwood relay (item by node named); the
//! Heartwood logs for the lines that name a reference peer (item by the node that logged them).
//! A line is counted in the class of the fault window it falls in ([`Window::class`]); a line
//! outside every window is counted as such.

use std::collections::BTreeMap;

use super::{SECTION, baseline, guard, heartwood, ip_of, references};
use crate::errors::Result;
use crate::live::bans::{self, OUTSIDE, Ruling, Tally};
use crate::live::nodes::{Event, event_of, read_logs};
use crate::live::run::{Window, window_at};
use crate::live::{Cases, MixedRun};

/// The class of the window a line at `at` falls in for any of `nodes`, else [`OUTSIDE`].
fn class_of(windows: &[Window], nodes: &[&str], at: Option<i64>) -> String {
    let Some(t) = at else {
        return OUTSIDE.to_owned();
    };
    nodes
        .iter()
        .filter_map(|n| window_at(windows, n, t))
        .max_by_key(|w| w.start)
        .map_or_else(|| OUTSIDE.to_owned(), |w| w.class().to_owned())
}

/// Lines that fail wherever they are: what was logged, how often, and by whom.
type Forbidden = BTreeMap<(String, String), usize>;

/// What the reference nodes `readers` logged of the nodes `named` (name and address): the
/// transport lines counted per named node and class, and the lines that fail anywhere.
fn named_by(
    dir: &std::path::Path,
    readers: &[String],
    named: &[(String, String)],
    windows: &[Window],
) -> Result<(Tally, Forbidden)> {
    let mut tally = Tally::default();
    let mut forbidden = Forbidden::new();
    for reader in readers {
        for line in read_logs(&dir.join("logs").join(reader))? {
            let Some(event) = event_of(&line.text) else {
                continue;
            };
            let Some((name, _)) = named.iter().find(|(n, ip)| n != reader && ip == event.ip())
            else {
                continue;
            };
            let class = class_of(windows, &[name], line.at);
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
/// passing case, a count left out is recorded.
fn rule(cases: &mut Cases, what: &str, tally: &Tally, base: &Tally, windows: &[Window]) {
    let found = tally.compare(base, windows, "node4");
    let mut within = 0usize;
    for f in &found {
        match f.ruling {
            Ruling::Over(_) => cases.fail(
                SECTION,
                format!("{what}: {} in {}", f.kind, f.class),
                f.describe(),
            ),
            Ruling::Within(_) => within += 1,
            Ruling::Left(_) => cases.record(SECTION, format!("{what}: {}", f.describe())),
        }
    }
    if within > 0 {
        cases.pass(
            SECTION,
            format!("{what}: {within} counts per node and class, none above the baseline's"),
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
        let (tally, forbidden) = named_by(run.run_dir, &readers, &named, run.windows)?;
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
        let (base_tally, _) = named_by(&base.dir, &readers, &base_named, &base.windows)?;
        rule(cases, "reference logs", &tally, &base_tally, &base.windows);
        Ok(())
    })
}

/// What the Heartwood node `reader` logged of the reference nodes: the socket-close bans counted
/// per reason and class, and the lines that fail anywhere.
fn heartwood_by(
    log: &std::path::Path,
    reader: &str,
    peers: &[(String, String)],
    devnet: &[(String, String)],
    windows: &[Window],
) -> Result<(Tally, Forbidden)> {
    let mut tally = Tally::default();
    let mut forbidden = Forbidden::new();
    for line in read_logs(log)? {
        if let Some(Event::Ban { ip, reason }) = event_of(&line.text) {
            if let Some((peer, _)) = peers.iter().find(|(_, p)| *p == ip) {
                if bans::is_socket_close(&reason) {
                    let class = class_of(windows, &[reader, peer], line.at);
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
        let mut total = Tally::default();
        let mut base_total = Tally::default();
        for relay in relays {
            let log = run.run_dir.join("nodes").join(&relay.name).join("log");
            let (tally, forbidden) = heartwood_by(&log, &relay.name, &peers, &devnet, run.windows)?;
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
            merge(&mut total, &tally);
            // The baseline's node of the same name, and the same peers.
            let base_log = base.dir.join("logs").join(&relay.name);
            let base_peers: Vec<(String, String)> = peers
                .iter()
                .filter_map(|(n, _)| ip_of(&base.manifest, n).map(|ip| (n.clone(), ip.to_owned())))
                .collect();
            let (t, _) = heartwood_by(&base_log, &relay.name, &base_peers, &[], &base.windows)?;
            merge(&mut base_total, &t);
        }
        rule(cases, "Heartwood logs", &total, &base_total, &base.windows);
        Ok(())
    })
}

fn merge(into: &mut Tally, from: &Tally) {
    for (n, c, k, v) in from.iter() {
        for _ in 0..v {
            into.add(n, c, k);
        }
    }
}
