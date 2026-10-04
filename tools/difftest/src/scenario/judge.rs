//! A suite run's chain judged against the reference, for `scenario compare`. Each run's control
//! node's chain (a Heartwood node's block files, or a reference node's archive) is imported as a
//! corpus and judged by the oracle with its processor, as `live check` judges a forging run's
//! chain: the judge's anchors must hold, and the reference's own processing must accept every
//! block (each generator at its slot, each block's economics). Then:
//!
//! - in a Heartwood run, every Heartwood node's final chain (its last line of each height, from its
//!   digest files) is held to the judge's line of that height, block and state;
//! - in a reference run, the chain is replayed through Heartwood's store (L2) against the judge's
//!   lines at every step.
//!
//! So both implementations' chains of a suite are judged against the reference's processing, not
//! only held to themselves.

use std::path::Path;

use serde_json::Value;

use super::run_chain;
use crate::corpus::{Corpus, TS_PROCESSOR};
use crate::errors::Result;
use crate::levels::l2::{self, L2Options};
use crate::live::corpora::{self, Judged};
use crate::live::run::{self, Implementation, Manifest};
use crate::oracle::Oracle;
use crate::report::Outcome;
use crate::restart::{SAME_STATE, SAME_STEP, differing_keys};

/// The problems shown per case.
const SHOWN: usize = 4;

/// One side of a suite: the run's implementation, as its directory names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    /// A run on the reference's devnet.
    Reference,
    /// A run on a devnet of this tree.
    Heartwood,
}

impl Side {
    /// The side's name, as the run's directory ends with it.
    pub fn name(self) -> &'static str {
        match self {
            Side::Reference => "reference",
            Side::Heartwood => "heartwood",
        }
    }
}

/// A run's chain, judged: its cases (label and outcome), and whether every one of them passed.
#[derive(Debug, Clone, Default)]
pub struct Judgement {
    /// The cases.
    pub cases: Vec<(String, Outcome)>,
    /// Whether the run's chain was judged and held at every step.
    pub holds: bool,
}

/// Judge the chain of the `side` run of `suite` in `run_dir`, its corpus under `root` (judged by
/// `oracle` when it has not been yet).
pub fn judge(
    run_dir: &Path,
    side: Side,
    suite: &str,
    root: &Path,
    oracle: &dyn Fn() -> Result<Oracle>,
) -> Judgement {
    let mut out = Judgement::default();
    let label = |what: &str| format!("the {} run's chain: {what}", side.name());
    let manifest = match Manifest::load(run_dir) {
        Ok(m) => m,
        Err(e) => {
            out.cases
                .push((label("its run.json"), Outcome::HarnessError(e.to_string())));
            return out;
        }
    };
    let source = match side {
        // The node's own directory holds its block files (`difftest/`) and its crypto files.
        Side::Heartwood => manifest.control.clone(),
        Side::Reference => format!("archives/{}", manifest.control),
    };
    let name = format!("suite-{suite}-{}-{}", side.name(), manifest.seed);
    let judged = match corpora::prepare(run_dir, &name, &source, &manifest.seed, root, oracle) {
        Ok(judged) => judged,
        Err(e) => {
            out.cases.push((
                label(&format!("imported from {source} and judged")),
                Outcome::HarnessError(e.to_string()),
            ));
            return out;
        }
    };
    out.cases.push(anchors(&judged, &source, &label));
    out.cases.push(processor(&judged, &label));
    match side {
        Side::Heartwood => out.cases.extend(nodes(run_dir, &manifest, &judged, &label)),
        Side::Reference => out.cases.push(replay(&judged, &label)),
    }
    out.holds = out.cases.iter().all(|(_, o)| *o == Outcome::Pass);
    out
}

/// What a Heartwood run's relay is built from: a change here since the run's commit makes it
/// another relay than this tree's.
const PRODUCT: [&str; 4] = [
    "packages",
    "Cargo.toml",
    "Cargo.lock",
    ":(top,exclude,glob)**/*.md",
];

/// Whether the Heartwood run in `run_dir` tested this tree's relay: its `run.json` names the
/// commit its relay was built from (`worktreeCommit`, from a clean tree), and nothing the relay is
/// built from changed between that commit and the head of the tree in `repo`. A comparison made
/// at a later head than its runs counts only for the relay they tested.
pub fn built_from(run_dir: &Path, repo: &Path) -> (String, Outcome) {
    let label = "the heartwood run's relay is built from this tree's product code".to_owned();
    let worktree = match Manifest::load(run_dir) {
        Ok(manifest) => manifest.worktree,
        Err(e) => return (label, Outcome::HarnessError(e.to_string())),
    };
    let problem = match worktree {
        None => Some("run.json names no worktreeCommit: the relay's tree is unknown".to_owned()),
        Some((commit, false)) => Some(format!("the relay was built from {commit} with changes")),
        Some((commit, true)) => {
            let diff = std::process::Command::new("git")
                .arg("-C")
                .arg(repo)
                .args(["diff", "--quiet", &commit, "HEAD", "--"])
                .args(PRODUCT)
                .output();
            match diff.map(|o| o.status.code()) {
                Ok(Some(0)) => None,
                Ok(Some(1)) => Some(format!(
                    "the product changed since {commit}, which the relay was built from: \
                     repeat the run"
                )),
                _ => Some(format!("{commit} is not a commit of this tree")),
            }
        }
    };
    (label, problem.map_or(Outcome::Pass, Outcome::HarnessError))
}

fn fail(problems: &[String]) -> Outcome {
    let shown: Vec<&str> = problems.iter().take(SHOWN).map(String::as_str).collect();
    let more = problems.len().saturating_sub(SHOWN);
    let text = if more > 0 {
        format!("{}; and {more} more", shown.join("; "))
    } else {
        shown.join("; ")
    };
    Outcome::HarnessError(text)
}

/// The judge's anchors hold for the imported chain.
fn anchors(judged: &Judged, source: &str, label: &dyn Fn(&str) -> String) -> (String, Outcome) {
    let what = label(&format!(
        "imported from {source} and judged to its tip {}, the judge's anchors hold",
        judged.tip
    ));
    if judged.anchors_ok {
        return (what, Outcome::Pass);
    }
    let failed: Vec<String> = judged
        .anchors
        .get("anchors")
        .and_then(Value::as_object)
        .map(|a| {
            a.iter()
                .filter(|(_, v)| v.get("ok") != Some(&Value::Bool(true)))
                .map(|(k, _)| format!("anchor {k} does not hold"))
                .collect()
        })
        .unwrap_or_default();
    (what, fail(&failed))
}

/// The reference's processor accepted every block of the chain: its generator at its slot, its
/// economics.
fn processor(judged: &Judged, label: &dyn Fn(&str) -> String) -> (String, Outcome) {
    let what = label("the reference's processing accepted every block, each forger at its slot");
    let rows = match run::read_lines(&judged.dir.join(TS_PROCESSOR)) {
        Ok(rows) => rows,
        Err(e) => return (what, Outcome::HarnessError(e.to_string())),
    };
    let accepted: std::collections::BTreeSet<u64> = rows
        .iter()
        .filter(|r| r["step"] == "apply" && r["result"] == "Accepted")
        .filter_map(|r| r["h"].as_u64())
        .collect();
    let mut problems: Vec<String> = (2..=u64::from(judged.tip))
        .filter(|h| !accepted.contains(h))
        .map(|h| format!("height {h} has no accepted result"))
        .collect();
    problems.extend(
        rows.iter()
            .filter(|r| r.get("result").is_some_and(|x| x != "Accepted"))
            .map(|r| format!("height {}: {}", r["h"], r["result"])),
    );
    if problems.is_empty() {
        (what, Outcome::Pass)
    } else {
        (what, fail(&problems))
    }
}

/// Every Heartwood node's final chain held to the judge's lines.
fn nodes(
    run_dir: &Path,
    manifest: &Manifest,
    judged: &Judged,
    label: &dyn Fn(&str) -> String,
) -> Vec<(String, Outcome)> {
    let chains = match run_chain::heartwood(run_dir) {
        Ok(chain) => chain.lines,
        Err(e) => {
            return vec![(
                label("the nodes' chains"),
                Outcome::HarnessError(e.to_string()),
            )];
        }
    };
    let mut out = Vec::new();
    for node in manifest
        .nodes
        .iter()
        .filter(|n| n.implementation == Implementation::Heartwood)
    {
        let what = |text: &str| label(&format!("{}'s {text}", node.name));
        let Some(lines) = chains.get(&node.name) else {
            out.push((
                what("final chain"),
                Outcome::HarnessError("no digest line of the node".into()),
            ));
            continue;
        };
        let mut problems = Vec::new();
        let mut compared = 0usize;
        for (h, line) in lines {
            let id = line.get("id").and_then(Value::as_str).unwrap_or_default();
            let Some(want) = judged.chain.forward.get(h) else {
                problems.push(format!("height {h} is above the judged tip {}", judged.tip));
                continue;
            };
            if !judged.chain.holds(*h, id) {
                problems.push(format!("height {h}: block {id} is not the judged chain's"));
                continue;
            }
            let keys: &[&str] = if *h == 1 { &SAME_STATE } else { &SAME_STEP };
            let differing = differing_keys(line, want, keys);
            if differing.is_empty() {
                compared += 1;
            } else {
                problems.push(format!(
                    "height {h}: its line differs from the judge's in {}",
                    differing.join(", ")
                ));
            }
        }
        let first = lines.keys().next().copied().unwrap_or(1);
        let missing: Vec<u32> = (first..=judged.tip)
            .filter(|h| !lines.contains_key(h))
            .collect();
        if !missing.is_empty() {
            problems.push(format!(
                "{} heights of the judged chain it does not hold, from {}",
                missing.len(),
                missing[0]
            ));
        }
        let text = format!("final chain equals the judge's at {compared} heights");
        out.push((
            what(&text),
            if problems.is_empty() {
                Outcome::Pass
            } else {
                fail(&problems)
            },
        ));
    }
    out
}

/// The reference's chain replayed through Heartwood's store against the judge's lines.
fn replay(judged: &Judged, label: &dyn Fn(&str) -> String) -> (String, Outcome) {
    let what =
        label("replayed through Heartwood's store, equal to the judge's lines at every step");
    let report = Corpus::load(&judged.dir).and_then(|c| l2::run(&c, &L2Options::default(), None));
    match report {
        Err(e) => (what, Outcome::HarnessError(e.to_string())),
        Ok(report) => {
            let problems: Vec<String> = report
                .results
                .iter()
                .filter(|r| r.outcome.is_failure() || matches!(r.outcome, Outcome::Skipped(_)))
                .map(|r| format!("{} (height {}): {:?}", r.label, r.height, r.outcome))
                .collect();
            if problems.is_empty() && report.compared() > 0 {
                (
                    format!("{what} ({} compared)", report.compared()),
                    Outcome::Pass,
                )
            } else if problems.is_empty() {
                (
                    what,
                    Outcome::HarnessError("the replay compared nothing".into()),
                )
            } else {
                (what, fail(&problems))
            }
        }
    }
}
