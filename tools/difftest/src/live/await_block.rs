//! The AwaitBlock report of a live run (`difftest live await RUN_DIR`): per Heartwood relay and
//! route, the requests, the share that waited, the median, 99th percentile and longest wait and
//! how the waits ended, beside the machine's load; the stall's window; and the record's minimums.
//! A mixed run's live check runs the same checks.
//!
//! The record is `nodes/<node>/difftest/await-NNNN.jsonl`, one file per start of the relay, beside
//! that start's `digest-NNNN.jsonl`. A line is one public request that reached the wait:
//! `{"t", "route", "caller", "waitMs", "end", "tipBefore", "tipAfter"}` with `t` the UTC time the
//! request reached the wait (so the wait ended `waitMs` later) and `end` one of `none`, `drained`,
//! `bound` and `closed`. The checks:
//!
//! - every start of every Heartwood relay has its record file, and the lines parse (a torn last
//!   line is dropped and recorded);
//! - the minimums: per relay at least 20 lines per round of the run for each of `getStatus`,
//!   `getBlocks` and `getUnconfirmedTransactions`, and in the run at least one `drained` end with a
//!   wait outside the stall;
//! - honest waits: no internal route in any record; outside a stall's window no wait ends `bound`
//!   and none ends `closed` outside a fault window of the relay; every wait that ended `drained`
//!   ended on a tip the relay's digest shows;
//! - the stall (a fault `stall` in `faults.jsonl`, its `recovered` being the job's commit): every
//!   wait that began more than 8 s before the commit ended `bound` between 8.0 and 8.3 s on the tip
//!   before the job, or `closed`; at least one ended `bound`; at least one ended `closed` unless the
//!   reference nodes' logs show no client reached three timeouts on the relay in the window; waits
//!   that began later did not end `bound`, and none ended `drained` on the tip before the job;
//!   requests after the commit carried a later tip and the chain went on; whether the node whose
//!   only peer the relay is kept or regained it is not checked yet (a case that fails a strict run);
//! - recorded, not gated: per relay and route the distribution of the waits, the longest honest
//!   wait beside the machine's load (`load.jsonl`, `{ts, load1}` lines, when the recipe wrote it),
//!   and what the stall did per route.
//!
//! The wait sees the queue through a watch that is refreshed every 20 ms, so a drain is seen up to
//! 20 ms late and a bound end lands between 8.0 and 8.3 s.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use heartwood_p2p_wire::RouteId;
use serde_json::Value;

use super::run::{self, Implementation, Manifest, Node, Window};
use super::{Cases, MixedRun, digest, nodes};
use crate::errors::Result;
use crate::report::Report;

/// The section of the report's cases.
const SECTION: &str = "await";
/// The bound of a wait, in milliseconds.
const BOUND_MS: u64 = 8_000;
/// How far past the bound a bound end may land: the queue is polled every 20 ms, and the timer and
/// the scheduler add a little.
const BOUND_LATE_MS: u64 = 300;
/// How much later than the job's commit the recipe may have noted it (the moment it saw the relay
/// answer again), in milliseconds.
const COMMIT_LAG_MS: i64 = 1_000;
/// The length of the stall the recipe asks for, used when the relay's log does not say when the
/// job was held.
const STALL_MS: i64 = 12_000;
/// Lines per round of the run, per route that must be exercised.
const PER_ROUND: usize = 20;
/// The routes the record must show, per round of the run.
const EXERCISED: [&str; 3] = ["getStatus", "getBlocks", "getUnconfirmedTransactions"];
/// A longest honest wait at or above this is investigated, in milliseconds.
const INVESTIGATE_MS: u64 = 800;
/// Timeouts in a row after which a reference client closes its socket to a peer.
const ERRORS_TO_CLOSE: usize = 3;
/// A load sample further than this from a wait does not stand beside it, in milliseconds.
const LOAD_REACH_MS: i64 = 60_000;
/// At most this many problems of one kind are listed one by one.
const LISTED: usize = 10;

/// How a wait ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum End {
    /// There was nothing to wait for.
    None,
    /// The queue drained.
    Drained,
    /// The bound passed.
    Bound,
    /// The client's socket closed during the wait.
    Closed,
}

impl End {
    fn parse(text: &str) -> Option<End> {
        match text {
            "none" => Some(End::None),
            "drained" => Some(End::Drained),
            "bound" => Some(End::Bound),
            "closed" => Some(End::Closed),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            End::None => "none",
            End::Drained => "drained",
            End::Bound => "bound",
            End::Closed => "closed",
        }
    }
}

/// One line of a record.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Request {
    /// The UTC time the request reached the wait, in milliseconds.
    at: i64,
    /// The route's name.
    route: String,
    /// The wait, in milliseconds.
    wait: u64,
    /// How it ended.
    end: End,
    /// The tip when the request arrived.
    before: u64,
    /// The tip when the wait ended.
    after: u64,
}

impl Request {
    /// When the wait began: when the request reached it.
    fn began(&self) -> i64 {
        self.at
    }

    /// When the wait ended.
    fn ended(&self) -> i64 {
        self.at
            .saturating_add(i64::try_from(self.wait).unwrap_or(i64::MAX))
    }

    fn parse(value: &Value) -> std::result::Result<Request, String> {
        let text = |key: &str| {
            value
                .get(key)
                .and_then(Value::as_str)
                .ok_or_else(|| format!("no {key}"))
        };
        let number = |key: &str| {
            value
                .get(key)
                .and_then(Value::as_u64)
                .ok_or_else(|| format!("no {key}"))
        };
        text("caller")?;
        let t = text("t")?;
        let end = text("end")?;
        Ok(Request {
            at: run::parse_time(t).ok_or_else(|| format!("{t:?} is not a UTC time"))?,
            route: text("route")?.to_owned(),
            wait: number("waitMs")?,
            end: End::parse(end).ok_or_else(|| format!("{end:?} is not an end"))?,
            before: number("tipBefore")?,
            after: number("tipAfter")?,
        })
    }
}

/// A Heartwood relay's record, read.
#[derive(Debug, Default)]
struct Relay {
    name: String,
    ip: String,
    /// Every whole line of every record file, in order.
    requests: Vec<Request>,
    /// The record files read.
    files: usize,
    /// The starts the digest files list.
    starts: Vec<u32>,
    /// The starts without a record file.
    missing: Vec<u32>,
    /// A torn last line: the file and its length.
    torn: Vec<(String, usize)>,
    /// What could not be read or parsed.
    problems: Vec<String>,
    /// The heights the relay's digest lines name.
    heights: BTreeSet<u64>,
}

/// The number of the start in a file name `<prefix>NNNN.jsonl`.
fn numbered(name: &str, prefix: &str) -> Option<u32> {
    name.strip_prefix(prefix)?
        .strip_suffix(".jsonl")?
        .parse()
        .ok()
}

fn load_relay(run_dir: &Path, node: &Node) -> Relay {
    let mut relay = Relay {
        name: node.name.clone(),
        ip: node.ip.clone(),
        ..Relay::default()
    };
    let dir = run_dir.join("nodes").join(&node.name).join("difftest");
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(e) => {
            relay.problems.push(format!("{}: {e}", dir.display()));
            return relay;
        }
    };
    let mut awaits = BTreeMap::new();
    for path in entries.filter_map(|e| e.ok().map(|e| e.path())) {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if let Some(start) = numbered(&name, "digest-") {
            relay.starts.push(start);
        } else if let Some(start) = numbered(&name, "await-") {
            awaits.insert(start, path);
        }
    }
    relay.starts.sort_unstable();
    relay.missing = (relay.starts.iter())
        .filter(|s| !awaits.contains_key(s))
        .copied()
        .collect();
    for (start, path) in &awaits {
        read_record(path, *start, &mut relay);
    }
    match digest::read(&dir) {
        Ok(read) => relay
            .heights
            .extend(read.lines.iter().map(|l| u64::from(l.height()))),
        Err(e) => relay.problems.push(e.to_string()),
    }
    relay
}

fn read_record(path: &Path, start: u32, relay: &mut Relay) {
    let name = format!("await-{start:04}.jsonl");
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) => {
            relay.problems.push(format!("{name}: {e}"));
            return;
        }
    };
    relay.files += 1;
    let text = String::from_utf8_lossy(&bytes);
    let whole = text.ends_with('\n');
    let parts: Vec<&str> = text.split_terminator('\n').collect();
    for (index, line) in parts.iter().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let last = index + 1 == parts.len();
        match serde_json::from_str::<Value>(line) {
            Ok(value) => match Request::parse(&value) {
                Ok(request) => relay.requests.push(request),
                Err(why) => relay.problems.push(format!("{name}:{}: {why}", index + 1)),
            },
            Err(_) if last && !whole => relay.torn.push((name.clone(), line.len())),
            Err(e) => relay.problems.push(format!("{name}:{}: {e}", index + 1)),
        }
    }
}

/// The load samples of `load.jsonl`, in order of time.
struct Load(Vec<(i64, f64)>);

impl Load {
    fn read(run_dir: &Path) -> Load {
        let lines = run::read_lines(&run_dir.join("load.jsonl")).unwrap_or_default();
        let mut samples: Vec<(i64, f64)> = lines
            .iter()
            .filter_map(|v| {
                let at = ["ts", "t"]
                    .iter()
                    .find_map(|k| v.get(k).and_then(Value::as_str))
                    .and_then(run::parse_time)?;
                Some((at, v.get("load1")?.as_f64()?))
            })
            .collect();
        samples.sort_by_key(|s| s.0);
        Load(samples)
    }

    /// The load beside the time `t`, as text.
    fn beside(&self, t: i64) -> String {
        let near = (self.0.iter())
            .min_by_key(|(at, _)| at.abs_diff(t))
            .filter(|(at, _)| at.abs_diff(t) <= LOAD_REACH_MS.unsigned_abs());
        near.map_or_else(
            || "no load sample".to_owned(),
            |(_, load)| format!("load {load:.2}"),
        )
    }
}

/// The time of a log line `[YYYY-MM-DD HH:MM:SS.fff] ...`, read as UTC.
fn line_time(line: &str) -> Option<i64> {
    let inner = line.strip_prefix('[')?.split(']').next()?;
    run::parse_time(&format!("{inner}Z"))
}

/// The `p`th percentile (nearest rank) of the sorted `values`.
fn percentile(values: &[u64], p: usize) -> u64 {
    let rank = (p * values.len()).div_ceil(100).max(1);
    values.get(rank - 1).copied().unwrap_or(0)
}

fn listed(cases: &mut Cases, label: &str, problems: &[String]) {
    for p in problems.iter().take(LISTED) {
        cases.fail(SECTION, label, p.clone());
    }
    if problems.len() > LISTED {
        cases.fail(
            SECTION,
            label,
            format!("... and {} more", problems.len() - LISTED),
        );
    }
}

/// Whether `route` is a public route's name.
fn public_route(route: &str) -> bool {
    RouteId::ALL
        .into_iter()
        .any(|r| r.name() == route && !r.is_internal())
}

/// Whether `route` is an internal route's name.
fn internal_route(route: &str) -> bool {
    RouteId::ALL
        .into_iter()
        .any(|r| r.name() == route && r.is_internal())
}

/// What a client of the route does when the wait passes its timeout: only the 10 s routes and a
/// download can still take a late answer.
fn client_class(route: &str) -> &'static str {
    match route {
        "postBlock" | "postTransactions" | "getUnconfirmedTransactions" => {
            "answer awaited (10 s timeout)"
        }
        "getBlocks" => "answer awaited by a download (30 s), gone for an idle poll (2 s)",
        _ => "client gone (timeout below 8 s), late answer dropped",
    }
}

/// The verdict on one stall.
#[derive(Debug, Default)]
struct StallVerdict {
    problems: Vec<String>,
    bound: usize,
    closed: usize,
    /// The tip before the job.
    pre_tip: Option<u64>,
}

/// Judge the requests of a relay against its stall `window`, the job having been held from
/// `hold` (or `STALL_MS` before its commit); `closed_needed` when the record must show a socket
/// closed by a client.
fn judge_stall(
    requests: &[Request],
    window: &Window,
    window_seconds: i64,
    hold: Option<i64>,
    closed_needed: bool,
) -> StallVerdict {
    let mut v = StallVerdict::default();
    if window.end == i64::MAX {
        v.problems
            .push("the stall has no recovery time, so no commit".into());
        return v;
    }
    let commit = window.end - window_seconds * 1_000;
    let hold_at = hold.unwrap_or(commit - STALL_MS);
    let bound_ms = i64::try_from(BOUND_MS).unwrap_or(i64::MAX);
    let held: Vec<&Request> = requests
        .iter()
        .filter(|r| r.ended() >= hold_at && r.began() <= window.end)
        .collect();
    let bounds: Vec<&&Request> = held.iter().filter(|r| r.end == End::Bound).collect();
    v.bound = bounds.len();
    v.closed = held.iter().filter(|r| r.end == End::Closed).count();
    v.pre_tip = bounds.first().map(|r| r.before);
    if bounds.is_empty() {
        v.problems.push("no wait ended `bound` in the stall".into());
    }
    for r in &bounds {
        if !(BOUND_MS..=BOUND_MS + BOUND_LATE_MS).contains(&r.wait) {
            v.problems.push(format!(
                "a {} wait ended `bound` after {} ms, not between 8.0 and 8.3 s (ended {})",
                r.route,
                r.wait,
                run::format_time(r.ended())
            ));
        }
        if let Some(pre) = v.pre_tip
            && (r.before != pre || r.after != pre)
        {
            v.problems.push(format!(
                "a {} wait ended `bound` on tips {} to {}, not on the tip before the job, {pre} (ended {})",
                r.route,
                r.before,
                r.after,
                run::format_time(r.ended())
            ));
        }
    }
    for r in held.iter().filter(|r| r.end != End::None) {
        let ahead = commit.saturating_sub(r.began());
        if ahead > bound_ms + COMMIT_LAG_MS && !matches!(r.end, End::Bound | End::Closed) {
            v.problems.push(format!(
                "a {} wait that began {} ms before the commit ended `{}`, not `bound` or `closed` (ended {})",
                r.route,
                ahead,
                r.end.name(),
                run::format_time(r.ended())
            ));
        }
        if ahead < bound_ms && r.end == End::Bound {
            v.problems.push(format!(
                "a {} wait that began {} ms before the commit ended `bound` (ended {})",
                r.route,
                ahead,
                run::format_time(r.ended())
            ));
        }
        if let Some(pre) = v.pre_tip
            && r.end == End::Drained
            && r.after <= pre
        {
            v.problems.push(format!(
                "a {} wait ended `drained` on the tip before the job, {pre} (ended {})",
                r.route,
                run::format_time(r.ended())
            ));
        }
    }
    if closed_needed && v.closed == 0 {
        v.problems
            .push("no wait ended `closed`, and the logs do not show that no client reached three timeouts".into());
    }
    if let Some(pre) = v.pre_tip {
        let after: Vec<&Request> = requests
            .iter()
            .filter(|r| r.began() >= commit && r.began() <= window.end)
            .collect();
        if after.is_empty() {
            v.problems.push("no request after the commit".into());
        }
        for r in after.iter().filter(|r| r.before <= pre) {
            v.problems.push(format!(
                "a {} request after the commit carried the tip before the job, {pre} (at {})",
                r.route,
                run::format_time(r.ended())
            ));
        }
        let resumed = requests
            .iter()
            .filter(|r| r.began() >= commit)
            .any(|r| r.after > pre.saturating_add(1));
        if !resumed {
            v.problems.push(format!(
                "no request after the commit saw the chain go past {}",
                pre.saturating_add(1)
            ));
        }
    }
    v
}

/// When the relay's log says it held a job, within `from` to `to` (the latest such line).
fn hold_time(run_dir: &Path, node: &str, from: i64, to: i64) -> Option<i64> {
    let dir = run_dir.join("nodes").join(node).join("log");
    let mut paths: Vec<_> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| (p.file_name().and_then(|n| n.to_str())).is_some_and(nodes::is_file_log))
        .collect();
    paths.sort();
    let mut latest = None;
    for path in paths {
        let text = nodes::log_text(&path).ok()?;
        for line in text.lines().filter(|l| l.contains("holding the job for")) {
            if let Some(at) = line_time(line).filter(|t| (from..=to).contains(t)) {
                latest = latest.max(Some(at));
            }
        }
    }
    latest
}

/// The lines of each reference node's logs that record a timeout naming `ip`, between `from` and
/// `to`; the reason when the logs cannot be counted.
fn timeouts_naming(
    run_dir: &Path,
    manifest: &Manifest,
    ip: &str,
    from: i64,
    to: i64,
) -> std::result::Result<Vec<(String, usize)>, String> {
    let needle = format!("Response error (peer {ip}/");
    let mut out = Vec::new();
    for node in manifest
        .nodes
        .iter()
        .filter(|n| n.implementation == Implementation::Reference)
    {
        let dir = run_dir.join("logs").join(&node.name);
        let entries = std::fs::read_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let mut count = 0;
        for path in entries
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| (p.file_name().and_then(|n| n.to_str())).is_some_and(nodes::is_file_log))
        {
            let text = nodes::log_text(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            count += text
                .lines()
                .filter(|l| l.contains(&needle))
                .filter(|l| line_time(l).is_some_and(|t| (from..=to).contains(&t)))
                .count();
        }
        out.push((node.name.clone(), count));
    }
    Ok(out)
}

/// The run's AwaitBlock checks, in the report's order.
fn checks(run_dir: &Path, manifest: &Manifest, windows: &[Window], cases: &mut Cases) {
    let relays: Vec<Relay> = manifest
        .nodes
        .iter()
        .filter(|n| n.implementation == Implementation::Heartwood)
        .filter(|_| manifest.has_heartwood())
        .map(|n| load_relay(run_dir, n))
        .collect();
    if relays.is_empty() {
        cases.record(
            SECTION,
            "the run has no Heartwood relay, so there is no AwaitBlock record to check",
        );
        return;
    }
    let load = Load::read(run_dir);
    let rounds = manifest
        .stop_height
        .div_ceil(manifest.round_length.max(1))
        .max(1) as usize;
    let stalls_of = |node: &str| -> Vec<&Window> {
        (windows.iter())
            .filter(|w| w.fault == "stall" && w.nodes.iter().any(|n| n == node))
            .collect()
    };
    let in_stall = |node: &str, r: &Request| {
        stalls_of(node)
            .iter()
            .any(|w| w.contains(r.ended()) || w.contains(r.began()))
    };
    let mut honest_drained = 0usize;
    let mut honest_max: Option<(String, u64, String, i64)> = None;

    for relay in &relays {
        let name = &relay.name;
        // The record files.
        let label = format!("{name}: a record file for every start");
        let mut problems = relay.problems.clone();
        if relay.starts.is_empty() {
            problems.push("no digest file, so the starts cannot be listed".into());
        }
        for start in &relay.missing {
            problems.push(format!("no await-{start:04}.jsonl for start {start}"));
        }
        if problems.is_empty() {
            cases.pass(
                SECTION,
                format!(
                    "{label}: {} starts, {} lines in {} files",
                    relay.starts.len(),
                    relay.requests.len(),
                    relay.files
                ),
            );
        } else {
            listed(cases, &label, &problems);
        }
        for (file, len) in &relay.torn {
            cases.record(
                SECTION,
                format!("{name}: a torn last line of {file} dropped ({len} bytes)"),
            );
        }
        // The minimums.
        let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
        for r in &relay.requests {
            *counts.entry(r.route.as_str()).or_default() += 1;
        }
        for route in EXERCISED {
            let have = counts.get(route).copied().unwrap_or(0);
            let need = PER_ROUND * rounds;
            let label = format!("{name}: at least {need} {route} lines ({rounds} rounds)");
            if have >= need {
                cases.pass(SECTION, format!("{label}: {have}"));
            } else {
                cases.fail(SECTION, label, format!("{have} lines"));
            }
        }
        // Honest waits.
        let label = format!("{name}: only public routes in the record");
        let strange: BTreeSet<&str> = (relay.requests.iter())
            .map(|r| r.route.as_str())
            .filter(|r| !public_route(r))
            .collect();
        if strange.is_empty() {
            cases.pass(SECTION, label);
        } else {
            let names: Vec<String> = strange
                .iter()
                .map(|r| {
                    let kind = if internal_route(r) {
                        "internal"
                    } else {
                        "unknown"
                    };
                    format!("{r} ({kind})")
                })
                .collect();
            cases.fail(SECTION, label, names.join(", "));
        }
        let honest: Vec<&Request> = (relay.requests.iter())
            .filter(|r| !in_stall(name, r))
            .collect();
        let label = format!("{name}: outside the stall every wait ended by the drain");
        let mut problems = Vec::new();
        for r in &honest {
            let fault = windows
                .iter()
                .find(|w| w.concerns(name) && (w.contains(r.ended()) || w.contains(r.began())));
            match r.end {
                End::Bound => problems.push(format!(
                    "a {} wait ended `bound` after {} ms (ended {})",
                    r.route,
                    r.wait,
                    run::format_time(r.ended())
                )),
                End::Closed if fault.is_none() => problems.push(format!(
                    "a {} wait ended `closed` outside a fault window (ended {})",
                    r.route,
                    run::format_time(r.ended())
                )),
                _ => {}
            }
        }
        if problems.is_empty() {
            cases.pass(SECTION, label);
        } else {
            listed(cases, &label, &problems);
        }
        let label = format!("{name}: every wait answered on a tip the relay applied");
        let mut problems = Vec::new();
        for r in relay
            .requests
            .iter()
            .filter(|r| r.end == End::Drained && r.wait > 0)
        {
            if r.after != r.before && !relay.heights.contains(&r.after) {
                problems.push(format!(
                    "a {} wait ended on the tip {}, which the digest never applied (ended {})",
                    r.route,
                    r.after,
                    run::format_time(r.ended())
                ));
            }
        }
        if problems.is_empty() {
            cases.pass(SECTION, label);
        } else {
            listed(cases, &label, &problems);
        }
        honest_drained += honest
            .iter()
            .filter(|r| r.end == End::Drained && r.wait > 0)
            .count();
        for r in &honest {
            if honest_max.as_ref().is_none_or(|m| r.wait > m.1) && r.end != End::Bound {
                honest_max = Some((name.clone(), r.wait, r.route.clone(), r.ended()));
            }
        }
    }
    if honest_drained > 0 {
        cases.pass(
            SECTION,
            format!("{honest_drained} waits outside the stall ended `drained` after a wait"),
        );
    } else {
        cases.fail(
            SECTION,
            "at least one wait outside the stall ended `drained` after a wait",
            "none did",
        );
    }

    // The stall.
    let stalls: Vec<(&Relay, &Window)> = relays
        .iter()
        .flat_map(|r| stalls_of(&r.name).into_iter().map(move |w| (r, w)))
        .collect();
    if stalls.is_empty() && manifest.run == "w" {
        cases.fail(
            SECTION,
            "the run stalled a relay's job",
            "faults.jsonl has no stall",
        );
    }
    for (relay, window) in &stalls {
        stall(run_dir, manifest, relay, window, cases);
    }
    if stalls.is_empty() {
        cases.record(SECTION, "the run has no stall window");
    }

    // The distribution.
    for relay in &relays {
        distribution(relay, &load, cases);
    }
    let across = honest_max.map_or_else(
        || "no request".to_owned(),
        |(node, wait, route, t)| {
            format!(
                "{wait} ms ({node} {route} at {}, {}){}",
                run::format_time(t),
                load.beside(t),
                if wait >= INVESTIGATE_MS {
                    ": at or above 800 ms, investigate from the digest files' job times"
                } else {
                    ""
                }
            )
        },
    );
    cases.record(
        SECTION,
        format!("longest honest wait of the run {across}; the bound stays {BOUND_MS} ms"),
    );
    cases.record(
        SECTION,
        "drains are seen with 20 ms granularity (the queue state is refreshed every 20 ms), so a bound end lands between 8.0 and 8.3 s",
    );
}

/// The checks and the record of one stall.
fn stall(run_dir: &Path, manifest: &Manifest, relay: &Relay, window: &Window, cases: &mut Cases) {
    let name = &relay.name;
    let label = format!("{name}: the stall's waits");
    let commit = window.end.saturating_sub(manifest.window_seconds * 1_000);
    let hold = if window.end == i64::MAX {
        None
    } else {
        hold_time(run_dir, name, window.start, commit)
    };
    let counts = timeouts_naming(run_dir, manifest, &relay.ip, window.start, window.end);
    let closed_needed = match &counts {
        Ok(counts) => counts.iter().any(|(_, n)| *n >= ERRORS_TO_CLOSE),
        Err(_) => true,
    };
    let verdict = judge_stall(
        &relay.requests,
        window,
        manifest.window_seconds,
        hold,
        closed_needed,
    );
    if verdict.problems.is_empty() {
        cases.pass(
            SECTION,
            format!(
                "{label}: {} ended `bound` and {} `closed` around the tip {}",
                verdict.bound,
                verdict.closed,
                verdict.pre_tip.unwrap_or(0)
            ),
        );
    } else {
        listed(cases, &label, &verdict.problems);
    }
    let mut by_route: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
    for r in relay
        .requests
        .iter()
        .filter(|r| r.ended() >= hold.unwrap_or(commit - STALL_MS) && r.began() <= window.end)
    {
        let entry = by_route.entry(r.route.as_str()).or_default();
        match r.end {
            End::Bound => entry.0 += 1,
            End::Closed => entry.1 += 1,
            _ => {}
        }
    }
    let routes: Vec<String> = by_route
        .iter()
        .filter(|(_, (b, c))| b + c > 0)
        .map(|(route, (b, c))| format!("{route} {b} bound, {c} closed: {}", client_class(route)))
        .collect();
    cases.record(
        SECTION,
        format!(
            "{name}: the stall from {} to {} (commit {}, job held {}): {}",
            run::format_time(window.start),
            run::format_time(window.end),
            run::format_time(commit),
            hold.map_or(
                "at an unknown time, taken as 12 s before the commit".to_owned(),
                |h| format!("from {}", run::format_time(h))
            ),
            if routes.is_empty() {
                "no wait ended bound or closed".to_owned()
            } else {
                routes.join("; ")
            }
        ),
    );
    let seen = match counts {
        Ok(counts) => {
            let list: Vec<String> = counts
                .iter()
                .map(|(node, n)| format!("{node} {n}"))
                .collect();
            format!(
                "reference log lines `Response error (peer {}/` in the window: {}",
                relay.ip,
                list.join(", ")
            )
        }
        Err(why) => format!("the reference logs could not be counted ({why})"),
    };
    cases.record(SECTION, format!("{name}: {seen}"));
    // The peer lists of the reference node that reaches only this relay are not read yet, so a
    // strict run cannot pass the stall without this check.
    cases.not_built(
        SECTION,
        &format!(
            "{name}: the node whose only peer it is kept or regained it in the stall's window"
        ),
    );
}

/// The distribution of the waits of one relay, per route.
fn distribution(relay: &Relay, load: &Load, cases: &mut Cases) {
    let mut routes: BTreeMap<&str, Vec<&Request>> = BTreeMap::new();
    for r in &relay.requests {
        routes.entry(r.route.as_str()).or_default().push(r);
    }
    for (route, requests) in routes {
        let mut waits: Vec<u64> = requests
            .iter()
            .filter(|r| r.wait > 0)
            .map(|r| r.wait)
            .collect();
        waits.sort_unstable();
        let ends = [End::None, End::Drained, End::Bound, End::Closed].map(|e| {
            format!(
                "{} {}",
                e.name(),
                requests.iter().filter(|r| r.end == e).count()
            )
        });
        let waited = if waits.is_empty() {
            "no wait".to_owned()
        } else {
            let longest = requests
                .iter()
                .max_by_key(|r| r.wait)
                .map_or(String::new(), |r| load.beside(r.ended()));
            format!(
                "waits median {} ms, p99 {} ms, max {} ms ({longest})",
                percentile(&waits, 50),
                percentile(&waits, 99),
                waits.last().copied().unwrap_or(0)
            )
        };
        cases.record(
            SECTION,
            format!(
                "{}: {route}: {} requests, {} waited ({:.1}%), {waited}; ends {}",
                relay.name,
                requests.len(),
                waits.len(),
                100.0 * waits.len() as f64 / requests.len().max(1) as f64,
                ends.join(", ")
            ),
        );
    }
}

/// The AwaitBlock report of the run in `run_dir`.
pub fn report(run_dir: &Path) -> Result<Report> {
    let manifest = Manifest::load(run_dir)?;
    let windows = run::windows(run_dir, manifest.window_seconds)?;
    let mut cases = Cases {
        run: manifest.run.clone(),
        results: Vec::new(),
    };
    checks(run_dir, &manifest, &windows, &mut cases);
    Ok(Report {
        title: format!(
            "AwaitBlock report of run {} ({})",
            manifest.run, manifest.network
        ),
        source: run_dir.display().to_string(),
        files: manifest
            .nodes
            .iter()
            .filter(|n| n.implementation == Implementation::Heartwood)
            .count(),
        results: cases.results,
    })
}

/// The AwaitBlock checks of a mixed run's live check.
pub(super) fn gate(run: &MixedRun<'_>, cases: &mut Cases) -> Result<()> {
    checks(run.run_dir, run.manifest, run.windows, cases);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(began: i64, wait: u64, end: End, before: u64, after: u64) -> Request {
        Request {
            at: began,
            route: "getStatus".into(),
            wait,
            end,
            before,
            after,
        }
    }

    #[test]
    fn percentiles_are_nearest_rank() {
        let waits: Vec<u64> = (1..=100).collect();
        assert_eq!(percentile(&waits, 50), 50);
        assert_eq!(percentile(&waits, 99), 99);
        assert_eq!(percentile(&waits, 100), 100);
        assert_eq!(percentile(&[7], 99), 7);
        assert_eq!(percentile(&[], 50), 0);
    }

    #[test]
    fn a_record_line_parses_and_a_bad_one_is_named() {
        let good = serde_json::json!({"t": "2026-09-29T10:00:08.123Z", "route": "getStatus",
            "caller": "198.51.100.15", "waitMs": 8004, "end": "bound", "tipBefore": 110, "tipAfter": 110});
        let r = Request::parse(&good).unwrap();
        assert_eq!((r.wait, r.end, r.before), (8004, End::Bound, 110));
        // `t` is when the request reached the wait, as the relay writes it.
        assert_eq!(
            r.began(),
            run::parse_time("2026-09-29T10:00:08.123Z").unwrap()
        );
        assert_eq!(r.ended(), r.began() + 8004);
        let bad = serde_json::json!({"t": "2026-09-29T10:00:08.123Z", "route": "getStatus",
            "caller": "x", "waitMs": 1, "end": "late", "tipBefore": 1, "tipAfter": 1});
        assert!(Request::parse(&bad).unwrap_err().contains("not an end"));
    }

    /// A stall whose job was held from 100 s to 112 s, the commit noted at 112 s.
    fn window() -> Window {
        Window {
            fault: "stall".into(),
            nodes: vec!["node4".into()],
            start: 95_000,
            end: 142_000,
        }
    }

    #[test]
    fn a_stall_with_bound_and_closed_ends_passes() {
        let requests = vec![
            request(100_500, 8_010, End::Bound, 110, 110),
            request(101_000, 8_030, End::Bound, 110, 110),
            request(102_000, 6_500, End::Closed, 110, 110),
            request(105_000, 7_000, End::Drained, 110, 111),
            request(113_000, 0, End::None, 111, 111),
            request(120_000, 40, End::Drained, 111, 112),
            request(130_000, 0, End::None, 112, 113),
        ];
        let v = judge_stall(&requests, &window(), 30, Some(100_000), true);
        assert_eq!(v.problems, Vec::<String>::new());
        assert_eq!((v.bound, v.closed, v.pre_tip), (2, 1, Some(110)));
    }

    #[test]
    fn a_stall_without_a_bound_end_fails() {
        let requests = vec![request(105_000, 7_000, End::Drained, 110, 111)];
        let v = judge_stall(&requests, &window(), 30, Some(100_000), false);
        assert!(
            v.problems
                .iter()
                .any(|p| p.contains("no wait ended `bound`"))
        );
    }

    #[test]
    fn an_early_wait_that_drained_or_a_late_bound_end_fails() {
        // Began 12 s before the commit, ended drained: it should have hit the bound.
        let drained = vec![
            request(100_000, 8_010, End::Bound, 110, 110),
            request(100_100, 12_000, End::Drained, 110, 111),
        ];
        let v = judge_stall(&drained, &window(), 30, Some(100_000), false);
        assert!(v.problems.iter().any(|p| p.contains("began")));
        // A bound end after 8.9 s, and one that began 3 s before the commit.
        let late = vec![
            request(100_000, 8_900, End::Bound, 110, 110),
            request(109_000, 8_000, End::Bound, 110, 110),
        ];
        let v = judge_stall(&late, &window(), 30, Some(100_000), false);
        assert!(
            v.problems
                .iter()
                .any(|p| p.contains("not between 8.0 and 8.3 s"))
        );
        assert!(
            v.problems
                .iter()
                .any(|p| p.contains("ended `bound` (ended"))
        );
    }

    #[test]
    fn a_record_with_extreme_numbers_is_judged_without_a_panic() {
        let requests = vec![
            request(100_000, 8_010, End::Bound, u64::MAX, u64::MAX),
            request(i64::MIN, u64::MAX, End::Drained, 1, 2),
            request(113_000, 0, End::None, u64::MAX, u64::MAX),
        ];
        let v = judge_stall(&requests, &window(), 30, Some(100_000), false);
        assert!(v.problems.iter().any(|p| p.contains("go past")));
        let load = Load(vec![(i64::MIN, 1.0), (i64::MAX, 2.0)]);
        assert_eq!(load.beside(0), "no load sample");
    }

    #[test]
    fn the_tip_before_the_job_must_hold_until_the_commit() {
        let requests = vec![
            request(100_000, 8_010, End::Bound, 110, 110),
            request(104_000, 8_000, End::Drained, 110, 110),
            request(113_000, 0, End::None, 110, 110),
        ];
        let v = judge_stall(&requests, &window(), 30, Some(100_000), false);
        assert!(
            v.problems
                .iter()
                .any(|p| p.contains("`drained` on the tip before"))
        );
        assert!(
            v.problems
                .iter()
                .any(|p| p.contains("after the commit carried"))
        );
        assert!(v.problems.iter().any(|p| p.contains("go past")));
    }
}
