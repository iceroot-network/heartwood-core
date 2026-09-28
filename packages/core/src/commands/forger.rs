//! `heartwood forger run|start|stop|restart|status|log`.

use std::process::ExitCode;

use clap::{Args, Subcommand};

use super::{LogFlags, ProcessFlags};

/// A forger's command.
#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
pub enum ForgerCommand {
    /// Run a forger in the foreground.
    Run(ForgerRun),
    /// Start a forger under its supervisor.
    Start(ForgerStart),
    /// Stop a forger.
    Stop(ProcessFlags),
    /// Restart a forger.
    Restart(ProcessFlags),
    /// A forger's process status.
    Status(ProcessFlags),
    /// A forger's log.
    Log(LogFlags),
}

/// The flags of `forger run`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Args)]
pub struct ForgerRun {
    /// The token of the node's directories (`solar` by default).
    #[arg(long)]
    pub token: Option<String>,
    /// The network's name.
    #[arg(long)]
    pub network: Option<String>,
    /// The environment's name.
    #[arg(long)]
    pub env: Option<String>,
}

/// The flags of `forger start`: those of `forger run`, and whether to leave the terminal.
#[derive(Debug, Clone, Default, PartialEq, Eq, Args)]
pub struct ForgerStart {
    /// The forger's flags.
    #[command(flatten)]
    pub run: ForgerRun,
    /// Run the supervisor as a daemon.
    #[arg(long)]
    pub daemon: bool,
}

/// Run `command`.
pub fn run(command: ForgerCommand) -> ExitCode {
    let _ = command;
    todo!("the forger's commands")
}
