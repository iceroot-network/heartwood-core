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
pub mod format;

use std::path::{Path, PathBuf};

use crate::errors::{Error, Result};
use crate::oracle::Oracle;
use crate::report::Report;

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
    let _ = (scenario, oracle);
    Err(Error::NotYet("scenario compile is not built yet".into()))
}

/// Run the compiled scenario on `backend`, as `options` say.
pub fn run(compiled: &Compiled, backend: &mut dyn Backend, options: &RunOptions) -> Result<Report> {
    let _ = (compiled, backend, options);
    Err(Error::NotYet("scenario run is not built yet".into()))
}

/// `difftest scenario run`: every scenario of `args.path` compiled, then run on its backend at
/// its level; one report of every step's checks.
pub fn run_command(args: &crate::cli::ScenarioRunArgs) -> Result<Report> {
    let _ = args;
    Err(Error::NotYet("scenario run is not built yet".into()))
}

/// Compare a suite's reference and Heartwood runs of one seed under `dir` (`<suite>-reference`
/// and `<suite>-heartwood`), every suite there or `suite` alone.
pub fn compare(dir: &Path, suite: Option<&str>) -> Result<Report> {
    let _ = (dir, suite);
    Err(Error::NotYet("scenario compare is not built yet".into()))
}
