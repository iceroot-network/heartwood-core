//! The scenarios: their format ([`mod@format`]), compiled once through the oracle (every transaction
//! built with the reference's builders, the fixed aux and a strict decode at its planned height,
//! so both implementations receive the same bytes), and run by the runner against a [`Backend`]
//! (a devnet of either implementation, the tooling's networks, or the lockstep producer), each step
//! fired when the observed tip first satisfies its trigger, each check recorded as a case.
//!
//! `difftest scenario compile FILE`, `difftest scenario run --backend B [--level L]
//! [--devnet-root DIR] FILE|DIR` and `difftest scenario compare DIR` (a suite's reference and
//! Heartwood runs of one seed, side by side) are the commands.

pub mod backend;
pub mod backends;
pub mod chain;
pub mod checks;
pub mod compile;
pub mod format;
pub mod projection;
pub mod record;
pub mod run_chain;
pub mod runner;

use std::path::{Path, PathBuf};

use crate::errors::{Error, Result};
use crate::oracle::Oracle;
use crate::report::{CaseResult, Outcome, Report};

pub use backend::{Backend, BackendError, Submitted, Tip};
pub use format::{Compiled, CompiledTx, Level, Scenario};

/// The backends the runner offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendKind {
    /// A devnet of this tree (`heartwood devnet`).
    Heartwood,
    /// The executable `DIFFTEST_SCENARIO_BACKEND` names.
    External,
    /// The lockstep producer.
    Lockstep,
}

impl std::str::FromStr for BackendKind {
    type Err = Error;

    fn from_str(text: &str) -> Result<BackendKind> {
        match text {
            "heartwood" => Ok(BackendKind::Heartwood),
            "external" => Ok(BackendKind::External),
            "lockstep" => Ok(BackendKind::Lockstep),
            other => Err(Error::Usage(format!(
                "unknown backend {other}: heartwood, external or lockstep"
            ))),
        }
    }
}

/// What `scenario run` runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunOptions {
    /// The backend.
    pub backend: BackendKind,
    /// The level the scenarios run at; each scenario's own levels when `None`.
    pub level: Option<Level>,
    /// The devnet's directory, for the `heartwood` backend.
    pub devnet_root: Option<PathBuf>,
}

/// Read the scenario file `path`.
pub fn load(path: &Path) -> Result<Scenario> {
    let text = std::fs::read_to_string(path).map_err(|e| Error::io(path, e))?;
    serde_json::from_str(&text).map_err(|e| Error::Corpus {
        path: path.to_path_buf(),
        message: e.to_string(),
    })
}

/// Compile `scenario`'s transactions through `oracle`.
pub fn compile(scenario: &Scenario, oracle: &Oracle) -> Result<Compiled> {
    compile::compile(scenario, oracle)
}

/// The level a backend runs when none is asked for: the lockstep producer's is L2, a devnet's L3.
fn default_level(backend: BackendKind) -> Level {
    match backend {
        BackendKind::Lockstep => Level::L2,
        BackendKind::Heartwood | BackendKind::External => Level::L3,
    }
}

/// Run the compiled scenario on `backend`, as `options` say.
pub fn run(compiled: &Compiled, backend: &mut dyn Backend, options: &RunOptions) -> Result<Report> {
    let level = options
        .level
        .unwrap_or_else(|| default_level(options.backend));
    Ok(runner::run(compiled, backend, level)?.0)
}

/// The scenario files at `path`: the file itself, or the `.json` files under a directory, in
/// order.
fn files(path: &Path) -> Result<Vec<PathBuf>> {
    if path.is_file() {
        return Ok(vec![path.to_path_buf()]);
    }
    let mut found = Vec::new();
    for entry in std::fs::read_dir(path).map_err(|e| Error::io(path, e))? {
        let entry = entry.map_err(|e| Error::io(path, e))?.path();
        if entry.is_dir() {
            found.extend(files(&entry)?);
        } else if entry.extension().is_some_and(|x| x == "json") {
            found.push(entry);
        }
    }
    found.sort();
    Ok(found)
}

/// A case of the command's own: a scenario file that cannot run, or a run with no scenario at its
/// level.
fn note(file: &Path, label: &str, outcome: Outcome) -> CaseResult {
    CaseResult {
        file: file.display().to_string(),
        line: 1,
        op: "scenario.run".into(),
        label: label.to_owned(),
        height: 0,
        outcome,
    }
}

/// The compiled scenario run on the backend `options` name, its record written under the
/// devnet's directory when there is one.
fn run_one(
    compiled: &Compiled,
    args: &crate::cli::ScenarioRunArgs,
    level: Level,
) -> Result<Report> {
    let options = &args.options;
    let root = || {
        options
            .devnet_root
            .clone()
            .ok_or_else(|| Error::Usage("this backend needs --devnet-root DIR".into()))
    };
    let backend_error = |e: BackendError| Error::Usage(e.to_string());
    let (report, record) = match options.backend {
        BackendKind::Lockstep => {
            let mut server = args.oracle.resolve(|k| std::env::var(k).ok())?.server()?;
            let mut reference = crate::lockstep::reference::OracleReference::new(&mut server);
            let mut backend = backends::lockstep::Lockstep::prepare(compiled, &mut reference)?;
            runner::run(compiled, &mut backend, level)?
        }
        BackendKind::Heartwood => {
            let mut backend =
                backends::heartwood::Heartwood::open(&root()?).map_err(backend_error)?;
            runner::run(compiled, &mut backend, level)?
        }
        BackendKind::External => {
            let program = std::env::var_os(backends::external::VARIABLE).ok_or_else(|| {
                Error::Usage(format!(
                    "set {}, the tooling's scenario backend",
                    backends::external::VARIABLE
                ))
            })?;
            let mut backend = backends::external::External::open(Path::new(&program), &root()?)
                .map_err(backend_error)?;
            runner::run(compiled, &mut backend, level)?
        }
    };
    if let Some(dir) = &options.devnet_root {
        record.write(dir)?;
    }
    Ok(report)
}

/// `difftest scenario run`: every scenario of `args.path` compiled, then run on its backend at
/// its level; one report of every step's checks.
pub fn run_command(args: &crate::cli::ScenarioRunArgs) -> Result<Report> {
    let oracle = args.oracle.resolve(|k| std::env::var(k).ok())?;
    let level = args
        .options
        .level
        .unwrap_or_else(|| default_level(args.options.backend));
    let mut results = Vec::new();
    let mut left = Vec::new();
    let paths = files(&args.path)?;
    for path in &paths {
        let scenario = match load(path) {
            Ok(scenario) => scenario,
            Err(e) => {
                results.push(note(
                    path,
                    "read the scenario",
                    Outcome::HarnessError(e.to_string()),
                ));
                continue;
            }
        };
        if !scenario.levels.contains(&level) {
            left.push(scenario.id);
            continue;
        }
        let ran = compile(&scenario, &oracle).and_then(|compiled| run_one(&compiled, args, level));
        match ran {
            Ok(report) => results.extend(report.results),
            Err(e) => results.push(note(
                path,
                &scenario.id,
                Outcome::HarnessError(e.to_string()),
            )),
        }
    }
    if results.is_empty() {
        let why = format!("no scenario of {} runs at {level:?}", args.path.display());
        results.push(note(&args.path, "scenarios", Outcome::HarnessError(why)));
    }
    let mut source = format!(
        "the {:?} backend, {}",
        args.options.backend,
        args.path.display()
    );
    if !left.is_empty() {
        // Named, not counted: a level a scenario does not list is proved by another record.
        source.push_str(&format!(
            "; left to the records of their other levels: {}",
            left.join(", ")
        ));
    }
    Ok(Report {
        title: format!("scenarios at {level:?}"),
        source,
        files: paths.len(),
        results,
    })
}

/// Compare a suite's reference and Heartwood runs of one seed under `dir` (`<suite>-reference`
/// and `<suite>-heartwood`), every suite there or `suite` alone.
pub fn compare(dir: &Path, suite: Option<&str>) -> Result<Report> {
    record::compare(dir, suite)
}
