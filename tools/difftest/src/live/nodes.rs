//! The checks of each node under test.
//!
//! A Heartwood relay: its digest lines against the judged chains ([`crate::live::digest`]), its
//! store against node 1's rows (the store anchors of [`crate::store`], with the missed rows of
//! blocks node 1 refused or undid listed: the reference writes them before it validates a block,
//! Heartwood only with an applied block), its stored tip, and its logs: no reply-schema refusal and
//! no protocol violation recorded against a reference peer.
//!
//! A reference relay (a dry run of the recipe, and node 4 of run `r`): its archive imported like node
//! 1's and compared with it: the same blocks, block columns and round rows, and the same missed
//! rows but at heights either node refused or undid a block.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use heartwood_database::{Database, Settings};
use serde_json::Value;

use crate::corpus::{self, Corpus};
use crate::errors::{Error, Result};
use crate::live::digest::{self, JudgedChain};
use crate::live::run::{Manifest, Node, Step, archives, read_lines};
use crate::report::Outcome;
use crate::store::{self, ScratchDir};
use crate::subject::Documented;

/// One result of a node's checks: a label, the height it concerns, and the outcome.
pub type Check = (String, u32, Outcome);

fn pass(label: impl Into<String>) -> Check {
    (label.into(), 0, Outcome::Pass)
}

fn fail(label: impl Into<String>, message: impl Into<String>) -> Check {
    (label.into(), 0, Outcome::HarnessError(message.into()))
}

/// At most this many problems of one kind are listed one by one.
const LISTED: usize = 20;

fn listed(label: &str, problems: &[String], out: &mut Vec<Check>) {
    for p in problems.iter().take(LISTED) {
        out.push(fail(label, p.clone()));
    }
    if problems.len() > LISTED {
        out.push(fail(
            label,
            format!("... and {} more", problems.len() - LISTED),
        ));
    }
}

/// The digest lines of the Heartwood relay `node` in `run_dir` against `chains` (the final chain
/// first); its starts against the run's scripted `faults` (each a fault's name and the nodes it
/// concerns); and, when the run killed it during its initial sync (`kill`, that step), that the
/// kill landed before the node caught up.
pub fn heartwood_digest(
    run_dir: &Path,
    node: &Node,
    chains: &[JudgedChain],
    faults: &[(String, Vec<String>)],
    kill: Option<&Step>,
) -> Vec<Check> {
    let label = format!("{}: digest lines", node.name);
    let dir = run_dir.join("nodes").join(&node.name).join("difftest");
    let read = match digest::read(&dir) {
        Ok(d) if d.files.is_empty() => {
            return vec![fail(&label, format!("no digest file in {}", dir.display()))];
        }
        Ok(d) => d,
        Err(e) => return vec![fail(&label, e.to_string())],
    };
    let judged = digest::judge(&read, chains);
    let mut out = Vec::new();
    for (start, text) in &read.torn {
        out.push(pass(format!(
            "{}: a torn last line of start {start} dropped ({} bytes)",
            node.name,
            text.len()
        )));
    }
    listed(&label, &judged.problems, &mut out);
    if !judged.uncovered.is_empty() {
        let shown: Vec<String> = judged
            .uncovered
            .iter()
            .take(LISTED)
            .map(u32::to_string)
            .collect();
        out.push(fail(
            format!("{}: every height of the final chain applied", node.name),
            format!(
                "{} heights without an apply line of the final chain's block: {}",
                judged.uncovered.len(),
                shown.join(", ")
            ),
        ));
    }
    if judged.problems.is_empty() && judged.uncovered.is_empty() {
        let counts: Vec<String> = judged
            .compared
            .iter()
            .map(|((chain, step), n)| format!("{n} {step} lines of {chain}"))
            .collect();
        out.push(pass(format!(
            "{}: {} digest lines in {} starts equal the judge's ({}), {} restart lines",
            node.name,
            read.lines.len(),
            read.files.len(),
            counts.join(", "),
            judged.restarts
        )));
    }
    match digest::starts(&read, &node.name, faults) {
        Ok(text) => out.push(pass(format!("{}: {text}", node.name))),
        Err(e) => out.push(fail(format!("{}: its starts", node.name), e)),
    }
    if let Some(step) = kill {
        let label = format!("{}: a kill during the initial sync", node.name);
        let target = step
            .record
            .get("node1Height")
            .and_then(Value::as_u64)
            .and_then(|h| u32::try_from(h).ok());
        match target {
            None => out.push(fail(
                &label,
                "the step does not record node 1's height at the node's start (node1Height)",
            )),
            Some(target) => match digest::killed_during_sync(&read, target) {
                Ok(text) => out.push(pass(format!("{label}: {text}"))),
                Err(e) => out.push(fail(&label, e)),
            },
        }
    }
    out
}

/// A missed row: timestamp, height, username.
type MissedKey = (i64, u64, String);

/// The node's missed rows and its refused or undone heights, from a corpus's `reference/`.
fn reference_rows(dir: &Path) -> Result<(Vec<MissedKey>, BTreeSet<u64>)> {
    let rows = read_lines(&dir.join("reference/missed-blocks.jsonl"))?
        .iter()
        .map(|r| {
            (
                r.get("timestamp").and_then(Value::as_i64).unwrap_or(-1),
                r.get("height").and_then(Value::as_u64).unwrap_or(0),
                r.get("username")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned(),
            )
        })
        .collect();
    let refused = read_lines(&dir.join("reference/rejections.jsonl"))?
        .iter()
        .filter_map(|r| r.get("height").and_then(Value::as_u64))
        .collect();
    Ok((rows, refused))
}

/// The store of the Heartwood relay `node` against the final chain's corpus in `corpus_dir`.
pub fn heartwood_store(
    run_dir: &Path,
    node: &Node,
    corpus_dir: &Path,
    tip: u32,
    round_length: u32,
) -> Vec<Check> {
    let label = format!("{}: store", node.name);
    let data = run_dir.join("nodes").join(&node.name).join("data");
    if !data.join("database").exists() {
        return vec![fail(&label, format!("no store under {}", data.display()))];
    }
    // A copy: opening a store may write to it (its journal, a schema step).
    let copy = match ScratchDir::new("live-store") {
        Ok(c) => c,
        Err(e) => return vec![fail(&label, e.to_string())],
    };
    if let Err(e) = copy_dir(&data, copy.path()) {
        return vec![fail(&label, e.to_string())];
    }
    let opened = store::guarded(|| Database::open(&Settings::in_data_dir(copy.path())));
    let database = match opened {
        Err(unbuilt) => {
            return vec![(
                label,
                0,
                Outcome::Skipped(format!("the store is not built yet: {}", unbuilt.0)),
            )];
        }
        Ok(Err(e)) => return vec![fail(&label, format!("the store does not open: {e}"))],
        Ok(Ok(d)) => d,
    };
    let mut out = Vec::new();
    match database.last_block() {
        Ok(Some(b)) if b.row.height == tip => out.push(pass(format!(
            "{}: the stored tip is the final tip {tip}",
            node.name
        ))),
        Ok(Some(b)) => out.push(fail(
            &label,
            format!("the stored tip is {}, the final tip {tip}", b.row.height),
        )),
        Ok(None) => out.push(fail(&label, "the store holds no block")),
        Err(e) => out.push(fail(&label, e.to_string())),
    }
    let last_round = store::last_round(tip, round_length);
    let rows = match store::read_rows(&database, tip, last_round) {
        Ok(r) => r,
        Err(e) => {
            out.push(fail(&label, e));
            return out;
        }
    };
    for (anchor, outcome) in store::anchors(corpus_dir, &rows, tip, last_round) {
        out.push((format!("{}: {anchor}", node.name), 0, outcome));
    }
    // The missed rows the anchors left out: node 1's rows at heights it refused or undid a block.
    match reference_rows(corpus_dir) {
        Err(e) => out.push(fail(&label, e.to_string())),
        Ok((node_rows, refused)) => {
            let ours: BTreeSet<MissedKey> = rows
                .missed
                .iter()
                .map(|m| (m.timestamp, u64::from(m.height), m.username.clone()))
                .collect();
            let theirs: BTreeSet<MissedKey> = node_rows.iter().cloned().collect();
            for row in node_rows.iter().filter(|r| refused.contains(&r.1)) {
                if ours.contains(row) {
                    out.push(pass(format!(
                        "{}: missed row {row:?} at a height node 1 refused or undid a block is in both stores",
                        node.name
                    )));
                } else {
                    out.push((
                        format!(
                            "{}: missed row {row:?} of a block node 1 refused or undid, which Heartwood does not write",
                            node.name
                        ),
                        u32::try_from(row.1).unwrap_or(0),
                        Outcome::Documented(Documented::MissedRowsOfRefusedBlock.reason().to_owned()),
                    ));
                }
            }
            for row in ours
                .iter()
                .filter(|r| refused.contains(&r.1) && !theirs.contains(*r))
            {
                out.push(fail(
                    &label,
                    format!("missed row {row:?} at a height node 1 refused or undid a block: only Heartwood has it"),
                ));
            }
        }
    }
    out
}

fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    std::fs::create_dir_all(to).map_err(|e| Error::io(to, e))?;
    for entry in std::fs::read_dir(from).map_err(|e| Error::io(from, e))? {
        let entry = entry.map_err(|e| Error::io(from, e))?;
        let path = entry.path();
        let target = to.join(entry.file_name());
        if path.is_dir() {
            copy_dir(&path, &target)?;
        } else {
            std::fs::copy(&path, &target).map_err(|e| Error::io(&path, e))?;
        }
    }
    Ok(())
}

/// The text of the log file at `path`: a rotated file (`.log.gz`) decompressed.
pub(super) fn log_text(path: &Path) -> std::io::Result<String> {
    use std::io::Read;
    let bytes = std::fs::read(path)?;
    if path.extension().and_then(|e| e.to_str()) != Some("gz") {
        return Ok(String::from_utf8_lossy(&bytes).into_owned());
    }
    let mut text = Vec::new();
    flate2::read::MultiGzDecoder::new(bytes.as_slice()).read_to_end(&mut text)?;
    Ok(String::from_utf8_lossy(&text).into_owned())
}

/// Whether `name` is a log file of the relay: a stream the supervisor keeps (`relay.out.log`), the
/// relay's own file log (`<stem>-current.log`), or one the file log rotated at midnight UTC
/// (`<stem>-<date>.log.gz`).
fn is_log(name: &str) -> bool {
    name.ends_with(".log") || name.ends_with(".log.gz")
}

/// Whether `name` is the relay's own file log, live or rotated, which alone holds its debug lines.
pub(super) fn is_file_log(name: &str) -> bool {
    name.ends_with("-current.log") || name.ends_with(".log.gz")
}

/// The lines of the Heartwood relay's logs (rotated files included) that record a reply refused at
/// its schema, or a protocol violation, naming one of `peers` (the reference nodes' addresses);
/// and, recorded from its file log, the peers its cleanse removed (by failure) and the times it
/// fell back to its seeds for want of peers.
pub fn heartwood_logs(run_dir: &Path, node: &Node, peers: &[&str]) -> Vec<Check> {
    let dir = run_dir.join("nodes").join(&node.name).join("log");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return vec![fail(
            format!("{}: logs", node.name),
            format!("no logs in {}", dir.display()),
        )];
    };
    let mut counts: BTreeMap<(String, &str), usize> = BTreeMap::new();
    let mut removed: BTreeMap<String, u64> = BTreeMap::new();
    let mut fallbacks = 0usize;
    let mut lines = 0usize;
    let mut unread = Vec::new();
    let mut paths: Vec<_> = entries
        .filter_map(std::result::Result::ok)
        .map(|e| e.path())
        .collect();
    paths.sort();
    for path in paths {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if !is_log(&name) {
            continue;
        }
        let text = match log_text(&path) {
            Ok(text) => text,
            Err(e) => {
                unread.push(format!("{name}: {e}"));
                continue;
            }
        };
        let file_log = is_file_log(&name);
        for line in text.lines() {
            lines += 1;
            for (marker, kind) in [
                ("Protocol violation by peer ", "protocol violation"),
                (
                    "Response validation failed from peer ",
                    "reply refused at its schema",
                ),
            ] {
                if let Some(rest) = line.split(marker).nth(1) {
                    let ip = rest.split([' ', ':']).next().unwrap_or("").to_owned();
                    if peers.contains(&ip.as_str()) {
                        *counts.entry((ip, kind)).or_default() += 1;
                    }
                }
            }
            if !file_log {
                continue;
            }
            // "Removed 2 peers because of \"<failure>\"": the cleanse's removals.
            if let Some(rest) = line.split("Removed ").nth(1)
                && let Some((count, failure)) = rest.split_once(" because of \"")
            {
                let n = count
                    .split(' ')
                    .next()
                    .and_then(|n| n.parse::<u64>().ok())
                    .unwrap_or(0);
                let failure = failure.split('"').next().unwrap_or("").to_owned();
                *removed.entry(failure).or_default() += n;
            }
            if line.contains("Couldn't find enough peers. Falling back to seed peers") {
                fallbacks += 1;
            }
        }
    }
    let label = format!(
        "{}: no reply refusal and no violation against a reference peer",
        node.name
    );
    let mut out: Vec<Check> = if counts.is_empty() {
        vec![pass(format!("{label} ({lines} log lines read)"))]
    } else {
        counts
            .into_iter()
            .map(|((ip, kind), n)| fail(&label, format!("{n} lines: {kind} of {ip}")))
            .collect()
    };
    for why in unread {
        out.push(fail(
            format!("{}: logs", node.name),
            format!("unreadable: {why}"),
        ));
    }
    let removals: Vec<String> = removed
        .iter()
        .map(|(failure, n)| format!("{n} for \"{failure}\""))
        .collect();
    out.push(pass(format!(
        "recorded: {}: its cleanse removed peers {}; it fell back to its seeds {fallbacks} times",
        node.name,
        if removals.is_empty() {
            "never".to_owned()
        } else {
            removals.join(", ")
        }
    )));
    out
}

/// A line of a node's log: `[YYYY-MM-DD HH:MM:SS.mmm] LEVEL: text`, the time in UTC.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct LogLine {
    /// When, in milliseconds since the Unix epoch.
    pub at: Option<i64>,
    /// The line's text after its level.
    pub text: String,
}

/// The line `line` of a log, when it has the format's time and level; a continuation line has
/// none.
pub(super) fn log_line(line: &str) -> Option<LogLine> {
    let rest = line.strip_prefix('[')?;
    let (stamp, rest) = rest.split_once("] ")?;
    let (level, text) = rest.split_once(": ")?;
    if level.is_empty() || !level.bytes().all(|b| b.is_ascii_uppercase()) {
        return None;
    }
    Some(LogLine {
        at: super::run::parse_time(&format!("{stamp}Z")),
        text: text.to_owned(),
    })
}

/// The lines of the logs in `dir` in time order of the files: the relay's own file log, rotated
/// files (`.log.gz`) first, which alone hold its debug lines; the streams a supervisor keeps
/// repeat its console, so they are read only when there is no file log. A directory that is not
/// there is an error, a file that cannot be read another.
pub(super) fn read_logs(dir: &Path) -> Result<Vec<LogLine>> {
    let mut paths: Vec<_> = std::fs::read_dir(dir)
        .map_err(|e| Error::io(dir, e))?
        .filter_map(std::result::Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .is_some_and(|n| is_log(&n.to_string_lossy()))
        })
        .collect();
    paths.sort();
    let named = |p: &std::path::PathBuf| {
        p.file_name()
            .is_some_and(|n| is_file_log(&n.to_string_lossy()))
    };
    if paths.iter().any(named) {
        paths.retain(named);
    }
    let mut out = Vec::new();
    for path in paths {
        let text = log_text(&path).map_err(|e| Error::io(&path, e))?;
        out.extend(text.lines().filter_map(log_line));
    }
    Ok(out)
}

/// What a log line records of a peer, for the checks that count the lines naming an address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Event {
    /// "Banning <ip> for <duration>. Reason: <reason>": the reason without its trailing emoji or
    /// short code.
    Ban {
        /// The banned address.
        ip: String,
        /// The reason.
        reason: String,
    },
    /// "Got unexpected reply from <url>": a reply the client could not use.
    UnexpectedReply(String),
    /// "Socket data validation error (peer <ip>": a reply whose content was refused.
    ValidationError(String),
    /// "Response error (peer <ip>/<route>)": how a client logs a timeout.
    ResponseError(String),
    /// "Socket error (peer <ip>)".
    SocketError(String),
    /// "Accepted new peer <ip>:<port>".
    Accepted(String),
}

impl Event {
    /// The address the event names.
    pub(super) fn ip(&self) -> &str {
        match self {
            Event::Ban { ip, .. }
            | Event::UnexpectedReply(ip)
            | Event::ValidationError(ip)
            | Event::ResponseError(ip)
            | Event::SocketError(ip)
            | Event::Accepted(ip) => ip,
        }
    }
}

/// The address at the start of `rest` (after any `scheme://`): digits and dots.
fn address(rest: &str) -> String {
    let rest = rest.split_once("://").map_or(rest, |(_, r)| r);
    rest.chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect()
}

/// The event a log line's `text` records, if it records one of them.
pub(super) fn event_of(text: &str) -> Option<Event> {
    if let Some(rest) = text.strip_prefix("Banning ") {
        let (ip, rest) = rest.split_once(" for ")?;
        let (_, reason) = rest.split_once(". Reason: ")?;
        return Some(Event::Ban {
            ip: ip.to_owned(),
            reason: super::bans::reason_text(reason).to_owned(),
        });
    }
    let after = |marker: &str| text.split_once(marker).map(|(_, rest)| address(rest));
    let ip = |marker: &str| after(marker).filter(|ip| !ip.is_empty());
    if let Some(ip) = ip("Got unexpected reply from ") {
        Some(Event::UnexpectedReply(ip))
    } else if let Some(ip) = ip("Socket data validation error (peer ") {
        Some(Event::ValidationError(ip))
    } else if let Some(ip) = ip("Response error (peer ") {
        Some(Event::ResponseError(ip))
    } else if let Some(ip) = ip("Socket error (peer ") {
        Some(Event::SocketError(ip))
    } else {
        ip("Accepted new peer ").map(Event::Accepted)
    }
}

/// A corpus's rows keyed for comparison: blocks by height, block columns by height, round rows by
/// round as sets, missed rows as a multiset.
struct Rows {
    blocks: BTreeMap<u64, String>,
    meta: BTreeMap<u64, Value>,
    rounds: BTreeMap<u64, BTreeSet<(String, String)>>,
    missed: Vec<MissedKey>,
    refused: BTreeSet<u64>,
}

fn rows_of(dir: &Path) -> Result<Rows> {
    let height = |v: &Value, k: &str| v.get(k).and_then(Value::as_u64).unwrap_or(0);
    let blocks = read_lines(&dir.join("blocks.jsonl"))?
        .iter()
        .map(|b| {
            (
                height(b, "h"),
                b.get("id").and_then(Value::as_str).unwrap_or("").to_owned(),
            )
        })
        .collect();
    let meta = read_lines(&dir.join("reference/meta.jsonl"))?
        .into_iter()
        .map(|m| (height(&m, "h"), m))
        .collect();
    let mut rounds: BTreeMap<u64, BTreeSet<(String, String)>> = BTreeMap::new();
    for r in read_lines(&dir.join("reference/rounds.jsonl"))? {
        rounds.entry(height(&r, "round")).or_default().insert((
            r.get("publicKey")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned(),
            r.get("balance").map(Value::to_string).unwrap_or_default(),
        ));
    }
    let (missed, refused) = reference_rows(dir)?;
    Ok(Rows {
        blocks,
        meta,
        rounds,
        missed,
        refused,
    })
}

/// The reference relay `node` of the run: its archive against the final chain's corpus.
pub fn reference(
    run_dir: &Path,
    manifest: &Manifest,
    node: &Node,
    corpus_dir: &Path,
) -> Vec<Check> {
    let label = format!("{}: archive", node.name);
    let dirs = match archives(&run_dir.join("archives").join(&node.name)) {
        Ok(d) if !d.is_empty() => d,
        Ok(_) => return vec![fail(&label, "no archive of the node")],
        Err(e) => return vec![fail(&label, e.to_string())],
    };
    let scratch = match ScratchDir::new("live-reference") {
        Ok(s) => s,
        Err(e) => return vec![fail(&label, e.to_string())],
    };
    let dir = scratch.path().join(&node.name);
    if let Err(e) = corpus::import(&dirs, &dir, &manifest.seed) {
        return vec![fail(&label, format!("the archive does not import: {e}"))];
    }
    let (ours, theirs) = match (rows_of(&dir), rows_of(corpus_dir)) {
        (Ok(a), Ok(b)) => (a, b),
        (Err(e), _) | (_, Err(e)) => return vec![fail(&label, e.to_string())],
    };
    let tip = Corpus::load(&dir).map(|c| c.tip()).unwrap_or(0);
    let mut out = Vec::new();
    let mut problems = Vec::new();
    if ours.blocks != theirs.blocks {
        let first = theirs
            .blocks
            .iter()
            .find(|(h, id)| ours.blocks.get(h) != Some(id))
            .map(|(h, _)| *h);
        problems.push(format!(
            "the chains differ: {} blocks against node 1's {}, first at height {first:?}",
            ours.blocks.len(),
            theirs.blocks.len()
        ));
    }
    if ours.meta != theirs.meta {
        problems.push("the block columns differ".into());
    }
    if ours.rounds != theirs.rounds {
        let first = theirs
            .rounds
            .keys()
            .find(|r| ours.rounds.get(r) != theirs.rounds.get(r));
        problems.push(format!("the round rows differ, first in round {first:?}"));
    }
    let skip: BTreeSet<u64> = ours.refused.union(&theirs.refused).copied().collect();
    let keep = |rows: &[MissedKey]| {
        let mut r: Vec<_> = rows
            .iter()
            .filter(|r| !skip.contains(&r.1))
            .cloned()
            .collect();
        r.sort();
        r
    };
    if keep(&ours.missed) != keep(&theirs.missed) {
        problems.push(
            "the missed rows differ (at heights neither node refused or undid a block)".into(),
        );
    }
    if problems.is_empty() {
        out.push(pass(format!(
            "{}: its chain of {tip} blocks, block columns, round rows and missed rows equal node 1's",
            node.name
        )));
    } else {
        listed(&label, &problems, &mut out);
    }
    out
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;
    use crate::live::run::Implementation;
    use crate::vectors::testutil::TestDir;

    #[test]
    fn log_lines_are_read_as_events() {
        let line = |text: &str| {
            let l = log_line(&format!("[2026-09-27 10:24:19.332] DEBUG: {text}")).unwrap();
            assert_eq!(l.at, super::super::run::parse_time("2026-09-27T10:24:19.332Z"));
            event_of(&l.text)
        };
        assert_eq!(
            line("Banning 10.82.0.14 for 10 seconds. Reason: Did not make an endpoint request 🚔"),
            Some(Event::Ban {
                ip: "10.82.0.14".into(),
                reason: "Did not make an endpoint request".into()
            })
        );
        assert_eq!(
            line("Banning 10.82.0.11 for 5 minutes. Reason: Validation failed :oncoming_police_car:"),
            Some(Event::Ban {
                ip: "10.82.0.11".into(),
                reason: "Validation failed".into()
            })
        );
        assert_eq!(
            line("Got unexpected reply from ws://10.82.0.14:6002/p2p.peer.getPeers: {}"),
            Some(Event::UnexpectedReply("10.82.0.14".into()))
        );
        assert_eq!(
            line("Socket data validation error (peer 10.82.0.14) : ..."),
            Some(Event::ValidationError("10.82.0.14".into()))
        );
        assert_eq!(
            line("Response error (peer 10.82.0.14/p2p.blocks.getBlocks) : timeout"),
            Some(Event::ResponseError("10.82.0.14".into()))
        );
        assert_eq!(
            line("Socket error (peer 10.82.0.14): Socket error ⚠️"),
            Some(Event::SocketError("10.82.0.14".into()))
        );
        assert_eq!(
            line("Accepted new peer 10.82.0.14:6002 (v4.3.1) 🤗"),
            Some(Event::Accepted("10.82.0.14".into()))
        );
        assert_eq!(line("Starting Round 3"), None);
        assert!(log_line("  at Object.<anonymous> (file.js:1:1)").is_none());
    }

    #[test]
    fn a_log_directory_reads_the_file_logs_with_the_rotated_ones_first() {
        let dir = TestDir::new("live-log-order");
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gz.write_all(b"[2026-09-27 23:59:01.000] DEBUG: rotated\n").unwrap();
        std::fs::write(dir.path().join("solar-testnet-relay-2026-09-27.log.gz"), gz.finish().unwrap())
            .unwrap();
        std::fs::write(
            dir.path().join("solar-testnet-relay-current.log"),
            "[2026-09-28 00:00:01.000] DEBUG: current\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("relay.out.log"),
            "[2026-09-28 00:00:01.000] INFO: console\n",
        )
        .unwrap();
        let lines = read_logs(dir.path()).unwrap();
        let texts: Vec<&str> = lines.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(texts, ["rotated", "current"]);
        assert!(read_logs(&dir.path().join("absent")).is_err());
    }

    #[test]
    fn the_log_scan_reads_rotated_files_and_records_the_cleanse() {
        let dir = TestDir::new("live-logs");
        let log = dir.path().join("nodes/node4/log");
        std::fs::create_dir_all(&log).unwrap();
        // The day before midnight, rotated: a violation and a removal, only in the file log.
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gz.write_all(
            b"[2026-09-27 23:59:01.000] DEBUG: Protocol violation by peer 198.51.100.11: Protocol :warning:\n\
              [2026-09-27 23:59:02.000] DEBUG: Removed 2 peers because of \"Request timed out\" :wastebasket:\n",
        )
        .unwrap();
        std::fs::write(
            log.join("solar-testnet-relay-2026-09-28.log.gz"),
            gz.finish().unwrap(),
        )
        .unwrap();
        std::fs::write(
            log.join("solar-testnet-relay-current.log"),
            "[2026-09-28 00:00:03.000] DEBUG: Removed 1 peer because of \"Request timed out\" :wastebasket:\n\
             [2026-09-28 00:00:04.000] INFO: Couldn't find enough peers. Falling back to seed peers :fire:\n",
        )
        .unwrap();
        // The supervisor's copy of the console: its info lines are not counted twice.
        std::fs::write(
            log.join("relay.out.log"),
            "[2026-09-28 00:00:04.000] INFO: Couldn't find enough peers. Falling back to seed peers :fire:\n",
        )
        .unwrap();
        let node = Node {
            name: "node4".into(),
            ip: "198.51.100.14".into(),
            implementation: Implementation::Heartwood,
            forger: false,
            under_test: true,
        };
        let checks = heartwood_logs(dir.path(), &node, &["198.51.100.11"]);
        let failed: Vec<&Check> = checks.iter().filter(|c| c.2.is_failure()).collect();
        assert_eq!(failed.len(), 1, "{checks:?}");
        assert!(
            matches!(&failed[0].2, Outcome::HarnessError(m) if m.contains("protocol violation of 198.51.100.11")),
            "{checks:?}"
        );
        assert!(
            checks.iter().any(|c| c.0
                == "recorded: node4: its cleanse removed peers 3 for \"Request timed out\"; it fell back to its seeds 1 times"),
            "{checks:?}"
        );
    }
}
