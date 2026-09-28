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

/// The lines of the Heartwood relay's logs that record a reply refused at its schema, or a
/// protocol violation, naming one of `peers` (the reference nodes' addresses).
pub fn heartwood_logs(run_dir: &Path, node: &Node, peers: &[&str]) -> Vec<Check> {
    let dir = run_dir.join("nodes").join(&node.name).join("log");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return vec![fail(
            format!("{}: logs", node.name),
            format!("no logs in {}", dir.display()),
        )];
    };
    let mut counts: BTreeMap<(String, &str), usize> = BTreeMap::new();
    let mut lines = 0usize;
    for entry in entries.filter_map(std::result::Result::ok) {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("log") {
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        for line in String::from_utf8_lossy(&bytes).lines() {
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
        }
    }
    let label = format!(
        "{}: no reply refusal and no violation against a reference peer",
        node.name
    );
    if counts.is_empty() {
        vec![pass(format!("{label} ({lines} log lines read)"))]
    } else {
        counts
            .into_iter()
            .map(|((ip, kind), n)| fail(&label, format!("{n} lines: {kind} of {ip}")))
            .collect()
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
