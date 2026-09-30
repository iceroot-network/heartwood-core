//! `devnet run FILE [ARGS]`: a scenario on the devnet, run by the harness's scenario runner. The
//! runner is not part of the product: `run` finds `heartwood-difftest` next to the running binary
//! or on `PATH` and execs `heartwood-difftest scenario run --backend heartwood --devnet-root
//! <root> FILE ARGS` by an explicit argv (never through a shell); without the harness installed it
//! says so.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};

use crate::errors::DevnetError;

/// The harness binary `run` looks for.
pub const RUNNER: &str = "heartwood-difftest";

/// The runner in the first of `dirs` that holds it.
pub fn find(dirs: impl IntoIterator<Item = PathBuf>) -> Result<PathBuf, DevnetError> {
    let mut paths = dirs.into_iter().map(|dir| dir.join(RUNNER));
    paths
        .find(|path| path.is_file())
        .ok_or(DevnetError::RunnerMissing(RUNNER))
}

/// The command that runs `scenario` on the devnet in `root`: `runner` with an explicit argv.
pub fn command(runner: &Path, root: &Path, scenario: &Path, args: &[OsString]) -> Command {
    let mut command = Command::new(runner);
    command.args(["scenario", "run", "--backend", "heartwood", "--devnet-root"]);
    command.arg(root).arg(scenario).args(args);
    command
}

/// Run the scenario `scenario` on the devnet in `root`, with the runner's own `args`; the
/// runner's exit status.
pub fn run(root: &Path, scenario: &Path, args: &[OsString]) -> Result<ExitStatus, DevnetError> {
    let exe = std::env::current_exe().ok();
    let beside = exe.as_deref().and_then(Path::parent).map(Path::to_path_buf);
    let list = std::env::var_os("PATH").unwrap_or_default();
    let runner = find(beside.into_iter().chain(std::env::split_paths(&list)))?;
    let io = |error: std::io::Error| DevnetError::Io {
        path: runner.clone(),
        message: error.to_string(),
    };
    command(&runner, root, scenario, args).status().map_err(io)
}
