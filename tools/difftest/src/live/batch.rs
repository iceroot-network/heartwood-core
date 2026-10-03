//! A scripted batch step's relay that did not list the whole batch before its next block because
//! of the reference's own race: a reference relay that receives one batch from two relays at once
//! (the relay it was posted to and a peer that relayed it within milliseconds) runs the two posts
//! through its pool side by side. Each post takes the transactions the other has not taken yet
//! (the processor's `cachedTransactions`), and their adds interleave, so the later nonces of the
//! one sender are refused for a nonce gap that the other post fills a moment later. The relay
//! then holds part of the batch at the next block and takes the rest later; the transactions are
//! forged later.
//!
//! The relay's own pool lines name no peer and no post: one post whose transactions arrive out of
//! nonce order writes the same lines. So the step is documented ([`REFERENCE_BATCH_RACE`]) only
//! with the two posts shown by the other relays' logs as well. Every relay that did not list the
//! whole batch is a reference relay whose own log shows, within the step, one outcome for each
//! transaction of the batch, all of one sender with consecutive nonces, some added and the others
//! refused with the message "Cannot apply a transaction with nonce N: the sender has nonce M" for
//! a gap inside the batch (M + 1 < N), at least one refused before a lower nonce of the batch was
//! added (the batch was taken apart: a post in nonce order, alone, adds the lower nonce first), in
//! an order two posts each in nonce order can give (the nonces form at most two rising runs); two
//! relays other than it logged a broadcast of exactly the batch's number of transactions within
//! the step, at most a millisecond a transaction apart (the time the reference's pool takes over
//! a post's first loop) and both no later than its pool's first line of the batch; another
//! reference relay took the whole batch within the step in nonce order with no refusal (a post of
//! the batch was in nonce order); and every transaction of the batch is in a block of the run's
//! chain (node 1's archive) above the height the batch was posted at.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::nodes;
use super::run::{self, Implementation, Manifest, Step};

/// The reason of a batch step documented as the reference's race.
pub const REFERENCE_BATCH_RACE: &str = "a reference relay that received one batch from two relays at once (two other relays broadcast the whole batch within milliseconds of each other before its pool took it, and another reference relay took the batch whole in nonce order): its pool ran the two posts side by side, took the batch apart between them and refused the later nonces of the sender for a gap the other post filled a moment later (the reference's race; every refused transaction was forged later and every Heartwood relay listed the whole batch)";

/// The reference's batch race at the relays that did not list the whole batch of `step` (a
/// failed `batch` step) before their next block: `Ok` with the evidence of each such relay when
/// it shows the race, `Err` with why not.
pub fn reference_race(
    run_dir: &Path,
    manifest: &Manifest,
    step: &Step,
) -> std::result::Result<String, String> {
    let detail = &step.record["detail"];
    if detail["step"] != "batch" || detail["http"] != 200 {
        return Err("not a batch the relay accepted with 200".to_owned());
    }
    let ids: Vec<&str> = (detail["ids"].as_array().into_iter().flatten())
        .map(Value::as_str)
        .collect::<Option<_>>()
        .ok_or("the step records no batch ids")?;
    if ids.len() < 2
        || ids.iter().any(|id| id.len() != 64)
        || detail["accepted"].as_u64() != u64::try_from(ids.len()).ok()
    {
        return Err("the submitting relay did not accept the whole batch".to_owned());
    }
    let after = detail["afterHeight"]
        .as_u64()
        .ok_or("the step records no height")?;
    let relays = detail["relays"]
        .as_object()
        .ok_or("the step records no relay")?;
    let missed: Vec<&String> = (relays.iter())
        .filter(|(_, state)| state["seenAllMs"].is_null())
        .map(|(name, _)| name)
        .collect();
    if missed.is_empty() {
        return Err("every relay listed the whole batch".to_owned());
    }
    if let Some(name) = missed.iter().find(|name| {
        manifest
            .node(name)
            .is_none_or(|n| n.implementation != Implementation::Reference)
    }) {
        return Err(format!(
            "{name}, which is not a reference relay, did not list the whole batch"
        ));
    }
    let forged = forged_at(run_dir, manifest, &ids)?;
    if let Some(id) = ids
        .iter()
        .find(|id| forged.get(**id).is_none_or(|h| *h <= after))
    {
        return Err(format!(
            "transaction {id} is in no block of the chain above {after}"
        ));
    }
    let (Some(from), Some(to)) = (step.start, step.time("end")) else {
        return Err("the step records no start or no end".to_owned());
    };
    let within = |name: &str| -> std::result::Result<Vec<nodes::LogLine>, String> {
        let dir =
            log_dir(run_dir, manifest, name).ok_or_else(|| format!("{name}: no such node"))?;
        let lines = nodes::read_logs(&dir).map_err(|e| e.to_string())?;
        Ok((lines.into_iter())
            .filter(|l| l.at.is_some_and(|at| (from..=to).contains(&at)))
            .collect())
    };
    let witness = (manifest.nodes.iter())
        .filter(|n| n.implementation == Implementation::Reference)
        .filter(|n| !missed.contains(&&n.name))
        .find(|n| {
            within(&n.name)
                .is_ok_and(|lines| in_order(lines.iter().map(|l| l.text.as_str()), &ids))
        })
        .map(|n| n.name.clone())
        .ok_or("no other reference relay took the whole batch in nonce order within the step, so nothing shows a post of it in nonce order")?;
    let mut evidence = Vec::new();
    for name in missed {
        let lines = within(name)?;
        let taken = taken_apart(lines.iter().map(|l| (l.at, l.text.as_str())), &ids)
            .map_err(|why| format!("{name}: {why}"))?;
        let posts = two_posts(
            run_dir,
            manifest,
            name,
            ids.len(),
            (from, to),
            taken.first_at,
        )
        .map_err(|why| format!("{name}: {why}"))?;
        let heights: Vec<String> = ids.iter().map(|id| forged[*id].to_string()).collect();
        evidence.push(format!(
            "{name}: {}; {posts}; {witness} took the whole batch in nonce order; the batch was forged at heights {}",
            taken.text,
            heights.join(", ")
        ));
    }
    Ok(evidence.join("; "))
}

/// Where the logs of node `name` are: a reference relay's under `logs/`, a Heartwood relay's
/// under its node directory.
fn log_dir(run_dir: &Path, manifest: &Manifest, name: &str) -> Option<PathBuf> {
    let node = manifest.node(name)?;
    Some(match node.implementation {
        Implementation::Reference => run_dir.join("logs").join(name),
        Implementation::Heartwood => run_dir.join("nodes").join(name).join("log"),
    })
}

/// The number of transactions a relay's log line says it broadcast ("Broadcasting N
/// transactions to M peers").
fn broadcast_size(text: &str) -> Option<usize> {
    let rest = text.strip_prefix("Broadcasting ")?;
    let (count, rest) = rest.split_once(' ')?;
    rest.starts_with("transactions to ")
        .then(|| count.parse().ok())
        .flatten()
}

/// The two posts of the batch that reached relay `missed`: two other relays' broadcasts of exactly
/// `size` transactions within the step `(from, to)`, at most a millisecond a transaction apart
/// and both no later than `first` (the relay's first pool line of the batch). Their evidence as
/// text, or why there are none.
fn two_posts(
    run_dir: &Path,
    manifest: &Manifest,
    missed: &str,
    size: usize,
    (from, to): (i64, i64),
    first: i64,
) -> std::result::Result<String, String> {
    let window = i64::try_from(size).unwrap_or(i64::MAX);
    let mut sent: Vec<(i64, &str)> = Vec::new();
    for node in manifest.nodes.iter().filter(|n| n.name != missed) {
        let Some(dir) = log_dir(run_dir, manifest, &node.name) else {
            continue;
        };
        let Ok(lines) = nodes::read_logs(&dir) else {
            continue;
        };
        let at = (lines.iter())
            .filter(|l| broadcast_size(&l.text) == Some(size))
            .filter_map(|l| l.at)
            .find(|at| (from..=to).contains(at) && *at <= first);
        if let Some(at) = at {
            sent.push((at, node.name.as_str()));
        }
    }
    sent.sort_unstable();
    let pair = sent
        .windows(2)
        .find(|w| w[1].0.saturating_sub(w[0].0) <= window);
    let Some([(a_at, a), (b_at, b)]) = pair.map(|w| [w[0], w[1]]) else {
        return Err(format!(
            "no two other relays broadcast the {size} transactions within {window} ms of each other before its pool took them ({} did)",
            if sent.is_empty() {
                "none".to_owned()
            } else {
                (sent.iter())
                    .map(|(at, n)| format!("{n} at {}", run::format_time(*at)))
                    .collect::<Vec<_>>()
                    .join(", ")
            }
        ));
    };
    Ok(format!(
        "{a} and {b} broadcast the {size} transactions at {} and {}, {} ms before its pool's first line of them",
        run::format_time(a_at),
        run::format_time(b_at),
        first.saturating_sub(b_at)
    ))
}
/// The heights of the blocks of the run's chain (the archives of the control node, node 1's)
/// that hold the transactions `ids`.
fn forged_at(
    run_dir: &Path,
    manifest: &Manifest,
    ids: &[&str],
) -> std::result::Result<BTreeMap<String, u64>, String> {
    let archives = run::archives(&run_dir.join("archives").join(&manifest.control))
        .map_err(|e| e.to_string())?;
    if archives.is_empty() {
        return Err(format!("no archive of {}", manifest.control));
    }
    let mut out = BTreeMap::new();
    for archive in archives {
        let blocks = archive.join("blocks");
        let mut files: Vec<_> = std::fs::read_dir(&blocks)
            .map_err(|e| format!("{}: {e}", blocks.display()))?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .collect();
        files.sort();
        for file in files {
            for block in run::read_lines(&file).map_err(|e| e.to_string())? {
                let height = block["height"].as_u64().unwrap_or(0);
                for id in
                    (block["txIds"].as_array().into_iter().flatten()).filter_map(Value::as_str)
                {
                    if ids.contains(&id) {
                        out.insert(id.to_owned(), height);
                    }
                }
            }
        }
    }
    Ok(out)
}

/// What a reference relay's pool did with a transaction, as its log says: added it, or refused it
/// with a reason.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Outcome {
    Added,
    Refused(String),
}

/// A pool line of a reference relay's log: `<sender>#<nonce> <the id's last 8 characters>
/// <type> ... added to the pool` or `... failed to enter the pool: <reason>`.
fn pool_line(text: &str) -> Option<(String, u64, String, Outcome)> {
    let mut words = text.splitn(3, ' ');
    let (sender, nonce) = words.next()?.split_once('#')?;
    let nonce = nonce.parse().ok()?;
    let suffix = words.next()?;
    if suffix.len() != 8 || !suffix.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let rest = words.next()?;
    let outcome = if rest.ends_with(" added to the pool") {
        Outcome::Added
    } else {
        Outcome::Refused(rest.split_once(" failed to enter the pool: ")?.1.to_owned())
    };
    Some((sender.to_owned(), nonce, suffix.to_owned(), outcome))
}

/// The sender's nonce a refusal for a nonce gap names: "Cannot apply a transaction with nonce N:
/// the sender has nonce M", for the transaction's own nonce `nonce`.
fn gap(reason: &str, nonce: u64) -> Option<u64> {
    let rest = reason
        .split("Cannot apply a transaction with nonce ")
        .nth(1)?;
    let (named, rest) = rest.split_once(": the sender has nonce ")?;
    let has: String = rest.chars().take_while(char::is_ascii_digit).collect();
    (named.parse::<u64>().ok()? == nonce).then(|| has.parse().ok())?
}

/// The first outcome of each transaction of the batch `ids` in a reference relay's pool lines
/// `lines` (time and text), in the log's order: `(index in the batch, sender, nonce, outcome,
/// time)`.
fn first_outcomes<'a>(
    lines: impl Iterator<Item = (Option<i64>, &'a str)>,
    ids: &[&str],
) -> Vec<(usize, String, u64, Outcome, Option<i64>)> {
    let mut seen: Vec<(usize, String, u64, Outcome, Option<i64>)> = Vec::new();
    for (at, (sender, nonce, suffix, outcome)) in
        lines.filter_map(|(at, text)| Some((at, pool_line(text)?)))
    {
        let Some(index) = ids.iter().position(|id| id.ends_with(&suffix)) else {
            continue;
        };
        if seen.iter().all(|(i, ..)| *i != index) {
            seen.push((index, sender, nonce, outcome, at));
        }
    }
    seen
}

/// Whether the pool lines `lines` (a reference relay's log within the step) show the whole batch
/// `ids` taken in nonce order: the first outcome of each transaction, each added, in the batch's
/// order.
fn in_order<'a>(lines: impl Iterator<Item = &'a str>, ids: &[&str]) -> bool {
    let seen = first_outcomes(lines.map(|text| (None, text)), ids);
    seen.len() == ids.len()
        && (seen.iter().enumerate())
            .all(|(at, (index, _, _, outcome, _))| *index == at && *outcome == Outcome::Added)
}

/// The length of the longest falling run of `nonces` (each a distinct number): two posts each in
/// nonce order, interleaved, never give more than 2.
fn longest_fall(nonces: &[u64]) -> usize {
    // The smallest last element of a rising run of each length, read from the end (patience).
    let mut tails: Vec<u64> = Vec::new();
    for nonce in nonces.iter().rev() {
        let at = tails.partition_point(|tail| tail < nonce);
        match tails.get_mut(at) {
            Some(tail) => *tail = *nonce,
            None => tails.push(*nonce),
        }
    }
    tails.len()
}

/// What a reference relay's pool lines show of the batch taken apart: the evidence as text and
/// the time of the first outcome line of the batch.
struct TakenApart {
    text: String,
    first_at: i64,
}

/// Whether the pool lines `lines` (a relay's log within the step, with their times) show the
/// batch `ids` (in nonce order) taken apart by two posts: the first outcome of each transaction,
/// of one sender with consecutive nonces; the refused ones refused for a gap inside the batch;
/// one refused before a lower nonce of the batch was added; the nonces in an order two posts each
/// in nonce order can give (at most two rising runs interleaved). The evidence, or why not.
fn taken_apart<'a>(
    lines: impl Iterator<Item = (Option<i64>, &'a str)>,
    ids: &[&str],
) -> std::result::Result<TakenApart, String> {
    let seen = first_outcomes(lines, ids);
    if seen.len() != ids.len() {
        return Err(format!(
            "its log shows the outcome of {} of the {} transactions",
            seen.len(),
            ids.len()
        ));
    }
    let sender = &seen[0].1;
    let first = seen
        .iter()
        .find(|(i, ..)| *i == 0)
        .map_or(0, |(_, _, n, ..)| *n);
    for (index, from, nonce, ..) in &seen {
        let expected = (u64::try_from(*index).ok()).and_then(|i| first.checked_add(i));
        if from != sender || Some(*nonce) != expected || first == 0 {
            return Err("the batch is not one sender's consecutive nonces in its log".to_owned());
        }
    }
    let mut refused = Vec::new();
    let mut filled = false;
    for (at, (_, _, nonce, outcome, _)) in seen.iter().enumerate() {
        let Outcome::Refused(reason) = outcome else {
            continue;
        };
        let has = gap(reason, *nonce).ok_or_else(|| format!("nonce {nonce} refused: {reason}"))?;
        let next = has.saturating_add(1);
        if next >= *nonce || next < first {
            return Err(format!(
                "nonce {nonce} refused with the sender's nonce {has}: no gap inside the batch"
            ));
        }
        refused.push(nonce.to_string());
        filled |= (seen[at + 1..].iter())
            .any(|(_, _, lower, later, _)| *lower < *nonce && *later == Outcome::Added);
    }
    if refused.is_empty() {
        return Err("its pool refused none of the batch".to_owned());
    }
    if !filled {
        return Err("no lower nonce of the batch was added after a refused one".to_owned());
    }
    let order: Vec<String> = (seen.iter())
        .map(|(_, _, nonce, outcome, _)| match outcome {
            Outcome::Added => format!("{nonce} added"),
            Outcome::Refused(_) => format!("{nonce} refused"),
        })
        .collect();
    let nonces: Vec<u64> = seen.iter().map(|(_, _, nonce, ..)| *nonce).collect();
    if longest_fall(&nonces) > 2 {
        return Err(format!(
            "its pool took the batch in an order two posts each in nonce order cannot give ({})",
            order.join(", ")
        ));
    }
    let first_at = (seen.iter())
        .filter_map(|(.., at)| *at)
        .min()
        .ok_or("its pool lines of the batch have no time")?;
    Ok(TakenApart {
        text: format!(
            "its pool took the batch of {sender} (nonces {first} to {}) apart: {} (nonces {} refused for a gap inside the batch that a lower nonce filled after them)",
            first.saturating_add(u64::try_from(ids.len()).unwrap_or(0) - 1),
            order.join(", "),
            refused.join(", ")
        ),
        first_at,
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::vectors::testutil::TestDir;

    const SENDER: &str = "dMbY1nq4CJsHxd7WQKDH4BGQtZQn5q4sxx";

    /// The ids of the batch: five 64-character ids, the nonces 1 to 5.
    fn ids() -> Vec<String> {
        (1..=5)
            .map(|n| format!("{}{n:08x}", "ab".repeat(28)))
            .collect()
    }

    /// The log lines of a reference relay's pool taking the batch's transactions in `order`
    /// (nonces), each `(nonce, refused for a gap from the sender's nonce)`, from 03:42:53.114.
    fn pool_lines(order: &[(u64, Option<u64>)]) -> String {
        let ids = ids();
        let mut text = String::new();
        for (i, (nonce, refused)) in order.iter().enumerate() {
            let at = format!("[2026-10-03 03:42:53.{:03}]", 114 + i);
            let id = &ids[usize::try_from(*nonce).unwrap() - 1];
            let tx = format!("{SENDER}#{nonce} {} Transfer v3", &id[56..]);
            text.push_str(&format!(
                "{at} DEBUG: {tx} eligible to enter pool (fee 0.5 dRT >= 0.0104941 dRT) 💸\n"
            ));
            match refused {
                None => text.push_str(&format!("{at} DEBUG: {tx} added to the pool\n")),
                Some(has) => text.push_str(&format!(
                    "{at} WARNING: {tx} failed to enter the pool: {tx} cannot be applied: Cannot apply a transaction with nonce {nonce}: the sender has nonce {has} ⚠️\n"
                )),
            }
        }
        text
    }

    /// A run of three reference relays and two Heartwood relays whose batch step failed at
    /// node 3, node 3's log `log`, and the chain's blocks holding the batch's transactions at
    /// `heights` (one per transaction; none for a transaction never forged).
    fn race_run(log: &str, heights: &[Option<u64>]) -> (TestDir, Manifest, Step) {
        let dir = TestDir::new("live-batch-race");
        let root = dir.path();
        let node = |n: u32, implementation: &str| {
            json!({"name": format!("node{n}"), "ip": format!("10.97.0.1{n}"),
                "implementation": implementation, "underTest": implementation == "heartwood"})
        };
        let manifest = json!({"format": run::FORMAT, "run": "m1", "network": "n", "seed": "s",
            "status": "complete", "control": "node1",
            "nodes": [node(1, "reference"), node(2, "reference"), node(3, "reference"),
                node(4, "heartwood"), node(5, "heartwood")]});
        std::fs::write(root.join("run.json"), manifest.to_string()).unwrap();
        let seen = |ms: Option<u64>, missing: u64| {
            json!({"seenAllMs": ms, "nextBlockMs": ms.map_or(json!(3084), |_| Value::Null),
                "missing": missing})
        };
        let step = json!({"step": "batch-node4", "node": "node4", "ok": false,
            "start": "2026-10-03T03:42:51.779Z", "end": "2026-10-03T03:42:56.405Z",
            "detail": {"ok": false, "step": "batch", "afterHeight": 161, "http": 200,
                "ids": ids(), "accepted": 5, "relays": {"node1": seen(Some(220), 0),
                "node2": seen(Some(220), 0), "node3": seen(None, 5), "node4": seen(Some(9), 0),
                "node5": seen(Some(9), 0)}}});
        std::fs::write(root.join("steps.jsonl"), format!("{step}\n")).unwrap();
        let logs = root.join("logs/node3");
        std::fs::create_dir_all(&logs).unwrap();
        std::fs::write(logs.join("solar-testnet-relay-current.log"), log).unwrap();
        let archive = root.join("archives/node1/20261003T035000Z");
        std::fs::create_dir_all(archive.join("blocks")).unwrap();
        std::fs::write(archive.join("manifest.json"), "{}").unwrap();
        let mut blocks: BTreeMap<u64, Vec<String>> = BTreeMap::new();
        for (id, height) in ids().into_iter().zip(heights) {
            if let Some(height) = height {
                blocks.entry(*height).or_default().push(id);
            }
        }
        let lines: String = (160..=164)
            .map(|h| {
                let ids = blocks.get(&h).cloned().unwrap_or_default();
                format!("{}\n", json!({"height": h, "txIds": ids}))
            })
            .collect();
        std::fs::write(archive.join("blocks/000000160-000000164.jsonl"), lines).unwrap();
        // The two posts: both Heartwood relays broadcast the whole batch within a millisecond,
        // before node 3's pool took it; nodes 1 and 2 took it from them in nonce order.
        heartwood_log(root, "node4", &broadcast("03:42:53.103", 5));
        heartwood_log(root, "node5", &broadcast("03:42:53.104", 5));
        let in_order = [(1, None), (2, None), (3, None), (4, None), (5, None)];
        for node in ["node1", "node2"] {
            let text = pool_lines(&in_order) + &broadcast("03:42:53.122", 5);
            reference_log(root, node, &text);
        }
        let manifest = Manifest::load(root).unwrap();
        let step = run::steps(root).unwrap().remove(0);
        (dir, manifest, step)
    }

    /// A relay's log line broadcasting `size` transactions at `at` (03:42:53.103).
    fn broadcast(at: &str, size: usize) -> String {
        format!("[2026-10-03 {at}] DEBUG: Broadcasting {size} transactions to 4 peers 💰\n")
    }

    /// Writes `text` as the file log of the reference relay `node` of the run in `root`.
    fn reference_log(root: &Path, node: &str, text: &str) {
        let logs = root.join("logs").join(node);
        std::fs::create_dir_all(&logs).unwrap();
        std::fs::write(logs.join("solar-testnet-relay-current.log"), text).unwrap();
    }

    /// Writes `text` as the file log of the Heartwood relay `node` of the run in `root`.
    fn heartwood_log(root: &Path, node: &str, text: &str) {
        let logs = root.join("nodes").join(node).join("log");
        std::fs::create_dir_all(&logs).unwrap();
        std::fs::write(logs.join("solar-testnet-relay-current.log"), text).unwrap();
    }

    /// The repeat's order at node 3: 3 and 4 refused, 1 added, 5 refused, 2 added.
    const REPEAT: [(u64, Option<u64>); 5] = [
        (3, Some(0)),
        (4, Some(0)),
        (1, None),
        (5, Some(1)),
        (2, None),
    ];
    /// The first run's order at node 2 (here node 3): 3, 1, 4, 2, 5.
    const FIRST: [(u64, Option<u64>); 5] = [
        (3, Some(0)),
        (1, None),
        (4, Some(1)),
        (2, None),
        (5, Some(2)),
    ];

    #[test]
    fn a_batch_a_reference_relay_took_apart_is_its_race() {
        for order in [REPEAT, FIRST] {
            let all = [Some(162); 5];
            let (dir, manifest, step) = race_run(&pool_lines(&order), &all);
            let text = reference_race(dir.path(), &manifest, &step).unwrap();
            assert!(text.starts_with("node3: "), "{text}");
            assert!(text.contains(SENDER), "{text}");
        }
        // The refused transactions forged a block later, as in the first run.
        let later = [Some(162), Some(162), Some(163), Some(163), Some(163)];
        let (dir, manifest, step) = race_run(&pool_lines(&FIRST), &later);
        assert!(reference_race(dir.path(), &manifest, &step).is_ok());
    }

    #[test]
    fn anything_else_keeps_the_step_failed() {
        let all = [Some(162); 5];
        let failed = |log: &str, heights: &[Option<u64>]| {
            let (dir, manifest, step) = race_run(log, heights);
            reference_race(dir.path(), &manifest, &step).is_err()
        };
        // A refused transaction never forged.
        let lost = [Some(162), Some(162), Some(162), None, Some(162)];
        assert!(failed(&pool_lines(&REPEAT), &lost));
        // The batch in nonce order: a refusal no lower nonce filled after it.
        let in_order = [(1, None), (2, None), (3, None), (4, None), (5, Some(3))];
        assert!(failed(&pool_lines(&in_order), &all));
        // A transaction without an outcome in the log, and no log at all.
        let short = [(3, Some(0)), (4, Some(0)), (1, None), (2, None)];
        assert!(failed(&pool_lines(&short), &all));
        assert!(failed("", &all));
        // A refusal for another reason than a gap inside the batch.
        let text = pool_lines(&REPEAT).replacen(
            "Cannot apply a transaction with nonce 3: the sender has nonce 0",
            "Insufficient balance in the wallet",
            1,
        );
        assert!(failed(&text, &all));
        let outside = [
            (3, Some(0)),
            (4, Some(0)),
            (1, None),
            (5, Some(9)),
            (2, None),
        ];
        assert!(failed(&pool_lines(&outside), &all));
        // Another sender, or another nonce than the batch's order gives.
        let other = pool_lines(&REPEAT).replace(
            &format!("{SENDER}#3 "),
            "dOther1nq4CJsHxd7WQKDH4BGQtZQn5q4s#3 ",
        );
        assert!(failed(&other, &all));
        let renumbered =
            pool_lines(&REPEAT).replace(&format!("{SENDER}#2 "), &format!("{SENDER}#6 "));
        assert!(failed(&renumbered, &all));
        // Nonces and senders' nonces at the end of the range read as any other number.
        let huge = pool_lines(&REPEAT)
            .replace(&format!("{SENDER}#3 "), &format!("{SENDER}#{} ", u64::MAX))
            .replace(
                "the sender has nonce 1",
                &format!("the sender has nonce {}", u64::MAX),
            );
        assert!(failed(&huge, &all));
        // A Heartwood relay that did not list the whole batch: never the reference's race.
        let (dir, manifest, mut step) = race_run(&pool_lines(&REPEAT), &all);
        step.record["detail"]["relays"]["node5"] =
            json!({"seenAllMs": null, "nextBlockMs": 3000, "missing": 2});
        assert!(reference_race(dir.path(), &manifest, &step).is_err());
        // A refused post, or one the submitting relay did not accept whole.
        let (dir, manifest, mut step) = race_run(&pool_lines(&REPEAT), &all);
        step.record["detail"]["accepted"] = json!(4);
        assert!(reference_race(dir.path(), &manifest, &step).is_err());
    }

    #[test]
    fn a_race_needs_two_posts_at_once_and_a_relay_that_took_the_batch_in_order() {
        let all = [Some(162); 5];
        let documented = |change: &dyn Fn(&Path), log: &str| {
            let (dir, manifest, step) = race_run(log, &all);
            change(dir.path());
            reference_race(dir.path(), &manifest, &step)
        };
        let repeat = pool_lines(&REPEAT);
        assert!(documented(&|_| {}, &repeat).is_ok());
        // One post alone, whatever its order, writes the same pool lines: without a second
        // relay's broadcast of the batch the step stays failed.
        let one_post = |root: &Path| heartwood_log(root, "node5", "");
        assert!(documented(&one_post, &repeat).is_err());
        // The second broadcast seconds later, or both after the relay's pool took the batch.
        let later = |root: &Path| heartwood_log(root, "node5", &broadcast("03:42:55.104", 5));
        assert!(documented(&later, &repeat).is_err());
        let after = |root: &Path| {
            heartwood_log(root, "node4", &broadcast("03:42:53.120", 5));
            heartwood_log(root, "node5", &broadcast("03:42:53.121", 5));
        };
        assert!(documented(&after, &repeat).is_err());
        // A broadcast of another number of transactions is not a post of the batch.
        let other = |root: &Path| heartwood_log(root, "node5", &broadcast("03:42:53.104", 4));
        assert!(documented(&other, &repeat).is_err());
        // Two posts each in nonce order interleave as two rising runs of nonces: a reversed
        // order is one post out of nonce order, not the race.
        let reversed = [
            (5, Some(0)),
            (4, Some(0)),
            (3, Some(0)),
            (2, Some(0)),
            (1, None),
        ];
        assert!(documented(&|_| {}, &pool_lines(&reversed)).is_err());
        // No other reference relay took the batch whole in nonce order: nothing shows that a
        // post of the batch was in nonce order.
        let no_witness = |root: &Path| {
            for node in ["node1", "node2"] {
                reference_log(root, node, &pool_lines(&REPEAT));
            }
        };
        assert!(documented(&no_witness, &repeat).is_err());
        let no_logs = |root: &Path| {
            for node in ["node1", "node2"] {
                std::fs::remove_dir_all(root.join("logs").join(node)).unwrap();
            }
        };
        assert!(documented(&no_logs, &repeat).is_err());
    }
}
