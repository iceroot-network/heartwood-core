//! A Heartwood relay's digest lines against the judge's lines of the run's chains.
//!
//! The relay writes one line per applied, reverted and restarted state (`--difftest-digest`), one
//! file per start (`digest-NNNN.jsonl`), each line whole; a kill leaves at most a torn last line in
//! the file of that start, which is dropped and reported. Every line is judged against the chain
//! that holds its block (the line's `id` at its height): the majority chain, or in a partition the
//! minority's chain as its node archived it before the heal.
//!
//! - an `apply` line equals the judge's forward line of its height in every key but the running
//!   digest (`h`, `id`, `rw`, `b`, `s`, `r`, `m`, `w`, `sd`);
//! - a `revert`, `restart` or `genesis` line (the state the node returned to, restored or started
//!   from) equals it in `b`, `s`, `r` and `w`;
//! - every start opens with the state the relay started from (a `restart` or `genesis` line),
//!   except a first start on an empty store, which may open with the apply line of height 2;
//! - the last `apply` line of every height of the final chain carries that chain's block, and the
//!   node's last line is the final tip.

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
    /// The heights of the final chain whose last apply line is missing or another block's.
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
            "apply" => &SAME_STEP,
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
        for l in digest.lines.iter().filter(|l| l.step() == "apply") {
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

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::vectors::testutil::TestDir;

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
}
