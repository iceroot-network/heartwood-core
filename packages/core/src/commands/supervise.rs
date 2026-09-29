//! `heartwood supervise`, hidden: the built-in supervisor of a role that `start` launched.

use std::process::ExitCode;

use clap::Args;
use heartwood_cli::supervisor::SuperviseArgs;

/// The flags of `supervise`, as `start` writes them.
#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct Supervise {
    /// The supervisor's arguments.
    #[command(flatten)]
    pub args: SuperviseArgs,
}

/// Run `command`: supervise until a stop; never returns.
pub fn run(command: Supervise) -> ExitCode {
    command.args.run()
}
