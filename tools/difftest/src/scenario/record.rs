//! What a run kept of each step (`scenario-<id>.json` in the network's directory), and the
//! comparison of a suite's two runs, the reference's and Heartwood's, from those records and from
//! each run's chain (the digests on their common prefix, [`super::run_chain`]).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::backend::{Submitted, Tip};
use super::run_chain::{self, RunChain};
use crate::compare::diff;
use crate::errors::{Error, Result};
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

/// The two runs of one scenario, step by step: what the pools answered, how each check came out,
/// and the scenario's wallets where a step read them. Two runs are two networks that forged their
/// own blocks, so their chains are not compared step by step: [`chains`] holds each run to one
/// chain and both to their common prefix.
fn compare_runs(suite: &str, reference: &Record, heartwood: &Record) -> Vec<CaseResult> {
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
            let outcome = if r.submitted == h.submitted {
                Outcome::Pass
            } else {
                differs("answers", &r.submitted, &h.submitted)
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
                Some((_, now)) if now == was => Outcome::Pass,
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

/// Compare the suites under `dir` (`<suite>-reference` and `<suite>-heartwood`), every one or
/// `only` alone: the scenarios' records side by side, and each run's chain.
pub fn compare(dir: &Path, only: Option<&str>) -> Result<Report> {
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
        let (r, h) = (records(reference)?, records(heartwood)?);
        for (id, run) in &r {
            match h.get(id) {
                Some(other) => results.extend(compare_runs(suite, run, other)),
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
