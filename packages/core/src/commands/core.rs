//! `heartwood core run|start|stop|restart|status|log`: the combined role, a relay and its forger in
//! one process.

use std::process::ExitCode;

use clap::{ArgAction, Args, Subcommand};
use heartwood_cli::Role;

use super::Manage;
use super::relay::RelayRun;

/// A `core` command.
#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
pub enum CoreCommand {
    /// Run the core role in the foreground.
    Run(CoreRun),
    /// Start the core role under its supervisor.
    Start(CoreStart),
    /// Stop, restart, show or tail a started core role.
    #[command(flatten)]
    Manage(Manage),
}

/// The flags of `core run`: the relay's, which the core role takes whole.
#[derive(Debug, Clone, Default, PartialEq, Eq, Args)]
pub struct CoreRun {
    /// The relay's flags.
    #[command(flatten)]
    pub relay: RelayRun,
}

/// The flags of `core start`: those of `core run`, and whether to leave the terminal.
#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct CoreStart {
    /// The flags of `core run`.
    #[command(flatten)]
    pub run: CoreRun,
    /// Run the supervisor as a daemon, as the reference's command does by default;
    /// `--daemon false`, `--daemon=false` or `--no-daemon` keeps it in the foreground until the
    /// node stops.
    #[arg(
        long,
        default_value_t = true,
        action = ArgAction::Set,
        num_args = 0..=1,
        default_missing_value = "true"
    )]
    pub daemon: bool,
}

/// Run `command`.
pub fn run(command: CoreCommand) -> ExitCode {
    match command {
        CoreCommand::Run(run) => crate::exit(crate::app::core::run_core(&run)),
        CoreCommand::Start(_) => super::not_built("core start"),
        CoreCommand::Manage(command) => super::manage(Role::Core, command),
    }
}
