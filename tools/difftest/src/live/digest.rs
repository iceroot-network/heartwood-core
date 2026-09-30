//! A Heartwood relay's digest lines against the judge's lines of the run's chains.
//!
//! The relay writes one line per applied, reverted and restarted state (`--difftest-digest`), one
//! file per start (`digest-NNNN.jsonl`), each line whole; a kill leaves at most a torn last line in
//! the file of that start, which is dropped and reported. Every line is judged against the chain
//! that holds its block (the line's `id` at its height): the majority chain, or in a partition the
//! minority's chain as its node archived it before the heal.
//!
//! - an `apply` line equals the judge's forward line of its height in every key but the running
//!   digest (`h`, `id`, `rw`, `b`, `s`, `r`, `m`, `w`, `sd`); so does a `reapply` line, which the
//!   relay writes for a block at or below a height it applied before in the same start (the blocks
//!   of the other side after a fork switch, or blocks it applies again after a revert);
//! - a `revert`, `restart` or `genesis` line (the state the node returned to, restored or started
//!   from) equals it in `b`, `s`, `r` and `w`;
//! - every start opens with the state the relay started from (a `restart` or `genesis` line),
//!   except a first start on an empty store, which may open with the apply line of height 2;
//! - the last `apply` or `reapply` line of every height of the final chain carries that chain's
//!   block, and the node's last line is the final tip.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::errors::{Error, Result};
use crate::restart::{SAME_STATE, SAME_STEP, differing_keys};

/// One line of a node's digest files.
#[derive(Debug, Clone, PartialEq)]
pub struct NodeLine {
    /// The start of the relay the line belongs to (from 1).
    pub start: u32,
    /// The line's position in its file, from 1.
    pub line: usize,
    /// The line.
    pub value: Value,
}

impl NodeLine {
    /// The line's height.
    pub fn height(&self) -> u32 {
        self.value
            .get("h")
            .and_then(Value::as_u64)
            .and_then(|h| u32::try_from(h).ok())
            .unwrap_or(0)
    }

    /// The line's step.
    pub fn step(&self) -> &str {
        self.value.get("step").and_then(Value::as_str).unwrap_or("")
    }

    /// The line's block id.
    pub fn id(&self) -> &str {
        self.value.get("id").and_then(Value::as_str).unwrap_or("")
    }
}

/// A node's digest files, read.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NodeDigest {
    /// Every whole line, in order of the starts and of the lines.
    pub lines: Vec<NodeLine>,
    /// The torn last lines that were dropped: the start and the text.
    pub torn: Vec<(u32, String)>,
    /// The digest files, in order.
    pub files: Vec<PathBuf>,
}

/// The number of the start of a digest file `digest-NNNN.jsonl`.
fn start_of(path: &Path) -> Option<u32> {
    path.file_name()?
        .to_str()?
        .strip_prefix("digest-")?
        .strip_suffix(".jsonl")?
        .parse()
        .ok()
}

/// Read the digest files in `dir` (a node's `difftest/`). A last line without its newline, or one
/// that is not a JSON object, is torn: it is dropped and listed. Any other line that is not a JSON
/// object is an error.
pub fn read(dir: &Path) -> Result<NodeDigest> {
    let mut files: Vec<(u32, PathBuf)> = std::fs::read_dir(dir)
        .map_err(|e| Error::io(dir, e))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter_map(|p| start_of(&p).map(|n| (n, p)))
        .collect();
    files.sort();
    let mut out = NodeDigest::default();
    for (start, path) in files {
        let bytes = std::fs::read(&path).map_err(|e| Error::io(&path, e))?;
        let text = String::from_utf8_lossy(&bytes);
        let whole = text.ends_with('\n');
        let parts: Vec<&str> = text.split_terminator('\n').collect();
        for (index, line) in parts.iter().enumerate() {
            let last = index + 1 == parts.len();
            let parsed = serde_json::from_str::<Value>(line)
                .ok()
                .filter(Value::is_object);
            match parsed {
                Some(value) if !(last && !whole) => out.lines.push(NodeLine {
                    start,
                    line: index + 1,
                    value,
                }),
                _ if last => out.torn.push((start, (*line).to_owned())),
                _ if line.trim().is_empty() => {}
                _ => {
                    return Err(Error::Parse {
                        path: path.clone(),
                        line: index + 1,
                        message: "a digest line that is not a JSON object, before the last line"
                            .into(),
                    });
                }
            }
        }
        out.files.push(path);
    }
    Ok(out)
}

/// One start of a relay, from its digest file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Start {
    /// The start's number (from 1).
    pub number: u32,
    /// The tip of the store it opened: its `restart` line's height, else 1 (a start on an empty
    /// store, which opens with the genesis line or the apply line of height 2).
    pub tip: u32,
    /// The highest height its lines name.
    pub reached: u32,
}

/// The starts of `digest`, in order.
pub fn starts_of(digest: &NodeDigest) -> Vec<Start> {
    let mut out: Vec<Start> = Vec::new();
    for line in &digest.lines {
        match out.last_mut() {
            Some(start) if start.number == line.start => {
                start.reached = start.reached.max(line.height());
            }
            _ => out.push(Start {
                number: line.start,
                tip: if line.step() == "restart" {
                    line.height()
                } else {
                    1
                },
                reached: line.height(),
            }),
        }
    }
    out
}

/// A judged chain: its name and the judge's forward line (genesis and apply) of each height.
#[derive(Debug, Clone, PartialEq)]
pub struct JudgedChain {
    /// The corpus's name.
    pub name: String,
    /// The forward lines by height.
    pub forward: BTreeMap<u32, Value>,
}

impl JudgedChain {
    /// The chain's tip.
    pub fn tip(&self) -> u32 {
        self.forward.keys().next_back().copied().unwrap_or(0)
    }

    /// Whether the chain's block at `height` is `id`.
    pub fn holds(&self, height: u32, id: &str) -> bool {
        self.forward
            .get(&height)
            .and_then(|l| l.get("id"))
            .and_then(Value::as_str)
            == Some(id)
    }
}

/// What a comparison of a node's lines found.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Judged {
    /// Lines compared, per chain and step.
    pub compared: BTreeMap<(String, String), usize>,
    /// The problems, each with the line's place.
    pub problems: Vec<String>,
    /// The heights of the final chain whose last apply (or reapply) line is missing or another
    /// block's.
    pub uncovered: Vec<u32>,
    /// The number of starts whose first line is a restart line (every start but a first one
    /// from an empty store must begin with one).
    pub restarts: usize,
}

/// Compare `digest` with the judged `chains`; `chains[0]` is the final chain.
pub fn judge(digest: &NodeDigest, chains: &[JudgedChain]) -> Judged {
    let mut out = Judged::default();
    let place = |l: &NodeLine| format!("start {} line {}", l.start, l.line);
    for l in &digest.lines {
        let (h, id, step) = (l.height(), l.id(), l.step());
        let Some(chain) = chains.iter().find(|c| c.holds(h, id)) else {
            out.problems.push(format!(
                "{}: {step} to {h} with block {id}, which no judged chain holds at that height",
                place(l)
            ));
            continue;
        };
        let want = &chain.forward[&h];
        let keys: &[&str] = match step {
            "apply" | "reapply" => &SAME_STEP,
            "revert" | "restart" | "genesis" => &SAME_STATE,
            other => {
                out.problems
                    .push(format!("{}: unknown step {other:?}", place(l)));
                continue;
            }
        };
        let differing = differing_keys(&l.value, want, keys);
        if differing.is_empty() {
            *out.compared
                .entry((chain.name.clone(), step.to_owned()))
                .or_default() += 1;
        } else {
            out.problems.push(format!(
                "{}: the {step} line of height {h} differs from {}'s in {}",
                place(l),
                chain.name,
                differing.join(", ")
            ));
        }
    }
    let mut starts: BTreeMap<u32, &NodeLine> = BTreeMap::new();
    for l in &digest.lines {
        starts.entry(l.start).or_insert(l);
    }
    out.restarts = starts.values().filter(|l| l.step() == "restart").count();
    for (start, first) in &starts {
        // A relay that starts on an empty store may have no state to report before its first
        // block: its first file can open with the apply line of height 2.
        let fresh = first.start == 1 && first.step() == "apply" && first.height() == 2;
        if !matches!(first.step(), "restart" | "genesis") && !fresh {
            out.problems.push(format!(
                "start {start}: its first line is a {} line, not the state it started from",
                first.step()
            ));
        }
    }
    if let Some(last) = chains.first() {
        let mut last_apply: BTreeMap<u32, &str> = BTreeMap::new();
        for l in digest
            .lines
            .iter()
            .filter(|l| matches!(l.step(), "apply" | "reapply"))
        {
            last_apply.insert(l.height(), l.id());
        }
        let first = digest.lines.first().map_or(2, |l| l.height().max(1) + 1);
        for h in first.max(2)..=last.tip() {
            if !last_apply.get(&h).is_some_and(|id| last.holds(h, id)) {
                out.uncovered.push(h);
            }
        }
        match digest.lines.last() {
            Some(l) if l.height() == last.tip() && last.holds(l.height(), l.id()) => {}
            Some(l) => out.problems.push(format!(
                "the node's last line is at height {} (block {}), not at the final tip {}",
                l.height(),
                l.id(),
                last.tip()
            )),
            None => out.problems.push("the node wrote no digest line".into()),
        }
    }
    out
}

/// The scripted faults after which a relay starts again (`faults.jsonl`): a kill (`sig` and the
/// signal's name), a stop and start, a restart with the replay marker; and in a forging devnet the
/// fresh sync after a reset, the relay stopped or killed for a failover, the supervised relay
/// crash, and the relays stopped for the two outages (`outage-three` names the one node it stops);
/// in the mixed runs a relay held and started again (`hold`) and the one an offline rollback
/// stopped (`rollback`).
pub const RESTARTING_FAULTS: [&str; 13] = [
    "sigint",
    "sigkill",
    "sigterm",
    "restart",
    "marker-restart",
    "fresh-sync",
    "relay-stop",
    "relay-kill",
    "supervised-relay",
    "outage-two",
    "outage-three",
    "hold",
    "rollback",
];

/// The node's starts (its digest files) against the scripted restarts of it, of `faults` (each a
/// fault's name and the nodes it concerns): one start, and one more after each of
/// [`RESTARTING_FAULTS`]. A start more is a restart nobody scripted (the relay stopped by itself);
/// a start less, a scripted restart that did not happen.
pub fn starts(
    digest: &NodeDigest,
    node: &str,
    faults: &[(String, Vec<String>)],
) -> std::result::Result<String, String> {
    let scripted = faults
        .iter()
        .filter(|(fault, nodes)| {
            RESTARTING_FAULTS.contains(&fault.as_str()) && nodes.iter().any(|n| n == node)
        })
        .count();
    let (want, have) = (scripted + 1, digest.files.len());
    if want == have && scripted == 0 {
        Ok("1 start, as scripted".to_owned())
    } else if want == have {
        Ok(format!(
            "{have} starts: the first and one after each of its {scripted} scripted restarts"
        ))
    } else {
        Err(format!(
            "{have} starts (digest files), but its scripted restarts make {want}"
        ))
    }
}

/// A kill during the initial sync: the node's first start, on an empty store, ends (its last whole
/// line) above genesis and below `target`, the height node 1 held when the node started, so the
/// kill landed after the sync began and before the node caught up with the chain; and the next
/// start opens with the state it restored (a `restart` line).
pub fn killed_during_sync(digest: &NodeDigest, target: u32) -> std::result::Result<String, String> {
    let first = |start: u32| digest.lines.iter().find(|l| l.start == start);
    let last = digest.lines.iter().rfind(|l| l.start == 1);
    let Some(last) = last else {
        return Err("its first start wrote no whole line".into());
    };
    if last.height() >= target {
        return Err(format!(
            "its first start reached height {} before the kill, not below node 1's height {target} at its start: the sync had ended",
            last.height()
        ));
    }
    if last.height() < 2 {
        return Err(format!(
            "its first start wrote no block (its last line is at height {}): the kill came before the sync began",
            last.height()
        ));
    }
    match first(2) {
        Some(l) if l.step() == "restart" => Ok(format!(
            "the kill ended its first start at height {} of {target} (node 1's height at its start); the next start restored height {}",
            last.height(),
            l.height()
        )),
        Some(l) => Err(format!(
            "the start after the kill opens with a line of step {}, not a restart line",
            l.step()
        )),
        None => Err("no start after the kill".into()),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::vectors::testutil::TestDir;

    fn digest_of(starts: &[&[Value]]) -> NodeDigest {
        let mut out = NodeDigest::default();
        for (s, lines) in starts.iter().enumerate() {
            let start = u32::try_from(s + 1).unwrap();
            out.files
                .push(PathBuf::from(format!("digest-{start:04}.jsonl")));
            out.lines
                .extend(lines.iter().enumerate().map(|(i, v)| NodeLine {
                    start,
                    line: i + 1,
                    value: v.clone(),
                }));
        }
        out
    }

    #[test]
    fn the_starts_follow_the_scripted_restarts() {
        let genesis = {
            let mut l = line(1, "g", "w1");
            l["step"] = json!("genesis");
            l
        };
        let restart = {
            let mut l = line(2, "a2", "w2");
            l["step"] = json!("restart");
            l
        };
        let two = digest_of(&[&[genesis.clone(), line(2, "a2", "w2")], &[restart]]);
        let fault = |name: &str, node: &str| (name.to_owned(), vec![node.to_owned()]);
        let faults = [
            fault("sigkill", "node5"),
            fault("sigkill", "node4"),
            fault("forger-stop", "node5"),
            fault("collect-stop", "node5"),
        ];
        assert!(starts(&two, "node5", &faults).is_ok());
        // Node 4's kill is not node 5's; a start without a scripted restart is a crash.
        let err = starts(&two, "node5", &faults[1..]).unwrap_err();
        assert!(
            err.contains("2 starts (digest files), but its scripted restarts make 1"),
            "{err}"
        );
        assert!(starts(&digest_of(&[&[genesis]]), "node5", &faults).is_err());
    }

    #[test]
    fn a_kill_during_the_initial_sync_ends_the_first_start_below_the_tip() {
        let mut genesis = line(1, "g", "w1");
        genesis["step"] = json!("genesis");
        let mut restart = line(1, "g", "w1");
        restart["step"] = json!("restart");
        let first: Vec<Value> = std::iter::once(genesis)
            .chain((2..=45).map(|h| line(h, &format!("a{h}"), "w")))
            .collect();
        let then: Vec<Value> = std::iter::once(restart.clone())
            .chain((2..=107).map(|h| line(h, &format!("a{h}"), "w")))
            .collect();
        let killed = digest_of(&[&first, &then]);
        let ok = killed_during_sync(&killed, 107).unwrap();
        assert!(ok.contains("at height 45 of 107"), "{ok}");
        // A kill after the node reached node 1's height is not one during the sync.
        let err = killed_during_sync(&killed, 45).unwrap_err();
        assert!(err.contains("the sync had ended"), "{err}");
        // The next start must open with the state it restored.
        let mut apply_first = then.clone();
        apply_first.remove(0);
        let err = killed_during_sync(&digest_of(&[&first, &apply_first]), 107).unwrap_err();
        assert!(err.contains("a line of step apply"), "{err}");
        assert!(killed_during_sync(&digest_of(&[&first]), 107).is_err());
        // A kill before the first block was applied is not one during the sync either.
        let before = digest_of(&[&first[..1], &then]);
        let err = killed_during_sync(&before, 107).unwrap_err();
        assert!(err.contains("before the sync began"), "{err}");
    }

    fn line(h: u32, id: &str, w: &str) -> Value {
        json!({"h": h, "step": "apply", "id": id, "rw": "1", "b": "b", "s": "s", "r": "r", "m": "m", "w": w, "sd": "sd", "cd": "x"})
    }

    fn chain(name: &str, ids: &[&str]) -> JudgedChain {
        let mut forward = BTreeMap::new();
        for (i, id) in ids.iter().enumerate() {
            let h = u32::try_from(i + 1).unwrap();
            let mut l = line(h, id, &format!("w{h}"));
            if h == 1 {
                l["step"] = json!("genesis");
            }
            forward.insert(h, l);
        }
        JudgedChain {
            name: name.into(),
            forward,
        }
    }

    #[test]
    fn a_torn_last_line_is_dropped_and_listed() {
        let dir = TestDir::new("live-digest-torn");
        let mut a = line(1, "g", "w1");
        a["step"] = json!("genesis");
        std::fs::write(
            dir.path().join("digest-0001.jsonl"),
            format!("{a}\n{}\n{{\"h\":3,\"st", line(2, "a2", "w2")),
        )
        .unwrap();
        let mut r = line(2, "a2", "w2");
        r["step"] = json!("restart");
        std::fs::write(
            dir.path().join("digest-0002.jsonl"),
            format!("{r}\n{}\n", line(3, "a3", "w3")),
        )
        .unwrap();
        let digest = read(dir.path()).unwrap();
        assert_eq!(digest.lines.len(), 4);
        assert_eq!(digest.torn, vec![(1, "{\"h\":3,\"st".to_owned())]);
        let judged = judge(&digest, &[chain("live", &["g", "a2", "a3"])]);
        assert!(judged.problems.is_empty(), "{:?}", judged.problems);
        assert!(judged.uncovered.is_empty(), "{:?}", judged.uncovered);
        assert_eq!(judged.restarts, 1);
        // A whole line that is not the last one must parse.
        std::fs::write(dir.path().join("digest-0003.jsonl"), "{bad\n{}\n").unwrap();
        assert!(read(dir.path()).is_err());
    }

    #[test]
    fn lines_are_judged_by_the_chain_that_holds_their_block() {
        let majority = chain("live", &["g", "a2", "a3", "a4"]);
        let minority = chain("minority", &["g", "a2", "b3"]);
        let mut revert = line(2, "a2", "w2");
        revert["step"] = json!("revert");
        revert["m"] = json!("other");
        let mut genesis = line(1, "g", "w1");
        genesis["step"] = json!("genesis");
        let lines = [
            genesis,
            line(2, "a2", "w2"),
            line(3, "b3", "w3"),
            revert,
            line(3, "a3", "w3"),
            line(4, "a4", "w4"),
        ];
        let digest = NodeDigest {
            lines: lines
                .iter()
                .enumerate()
                .map(|(i, v)| NodeLine {
                    start: 1,
                    line: i + 1,
                    value: v.clone(),
                })
                .collect(),
            ..NodeDigest::default()
        };
        let judged = judge(&digest, &[majority.clone(), minority.clone()]);
        assert!(judged.problems.is_empty(), "{:?}", judged.problems);
        assert_eq!(
            judged.compared.get(&("minority".into(), "apply".into())),
            Some(&1)
        );
        // Without the minority's chain, its block is nobody's; a wrong wallet hash is found.
        let mut bad = digest.clone();
        bad.lines[5].value["w"] = json!("wrong");
        let judged = judge(&bad, &[majority]);
        assert_eq!(judged.problems.len(), 2, "{:?}", judged.problems);
        assert!(judged.problems[0].contains("no judged chain"));
        assert!(judged.problems[1].contains("differs from live's in w"));
        // A node that stops below the final tip.
        let mut short = digest.clone();
        short.lines.truncate(5);
        let judged = judge(&short, &[chain("live", &["g", "a2", "a3", "a4"])]);
        assert_eq!(judged.uncovered, vec![4]);
        assert!(
            judged
                .problems
                .iter()
                .any(|p| p.contains("not at the final tip 4")),
            "{:?}",
            judged.problems
        );
    }

    #[test]
    fn the_blocks_of_a_fork_switch_are_reapply_lines() {
        // The relay applied the minority's 3 and 4, went back to 2 and took the majority's 3 and 4
        // (`reapply`: it had applied those heights before), then 5.
        let majority = chain("live", &["g", "a2", "a3", "a4", "a5"]);
        let minority = chain("minority", &["g", "a2", "b3", "b4"]);
        let step = |mut l: Value, step: &str| {
            l["step"] = json!(step);
            l
        };
        let lines = [
            step(line(1, "g", "w1"), "genesis"),
            line(2, "a2", "w2"),
            line(3, "b3", "w3"),
            line(4, "b4", "w4"),
            step(line(3, "b3", "w3"), "revert"),
            step(line(2, "a2", "w2"), "revert"),
            step(line(3, "a3", "w3"), "reapply"),
            step(line(4, "a4", "w4"), "reapply"),
            line(5, "a5", "w5"),
        ];
        let digest = NodeDigest {
            lines: lines
                .iter()
                .enumerate()
                .map(|(i, v)| NodeLine {
                    start: 1,
                    line: i + 1,
                    value: v.clone(),
                })
                .collect(),
            ..NodeDigest::default()
        };
        let judged = judge(&digest, &[majority.clone(), minority]);
        assert!(judged.problems.is_empty(), "{:?}", judged.problems);
        assert!(judged.uncovered.is_empty(), "{:?}", judged.uncovered);
        assert_eq!(
            judged.compared.get(&("live".into(), "reapply".into())),
            Some(&2)
        );
        // A reapply line is compared as an apply line is.
        let mut bad = digest.clone();
        bad.lines[7].value["w"] = json!("wrong");
        let judged = judge(&bad, std::slice::from_ref(&majority));
        assert!(
            judged
                .problems
                .iter()
                .any(|p| p.contains("the reapply line of height 4 differs from live's in w")),
            "{:?}",
            judged.problems
        );
        // Any other step is still unknown.
        let mut odd = digest;
        odd.lines[7].value["step"] = json!("replay");
        let judged = judge(&odd, &[majority]);
        assert!(
            judged
                .problems
                .iter()
                .any(|p| p.contains("unknown step \"replay\"")),
            "{:?}",
            judged.problems
        );
        assert_eq!(judged.uncovered, vec![4]);
    }
}
