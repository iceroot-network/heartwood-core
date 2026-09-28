//! `heartwood relay run|start|stop|restart|status|log`.

use std::process::ExitCode;

use clap::{Args, Subcommand};

use super::{LogFlags, ProcessFlags};
pub use crate::cli::RelayRun;

/// A relay's command.
#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
pub enum RelayCommand {
    /// Run a relay in the foreground.
    Run(RelayRun),
    /// Start a relay under its supervisor.
    Start(RelayStart),
    /// Stop a relay.
    Stop(ProcessFlags),
    /// Restart a relay.
    Restart(ProcessFlags),
    /// A relay's process status.
    Status(ProcessFlags),
    /// A relay's log.
    Log(LogFlags),
}

/// The flags of `relay start`: those of `relay run`, and whether to leave the terminal.
#[derive(Debug, Clone, Default, PartialEq, Eq, Args)]
pub struct RelayStart {
    /// The relay's flags.
    #[command(flatten)]
    pub run: RelayRun,
    /// Run the supervisor as a daemon.
    #[arg(long)]
    pub daemon: bool,
}

/// Run `command`.
pub fn run(command: RelayCommand) -> ExitCode {
    match command {
        RelayCommand::Run(run) => crate::exit(crate::app::run_relay(&run)),
        command => {
            let _ = command;
            todo!("the relay's process commands")
        }
    }
}
