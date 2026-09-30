//! `devnet run FILE [ARGS]`: a scenario on the devnet, run by the harness's scenario runner. The
//! runner is not part of the product: `run` finds `heartwood-difftest` next to the running binary
//! or on `PATH` and execs `heartwood-difftest scenario run --backend heartwood --devnet-root
//! <root> FILE ARGS` by an explicit argv (never through a shell); without the harness installed it
//! says so.

use std::ffi::OsString;
use std::path::Path;
use std::process::ExitStatus;

use crate::errors::DevnetError;

/// The harness binary `run` looks for.
pub const RUNNER: &str = "heartwood-difftest";

/// Run the scenario `scenario` on the devnet in `root`, with the runner's own `args`; the
/// runner's exit status.
pub fn run(root: &Path, scenario: &Path, args: &[OsString]) -> Result<ExitStatus, DevnetError> {
    let _ = (root, scenario, args);
    Err(DevnetError::NotBuilt("devnet run"))
}
