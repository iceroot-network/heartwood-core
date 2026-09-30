//! The resources of a process tree, read from a `/proc` tree: resident and proportional memory,
//! CPU time and written bytes, summed over a process and all its descendants.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use crate::devnet::Resources;

/// Milliseconds per clock tick: the kernel counts CPU time in ticks of 1/100 s.
const MS_PER_TICK: u64 = 10;

/// What one process's `stat` says: its parent and the ticks it and its reaped children used.
struct Stat {
    parent: u32,
    ticks: u64,
}

/// The resources of the tree under `pid`, read from the `/proc` tree at `proc`: the memory and
/// written bytes of the process and every descendant, and their CPU time, the time of children
/// already reaped included. `None` when `pid` is not there. `pss` and `written` are `None` when
/// no process of the tree lets them be read; a process that exits during the read is skipped.
pub fn tree_resources(proc: &Path, pid: u32) -> Option<Resources> {
    let mut children: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    let mut stats = BTreeMap::new();
    for entry in fs::read_dir(proc).ok()?.flatten() {
        let name = entry.file_name();
        let Some(other) = name.to_str().and_then(|name| name.parse::<u32>().ok()) else {
            continue;
        };
        if let Some(stat) = stat(proc, other) {
            children.entry(stat.parent).or_default().push(other);
            stats.insert(other, stat);
        }
    }
    stats.get(&pid)?;
    let mut tree = BTreeSet::from([pid]);
    let mut next = vec![pid];
    while let Some(parent) = next.pop() {
        for child in children.get(&parent).into_iter().flatten() {
            if tree.insert(*child) {
                next.push(*child);
            }
        }
    }
    let mut used = Resources::default();
    for member in tree {
        let file = |name: &str| fs::read_to_string(proc.join(member.to_string()).join(name)).ok();
        let kib = |text: Option<String>, key: &str| field(text?.as_str(), key)?.checked_mul(1024);
        let rss = kib(file("status"), "VmRSS:");
        let pss = kib(file("smaps_rollup"), "Pss:");
        let written = file("io").and_then(|text| field(&text, "write_bytes:"));
        let ticks = stats.get(&member).map_or(0, |stat| stat.ticks);
        used.rss = used.rss.saturating_add(rss.unwrap_or(0));
        used.pss = sum(used.pss, pss);
        used.cpu_ms = used
            .cpu_ms
            .saturating_add(ticks.saturating_mul(MS_PER_TICK));
        used.written = sum(used.written, written);
    }
    Some(used)
}

/// `total` plus `part` when there is one; `total` alone otherwise, `None` while nothing was read.
fn sum(total: Option<u64>, part: Option<u64>) -> Option<u64> {
    match (total, part) {
        (Some(total), Some(part)) => Some(total.saturating_add(part)),
        (total, part) => total.or(part),
    }
}

/// The number after `key` at the start of a line of `text`.
fn field(text: &str, key: &str) -> Option<u64> {
    let line = text.lines().find_map(|line| line.strip_prefix(key))?;
    line.split_whitespace().next()?.parse().ok()
}

/// The parent and CPU ticks (user, system, and the same of reaped children) of process `pid`.
fn stat(proc: &Path, pid: u32) -> Option<Stat> {
    let text = fs::read_to_string(proc.join(pid.to_string()).join("stat")).ok()?;
    // The command name comes first, in parentheses, and may hold spaces and parentheses.
    let rest = text.get(text.rfind(") ")?.checked_add(2)?..)?;
    let fields: Vec<&str> = rest.split_whitespace().collect();
    let number = |i: usize| fields.get(i)?.parse::<u64>().ok();
    let ticks = (11..=14).try_fold(0_u64, |sum, i| Some(sum.saturating_add(number(i)?)))?;
    Some(Stat {
        parent: u32::try_from(number(1)?).ok()?,
        ticks,
    })
}
