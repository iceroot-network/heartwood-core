//! Each suite run's own chain, for `scenario compare`: the digests on the common prefix of a
//! suite's two runs. The two runs are two networks that forged their own blocks, so each run's
//! nodes are held to one chain, and the two runs to the prefix they share, their genesis.
//!
//! - A Heartwood run (`nodes/<node>/difftest/digest-NNNN.jsonl`): each node's final chain is the
//!   last `apply`, `reapply` or `genesis` line of each height, a `revert` or `restart` line at a
//!   height dropping the heights above it; at every height two nodes hold, their lines are equal in
//!   every key but the running digest; a `restart` line equals the node's own line of its height
//!   before it in the state's keys.
//! - A reference run (`archives/<node>/<stamp>/blocks/*.jsonl`, the devnet tooling's archives):
//!   at every height two nodes archived, their block ids are equal.
//! - Both runs: each holds one genesis, and it is the same block in both.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::errors::{Error, Result};
use crate::live::digest;
use crate::restart::{SAME_STATE, SAME_STEP, differing_keys};

/// What one run's nodes hold: the heights two or more of them share, their genesis ids, each
/// node's tip and the disagreements.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct RunChain {
    /// The nodes read.
    pub nodes: usize,
    /// The heights two nodes or more hold.
    pub shared: usize,
    /// The block ids the nodes hold at height 1.
    pub genesis: BTreeSet<String>,
    /// What disagrees (the first few).
    pub problems: Vec<String>,
    /// In a Heartwood run, each node's final chain: its last line of each height.
    pub lines: BTreeMap<String, BTreeMap<u32, Value>>,
}

impl RunChain {
    /// The text of a run's one-chain case: `None` when it holds, else what failed.
    pub fn verdict(&self) -> Option<String> {
        if !self.problems.is_empty() {
            return Some(self.problems.join("; "));
        }
        if self.nodes < 2 || self.shared == 0 {
            return Some(format!(
                "{} node(s) and {} shared height(s): nothing to hold to one chain",
                self.nodes, self.shared
            ));
        }
        if self.genesis.len() != 1 {
            return Some(format!("the genesis ids {:?}", self.genesis));
        }
        None
    }
}

/// Keep the first few problems and count the rest.
fn note(problems: &mut Vec<String>, more: &mut usize, problem: String) {
    if problems.len() < 6 {
        problems.push(problem);
    } else {
        *more += 1;
    }
}

/// The directories under `dir` named `node<k>`, sorted.
fn node_dirs(dir: &Path) -> Result<Vec<(String, PathBuf)>> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for entry in entries {
        let path = entry.map_err(|e| Error::io(dir, e))?.path();
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_owned();
        if path.is_dir() && name.starts_with("node") {
            out.push((name, path));
        }
    }
    out.sort();
    Ok(out)
}

/// Hold every node's final chain, by height, to the others'.
fn agree<T: PartialEq>(
    chains: &BTreeMap<String, BTreeMap<u32, T>>,
    differ: impl Fn(&T, &T) -> Option<String>,
    out: &mut RunChain,
    more: &mut usize,
) {
    let heights: BTreeSet<u32> = chains.values().flat_map(|c| c.keys().copied()).collect();
    for h in heights {
        let holders: Vec<(&String, &T)> = chains
            .iter()
            .filter_map(|(node, chain)| chain.get(&h).map(|line| (node, line)))
            .collect();
        let Some(((first_node, first), rest)) = holders.split_first() else {
            continue;
        };
        if rest.is_empty() {
            continue;
        }
        out.shared += 1;
        for (node, line) in rest {
            if let Some(what) = differ(first, line) {
                note(
                    &mut out.problems,
                    more,
                    format!("height {h}: {node} differs from {first_node} in {what}"),
                );
            }
        }
    }
}

/// The one chain of a Heartwood run's nodes, from their digest files.
pub fn heartwood(run: &Path) -> Result<RunChain> {
    let mut out = RunChain::default();
    let mut more = 0usize;
    let mut chains: BTreeMap<String, BTreeMap<u32, Value>> = BTreeMap::new();
    for (node, dir) in node_dirs(&run.join("nodes"))? {
        let dir = dir.join("difftest");
        if !dir.is_dir() {
            continue;
        }
        let read = digest::read(&dir)?;
        let mut chain: BTreeMap<u32, Value> = BTreeMap::new();
        for line in &read.lines {
            let h = line.height();
            match line.step() {
                "apply" | "reapply" | "genesis" => {
                    chain.insert(h, line.value.clone());
                }
                step @ ("revert" | "restart") => {
                    if step == "restart"
                        && let Some(own) = chain.get(&h)
                    {
                        let differing = differing_keys(&line.value, own, &SAME_STATE);
                        if !differing.is_empty() {
                            note(
                                &mut out.problems,
                                &mut more,
                                format!(
                                    "{node}: the restart line of height {h} (start {}) differs from its line before the restart in {}",
                                    line.start,
                                    differing.join(", ")
                                ),
                            );
                        }
                    }
                    chain.retain(|height, _| *height <= h);
                }
                other => note(
                    &mut out.problems,
                    &mut more,
                    format!("{node}: unknown step {other:?} at height {h}"),
                ),
            }
        }
        if let Some(genesis) = chain
            .get(&1)
            .and_then(|l| l.get("id"))
            .and_then(Value::as_str)
        {
            out.genesis.insert(genesis.to_owned());
        }
        chains.insert(node, chain);
    }
    out.nodes = chains.len();
    // The keys every node's line of one block shares (the running digest is each node's own).
    let keys: Vec<&str> = SAME_STEP.iter().copied().filter(|k| *k != "h").collect();
    agree(
        &chains,
        |a, b| {
            let differing = differing_keys(a, b, &keys);
            (!differing.is_empty()).then(|| differing.join(", "))
        },
        &mut out,
        &mut more,
    );
    if more > 0 {
        out.problems.push(format!("and {more} more"));
    }
    out.lines = chains;
    Ok(out)
}

/// The one chain of a reference run's nodes, from the block ids of their archives.
pub fn reference(run: &Path) -> Result<RunChain> {
    let mut out = RunChain::default();
    let mut more = 0usize;
    let mut chains: BTreeMap<String, BTreeMap<u32, String>> = BTreeMap::new();
    for (node, dir) in node_dirs(&run.join("archives"))? {
        let mut chain: BTreeMap<u32, String> = BTreeMap::new();
        // Each archive is a stamped directory; a later one holds the chain as it was later.
        for (_, stamp) in stamps(&dir)? {
            let blocks = stamp.join("blocks");
            let Ok(files) = std::fs::read_dir(&blocks) else {
                continue;
            };
            let mut files: Vec<PathBuf> = files.filter_map(|e| e.ok().map(|e| e.path())).collect();
            files.retain(|p| p.extension().is_some_and(|e| e == "jsonl"));
            files.sort();
            for file in files {
                let text = std::fs::read_to_string(&file).map_err(|e| Error::io(&file, e))?;
                for (index, line) in text.lines().enumerate() {
                    if line.trim().is_empty() {
                        continue;
                    }
                    let value: Value = serde_json::from_str(line).map_err(|e| Error::Parse {
                        path: file.clone(),
                        line: index + 1,
                        message: e.to_string(),
                    })?;
                    let height = value
                        .get("height")
                        .and_then(Value::as_u64)
                        .and_then(|h| u32::try_from(h).ok());
                    let id = value.get("id").and_then(Value::as_str);
                    match (height, id) {
                        (Some(h), Some(id)) => {
                            chain.insert(h, id.to_owned());
                        }
                        _ => {
                            return Err(Error::Parse {
                                path: file.clone(),
                                line: index + 1,
                                message: "an archived block without its height or id".into(),
                            });
                        }
                    }
                }
            }
        }
        if chain.is_empty() {
            continue;
        }
        if let Some(genesis) = chain.get(&1) {
            out.genesis.insert(genesis.clone());
        }
        chains.insert(node, chain);
    }
    out.nodes = chains.len();
    agree(
        &chains,
        |a, b| (a != b).then(|| format!("the block id ({a} against {b})")),
        &mut out,
        &mut more,
    );
    if more > 0 {
        out.problems.push(format!("and {more} more"));
    }
    Ok(out)
}

/// The stamped archive directories of a node's archives, in order.
fn stamps(dir: &Path) -> Result<Vec<(String, PathBuf)>> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).map_err(|e| Error::io(dir, e))? {
        let path = entry.map_err(|e| Error::io(dir, e))?.path();
        if path.is_dir() {
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default()
                .to_owned();
            out.push((name, path));
        }
    }
    out.sort();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::ScratchDir;

    fn line(h: u32, step: &str, id: &str, b: &str) -> String {
        format!(
            "{{\"h\":{h},\"step\":\"{step}\",\"id\":\"{id}\",\"rw\":\"rw\",\"b\":\"{b}\",\"s\":\"s\",\"r\":\"r\",\"m\":\"m\",\"w\":\"w\",\"sd\":\"sd\",\"cd\":\"{step}{h}\"}}\n"
        )
    }

    fn digest(run: &Path, node: &str, start: u32, lines: &[String]) {
        let dir = run.join("nodes").join(node).join("difftest");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("digest-{start:04}.jsonl")), lines.concat()).unwrap();
    }

    #[test]
    fn heartwood_nodes_hold_one_final_chain_after_a_fork_and_a_restart() {
        let scratch = ScratchDir::new("run-chain").unwrap();
        let run = scratch.path();
        digest(
            run,
            "node1",
            1,
            &[line(1, "genesis", "g", "b1"), line(2, "apply", "a2", "b2")],
        );
        digest(run, "node1", 2, &[line(2, "restart", "a2", "b2")]);
        // Node 2 forged its own block 2, reverted it and applied the other side's.
        digest(
            run,
            "node2",
            1,
            &[
                line(1, "genesis", "g", "b1"),
                line(2, "apply", "x2", "bx"),
                line(1, "revert", "g", "b1"),
                line(2, "reapply", "a2", "b2"),
            ],
        );
        let chain = heartwood(run).unwrap();
        assert_eq!(chain.verdict(), None, "{chain:?}");
        assert_eq!((chain.nodes, chain.shared), (2, 2));

        // A restart line that is not the state it restored, and a node that ends on another block.
        digest(
            run,
            "node3",
            1,
            &[line(1, "genesis", "g", "b1"), line(2, "apply", "y2", "by")],
        );
        digest(run, "node1", 2, &[line(2, "restart", "a2", "other")]);
        let chain = heartwood(run).unwrap();
        let text = chain.verdict().expect("two problems");
        assert!(
            text.contains("node1: the restart line of height 2"),
            "{text}"
        );
        assert!(
            text.contains("height 2: node3 differs from node1 in id, b"),
            "{text}"
        );
    }

    #[test]
    fn reference_archives_hold_one_chain_and_one_genesis() {
        let scratch = ScratchDir::new("run-chain-reference").unwrap();
        let run = scratch.path();
        let archive = |node: &str, stamp: &str, lines: &str| {
            let dir = run.join("archives").join(node).join(stamp).join("blocks");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("000000001-000000002.jsonl"), lines).unwrap();
        };
        archive(
            "node1",
            "1",
            "{\"height\":1,\"id\":\"g\"}\n{\"height\":2,\"id\":\"a\"}\n",
        );
        archive(
            "node2",
            "1",
            "{\"height\":1,\"id\":\"g\"}\n{\"height\":2,\"id\":\"b\"}\n",
        );
        // A later archive of node 2 holds the chain it ended on.
        archive(
            "node2",
            "2",
            "{\"height\":1,\"id\":\"g\"}\n{\"height\":2,\"id\":\"a\"}\n",
        );
        let chain = reference(run).unwrap();
        assert_eq!(chain.verdict(), None, "{chain:?}");
        assert_eq!(chain.genesis.len(), 1);
        archive(
            "node3",
            "1",
            "{\"height\":1,\"id\":\"g\"}\n{\"height\":2,\"id\":\"c\"}\n",
        );
        let text = reference(run).unwrap().verdict().expect("a fork");
        assert!(
            text.contains("height 2: node3 differs from node1"),
            "{text}"
        );
        let empty = ScratchDir::new("run-chain-empty").unwrap();
        assert!(reference(empty.path()).unwrap().verdict().is_some());
    }
}
