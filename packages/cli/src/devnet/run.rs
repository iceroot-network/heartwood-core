//! `devnet run FILE [ARGS]`: a scenario on the devnet, run by the harness's scenario runner. The
//! runner is not part of the product: `run` finds `heartwood-difftest` next to the running binary
//! or on `PATH` (or the harness's build, `difftest`, next to the running binary) and execs
//! `<runner> scenario run --backend heartwood --devnet-root <root> FILE ARGS` by an explicit argv
//! (never through a shell); without the harness installed it says so.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};

use crate::errors::DevnetError;

/// The harness binary `run` looks for.
pub const RUNNER: &str = "heartwood-difftest";

/// The name the harness's build gives the runner, looked for next to the running binary only.
pub const BUILT_RUNNER: &str = "difftest";

/// The runner in the first of `dirs` that holds it.
pub fn find(dirs: impl IntoIterator<Item = PathBuf>) -> Result<PathBuf, DevnetError> {
    let mut paths = dirs.into_iter().map(|dir| dir.join(RUNNER));
    paths
        .find(|path| path.is_file())
        .ok_or(DevnetError::RunnerMissing(RUNNER))
}

/// The runner: [`RUNNER`] next to the running binary (in `beside`) or in a directory of `path`,
/// else [`BUILT_RUNNER`] next to the running binary.
pub fn locate(beside: Option<&Path>, path: &OsStr) -> Result<PathBuf, DevnetError> {
    let dirs = beside.map(Path::to_path_buf).into_iter();
    find(dirs.chain(std::env::split_paths(path))).or_else(|missing| {
        let built = beside.map(|dir| dir.join(BUILT_RUNNER));
        built.filter(|path| path.is_file()).ok_or(missing)
    })
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
    let list = std::env::var_os("PATH").unwrap_or_default();
    let runner = locate(exe.as_deref().and_then(Path::parent), &list)?;
    let io = |error: std::io::Error| DevnetError::Io {
        path: runner.clone(),
        message: error.to_string(),
    };
    command(&runner, root, scenario, args).status().map_err(io)
}
