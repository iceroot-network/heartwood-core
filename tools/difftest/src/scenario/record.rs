//! What a run kept of each step (`scenario-<id>.json` in the network's directory), and the
//! comparison of a suite's two runs, the reference's and Heartwood's, from those records, from each
//! run's chain (the digests on their common prefix, [`super::run_chain`]) and from each run's
//! chain judged against the reference ([`super::judge`]). A check passes when it passed on both
//! runs: one that failed on both fails, one skipped on both stays skipped (so a strict comparison
//! fails it), except a digest check, which the chains' judgement settles. A refusal is held to its
//! code and its reason, the numbers that stand alone in the reason masked; one whose reason either
//! run did not keep is not judged. Every scenario a run's directory holds at L3 must have a record
//! on both sides, and every scripted step of either run must have met its expectation.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::backend::{Submitted, Tip};
use super::format::Level;
use super::judge::{self, Side};
use super::run_chain::{self, RunChain};
use crate::compare::diff;
use crate::errors::{Error, Result};
use crate::oracle::Oracle;
use crate::report::{CaseResult, Outcome, Report};

/// The directory suffixes of a suite's two runs.
const SIDES: [&str; 2] = ["reference", "heartwood"];

/// One run of one scenario.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Record {
    /// The scenario's id.
    pub scenario: String,
    /// The backend that ran it.
    pub backend: String,
    /// The tip at the end, when the network still answered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tip: Option<(u32, String)>,
    /// The steps that fired.
    pub steps: Vec<StepRecord>,
}

/// One step that fired.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StepRecord {
    /// The step's index.
    pub index: usize,
    /// Its name.
    pub name: String,
    /// The tip it fired at.
    pub tip: (u32, String),
    /// The pools' answers to what it submitted: reference, admitted, refusal code.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub submitted: Vec<(String, bool, Option<String>)>,
    /// The refusals' messages, without the transaction's own text, by reference.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub refusals: Vec<(String, String)>,
    /// Each check's label and how it came out: `pass`, `skipped: why` or `failed: why`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outcomes: Vec<(String, String)>,
    /// The value of each `api` check, by label, as the nodes last agreed on it (or as the first
    /// node answered when they did not).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub projections: Vec<(String, Value)>,
}

impl StepRecord {
    /// The record of step `index`, named `name`, fired at `tip`.
    pub fn new(index: usize, name: &str, tip: &Tip) -> Self {
        StepRecord {
            index,
            name: name.to_owned(),
            tip: (tip.height, tip.id.clone()),
            submitted: Vec::new(),
            refusals: Vec::new(),
            outcomes: Vec::new(),
            projections: Vec::new(),
        }
    }

    /// Keep the pools' `answers`.
    pub fn answered(&mut self, answers: &[Submitted]) {
        self.submitted = answers
            .iter()
            .map(|a| (a.reference.clone(), a.accepted, a.code.clone()))
            .collect();
        self.refusals = answers
            .iter()
            .filter_map(|a| Some((a.reference.clone(), a.message.clone()?)))
            .collect();
    }

    /// Keep the outcome of the check `label`.
    pub fn checked(&mut self, label: &str, outcome: &Outcome) {
        let text = match outcome {
            Outcome::Pass => "pass".to_owned(),
            Outcome::Skipped(why) => format!("skipped: {why}"),
            other => format!("failed: {other:?}"),
        };
        self.outcomes.push((label.to_owned(), text));
    }
}

impl Record {
    /// The record of the run of scenario `scenario` on `backend`.
    pub fn new(scenario: &str, backend: &str, steps: Vec<StepRecord>, tip: Option<Tip>) -> Self {
        Record {
            scenario: scenario.to_owned(),
            backend: backend.to_owned(),
            tip: tip.map(|t| (t.height, t.id)),
            steps,
        }
    }

    /// Where the record of scenario `id` lives under the network directory `dir`.
    pub fn path(dir: &Path, id: &str) -> PathBuf {
        dir.join(format!("scenario-{id}.json"))
    }

    /// Write the record under `dir`.
    pub fn write(&self, dir: &Path) -> Result<PathBuf> {
        let path = Record::path(dir, &self.scenario);
        std::fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
        let text = serde_json::to_string_pretty(self).map_err(|e| Error::Usage(e.to_string()))?;
        std::fs::write(&path, text).map_err(|e| Error::io(&path, e))?;
        Ok(path)
    }

    /// Read a record.
    pub fn read(path: &Path) -> Result<Record> {
        let text = std::fs::read_to_string(path).map_err(|e| Error::io(path, e))?;
        serde_json::from_str(&text).map_err(|e| Error::Corpus {
            path: path.to_path_buf(),
            message: e.to_string(),
        })
    }
}

/// The records of every scenario under `dir`, by scenario id.
fn records(dir: &Path) -> Result<BTreeMap<String, Record>> {
    let mut out = BTreeMap::new();
    for entry in std::fs::read_dir(dir).map_err(|e| Error::io(dir, e))? {
        let path = entry.map_err(|e| Error::io(dir, e))?.path();
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        if name.starts_with("scenario-") && name.ends_with(".json") {
            let record = Record::read(&path)?;
            out.insert(record.scenario.clone(), record);
        }
    }
    Ok(out)
}

fn case(
    suite: &str,
    scenario: &str,
    step: usize,
    label: String,
    height: u32,
    outcome: Outcome,
) -> CaseResult {
    CaseResult {
        file: format!("{suite}/{scenario}"),
        line: step + 1,
        op: "scenario.compare".into(),
        label,
        height: u64::from(height),
        outcome,
    }
}

fn differs(
    what: &str,
    reference: impl std::fmt::Debug,
    heartwood: impl std::fmt::Debug,
) -> Outcome {
    Outcome::VerdictMismatch {
        oracle: format!("{what} {reference:?}"),
        heartwood: format!("{what} {heartwood:?}"),
    }
}

/// The projection that two runs of a suite compare with each other: the scenario's own wallets,
/// which the same accepted transactions leave equal on both implementations. The other projections
/// hold what two independent networks do not share (the blocks, their forgers and rewards).
const SHARED_PROJECTION: &str = ": api wallets";

/// The start of the reason a live run gives a digest check: its chain's judgement settles it.
const LIVE_DIGESTS: &str = "skipped: a live network's digests are judged against the reference";

/// A refusal's message as two networks can share it: each number that stands alone taken out (a
/// height, a balance or a count of blocks belongs to each network). Digits that are part of a
/// word, such as a delegate's name (`genesis_44`) or an id, are kept: they name what was refused.
fn shared_message(message: &str) -> String {
    let word = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
    let chars: Vec<char> = message.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        if !chars[i].is_ascii_digit() {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        let end = chars[i..]
            .iter()
            .position(|c| !c.is_ascii_digit())
            .map_or(chars.len(), |n| i + n);
        let before = i.checked_sub(1).map(|j| chars[j]);
        if word(before) || word(chars.get(end).copied()) {
            out.extend(&chars[i..end]);
        } else {
            out.push('#');
        }
        i = end;
    }
    out
}

/// How each run's chain came out of its judgement: `Some(true)` judged and held, `Some(false)`
/// judged and not held, `None` not judged.
type Chains = (Option<bool>, Option<bool>);

/// A check that came out alike on both runs: passed on both, failed on both (a failure), skipped
/// on both (skipped, so that a strict comparison fails it), or a digest check that the chains'
/// judgement settles.
fn alike(was: &str, chains: Chains) -> Outcome {
    if was == "pass" {
        return Outcome::Pass;
    }
    if was.starts_with(LIVE_DIGESTS) {
        return match chains {
            (Some(true), Some(true)) => Outcome::Pass,
            (Some(false), _) | (_, Some(false)) => Outcome::HarnessError(
                "a run's chain is not held to the reference's (see its chain cases)".into(),
            ),
            _ => Outcome::Skipped("the runs' chains were not judged".into()),
        };
    }
    match was.strip_prefix("skipped: ") {
        Some(why) => Outcome::Skipped(why.to_owned()),
        None => Outcome::HarnessError(format!("on both runs: {was}")),
    }
}

/// The two runs of one scenario, step by step: what the pools answered and why they refused, how
/// each check came out, and the scenario's wallets where a step read them. Two runs are two
/// networks that forged their own blocks, so their chains are not compared step by step: [`chains`]
/// holds each run to one chain and both to their common prefix, and each chain is judged against
/// the reference.
fn compare_runs(
    suite: &str,
    reference: &Record,
    heartwood: &Record,
    judged: Chains,
) -> Vec<CaseResult> {
    let id = &reference.scenario;
    let mut cases = Vec::new();
    if reference.steps.len() != heartwood.steps.len() {
        cases.push(case(
            suite,
            id,
            0,
            "steps fired".into(),
            0,
            differs("steps", reference.steps.len(), heartwood.steps.len()),
        ));
    }
    for (r, h) in reference.steps.iter().zip(&heartwood.steps) {
        let height = r.tip.0.min(h.tip.0);
        if !r.submitted.is_empty() || !h.submitted.is_empty() {
            // Every refusal is held to its reason as well as its code: the reference reports most
            // refusals as ERR_APPLY, its handler's reason in the message only. A refusal whose
            // reason a run did not keep is not judged.
            let reasons = |s: &StepRecord| -> BTreeMap<String, String> {
                s.refusals
                    .iter()
                    .map(|(reference, m)| (reference.clone(), shared_message(m)))
                    .collect()
            };
            let (said, heard) = (reasons(r), reasons(h));
            let unexplained: Vec<&str> = r
                .submitted
                .iter()
                .filter(|(reference, accepted, _)| {
                    !accepted && !(said.contains_key(reference) && heard.contains_key(reference))
                })
                .map(|(reference, _, _)| reference.as_str())
                .collect();
            let outcome = if r.submitted != h.submitted {
                differs("answers", &r.submitted, &h.submitted)
            } else if !unexplained.is_empty() {
                Outcome::Skipped(format!(
                    "a run kept no reason for the refusal of {}",
                    unexplained.join(", ")
                ))
            } else if said != heard {
                differs("refusals", &r.refusals, &h.refusals)
            } else {
                Outcome::Pass
            };
            cases.push(case(
                suite,
                id,
                r.index,
                format!("{}: pool answers", r.name),
                height,
                outcome,
            ));
        }
        for (label, was) in &r.outcomes {
            let outcome = match h.outcomes.iter().find(|(l, _)| l == label) {
                None => differs(label, was, "no such check"),
                Some((_, now)) if now == was => alike(was, judged),
                Some((_, now)) => differs(label, was, now),
            };
            cases.push(case(suite, id, r.index, label.clone(), height, outcome));
        }
        for (label, was) in r
            .projections
            .iter()
            .filter(|(l, _)| l.ends_with(SHARED_PROJECTION))
        {
            let outcome = match h.projections.iter().find(|(l, _)| l == label) {
                None => differs(label, "a value", "none"),
                Some((_, now)) => {
                    let differences = diff(was, now);
                    if differences.is_empty() {
                        Outcome::Pass
                    } else {
                        Outcome::Mismatch(differences)
                    }
                }
            };
            cases.push(case(
                suite,
                id,
                r.index,
                format!("{label} of both runs"),
                height,
                outcome,
            ));
        }
    }
    cases
}

/// The digests on the common prefix of a suite's two runs: each run's nodes hold one chain, and the
/// two runs one genesis, the prefix two independent networks of one seed share.
fn chains(suite: &str, reference: &Path, heartwood: &Path) -> Vec<CaseResult> {
    let sides: [(&str, std::result::Result<RunChain, String>); 2] = [
        (
            "reference",
            run_chain::reference(reference).map_err(|e| e.to_string()),
        ),
        (
            "heartwood",
            run_chain::heartwood(heartwood).map_err(|e| e.to_string()),
        ),
    ];
    let mut out = Vec::new();
    for (side, chain) in &sides {
        let (label, outcome) = match chain {
            Ok(chain) => (
                format!(
                    "the {side} run: one chain, {} nodes equal at {} shared heights",
                    chain.nodes, chain.shared
                ),
                chain.verdict().map_or(Outcome::Pass, Outcome::HarnessError),
            ),
            Err(e) => (
                format!("the {side} run: one chain"),
                Outcome::HarnessError(format!("the {side} run's chain: {e}")),
            ),
        };
        out.push(case(suite, "chain", 0, label, 0, outcome));
    }
    let genesis = |i: usize| {
        sides[i]
            .1
            .as_ref()
            .map(|c| c.genesis.clone())
            .unwrap_or_default()
    };
    let (r, h) = (genesis(0), genesis(1));
    let outcome = if r.len() == 1 && r == h {
        Outcome::Pass
    } else {
        differs("genesis", &r, &h)
    };
    out.push(case(
        suite,
        "chain",
        0,
        "the genesis of both runs, their common prefix".into(),
        1,
        outcome,
    ));
    out
}

/// The scenarios a run's directory holds (`scenarios/*.json`, as the recipe copied them) that run
/// at L3, by id.
fn planned(run: &Path) -> Result<Vec<String>> {
    let dir = run.join("scenarios");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for entry in entries {
        let path = entry.map_err(|e| Error::io(&dir, e))?.path();
        if path.extension().is_some_and(|x| x == "json") {
            let scenario = super::load(&path)?;
            if scenario.levels.contains(&Level::L3) {
                out.push(scenario.id);
            }
        }
    }
    out.sort();
    Ok(out)
}

/// The scripted steps of a run (`steps.jsonl`) that did not meet their expectation; the block-lag
/// step judged from the run's records ([`crate::live::suite_propagation`]) where the recipe judged it.
fn failed_steps(run: &Path) -> std::result::Result<(usize, Vec<String>), String> {
    let path = run.join("steps.jsonl");
    let lines = crate::live::run::read_lines(&path).map_err(|e| e.to_string())?;
    let failed = lines
        .iter()
        .filter_map(|l| {
            let step = l.get("step").and_then(Value::as_str).unwrap_or("?");
            let node = l.get("node").and_then(Value::as_str).unwrap_or("?");
            if step == "propagation-lag" {
                match crate::live::suite_propagation(run) {
                    Ok(None) => {}
                    Ok(Some(problems)) if problems.is_empty() => return None,
                    Ok(Some(problems)) => {
                        return Some(format!("{step} ({node}): {}", problems.join("; ")));
                    }
                    Err(e) => return Some(format!("{step} ({node}): {e}")),
                }
            }
            (l.get("ok") != Some(&Value::Bool(true))).then(|| format!("{step} ({node})"))
        })
        .collect();
    Ok((lines.len(), failed))
}

/// Where a comparison judges the runs' chains: the corpus root, and the oracle when a chain has
/// not been judged yet; without it, the chains are not judged.
pub struct Judging<'a> {
    /// Where the runs' corpora are imported and judged.
    pub root: &'a Path,
    /// The oracle.
    pub oracle: &'a dyn Fn() -> Result<Oracle>,
    /// The tree whose relay the Heartwood runs must have tested ([`judge::built_from`]), when
    /// the comparison binds them to one.
    pub repo: Option<&'a Path>,
}

/// Compare the suites under `dir` (`<suite>-reference` and `<suite>-heartwood`), every one or
/// `only` alone: the scenarios' records side by side, each run's scripted steps, each run's chain,
/// and, with `judging`, each run's chain judged against the reference.
pub fn compare(dir: &Path, only: Option<&str>, judging: Option<&Judging<'_>>) -> Result<Report> {
    let mut suites: BTreeMap<String, [Option<PathBuf>; 2]> = BTreeMap::new();
    for entry in std::fs::read_dir(dir).map_err(|e| Error::io(dir, e))? {
        let path = entry.map_err(|e| Error::io(dir, e))?.path();
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_owned();
        for (side, suffix) in SIDES.iter().enumerate() {
            let suite = name.strip_suffix(&format!("-{suffix}"));
            if let Some(suite) = suite.filter(|s| path.is_dir() && only.is_none_or(|o| o == *s)) {
                suites.entry(suite.to_owned()).or_default()[side] = Some(path.clone());
            }
        }
    }
    let mut results = Vec::new();
    if suites.is_empty() {
        results.push(case(
            "suites",
            "-",
            0,
            "suites found".into(),
            0,
            Outcome::HarnessError(format!(
                "{} holds no suite with a reference and a heartwood run",
                dir.display()
            )),
        ));
    }
    for (suite, sides) in &suites {
        let [Some(reference), Some(heartwood)] = sides else {
            results.push(case(
                suite,
                "-",
                0,
                "both runs".into(),
                0,
                Outcome::HarnessError(format!("suite {suite} has only one of its two runs")),
            ));
            continue;
        };
        let mut judged: Chains = (None, None);
        if let Some(judging) = judging {
            for (side, run, slot) in [
                (Side::Reference, reference, &mut judged.0),
                (Side::Heartwood, heartwood, &mut judged.1),
            ] {
                let mut judgement = judge::judge(run, side, suite, judging.root, judging.oracle);
                if let (Side::Heartwood, Some(repo)) = (side, judging.repo) {
                    let (label, outcome) = judge::built_from(run, repo);
                    judgement.holds &= outcome == Outcome::Pass;
                    judgement.cases.push((label, outcome));
                }
                for (label, outcome) in judgement.cases {
                    results.push(case(suite, "chain", 0, label, 0, outcome));
                }
                *slot = Some(judgement.holds);
            }
        }
        for (side, run) in [("reference", reference), ("heartwood", heartwood)] {
            let (label, outcome) = match failed_steps(run) {
                Err(e) => (
                    format!("the {side} run's scripted steps"),
                    Outcome::HarnessError(e),
                ),
                Ok((_, failed)) if !failed.is_empty() => (
                    format!("the {side} run's scripted steps"),
                    Outcome::HarnessError(format!(
                        "{} step(s) did not meet their expectation: {}",
                        failed.len(),
                        failed.join(", ")
                    )),
                ),
                Ok((all, _)) => (
                    format!("the {side} run's {all} scripted steps met their expectations"),
                    Outcome::Pass,
                ),
            };
            results.push(case(suite, "steps", 0, label, 0, outcome));
        }
        let (r, h) = (records(reference)?, records(heartwood)?);
        let mut ids: Vec<String> = planned(reference)?;
        ids.extend(planned(heartwood)?);
        ids.sort();
        ids.dedup();
        for id in ids
            .iter()
            .filter(|id| !r.contains_key(*id) && !h.contains_key(*id))
        {
            results.push(case(
                suite,
                id,
                0,
                "both runs".into(),
                0,
                Outcome::HarnessError("neither run has a record of it".into()),
            ));
        }
        for (id, run) in &r {
            match h.get(id) {
                Some(other) => results.extend(compare_runs(suite, run, other, judged)),
                None => results.push(case(
                    suite,
                    id,
                    0,
                    "both runs".into(),
                    0,
                    Outcome::HarnessError("Heartwood's run has no record of it".into()),
                )),
            }
        }
        if let Some(id) = h.keys().find(|id| !r.contains_key(*id)) {
            results.push(case(
                suite,
                id,
                0,
                "both runs".into(),
                0,
                Outcome::HarnessError("the reference's run has no record of it".into()),
            ));
        }
        results.extend(chains(suite, reference, heartwood));
    }
    Ok(Report {
        title: "scenario compare".into(),
        source: format!("the runs under {}", dir.display()),
        files: suites.len(),
        results,
    })
}
