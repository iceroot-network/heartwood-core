//! `heartwood forger run|start|stop|restart|status|log`.

use std::ffi::OsString;
use std::process::ExitCode;

use clap::{Args, Subcommand};
use heartwood_cli::Role;
use heartwood_cli::instance::InstanceFlags;

use super::{LogFlags, ProcessCommand, ProcessFlags, process_command};

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
    let process = match command {
        ForgerCommand::Run(run) => return crate::exit(crate::app::forger::run_forger(&run)),
        ForgerCommand::Start(start) => {
            let flags = InstanceFlags {
                token: start.run.token,
                network: start.run.network,
            };
            let env = start.run.env.into_iter();
            let args = env.flat_map(|env| [OsString::from("--env"), env.into()]);
            ProcessCommand::Start(flags, args.collect(), start.daemon)
        }
        ForgerCommand::Stop(flags) => ProcessCommand::Stop(flags),
        ForgerCommand::Restart(flags) => ProcessCommand::Restart(flags),
        ForgerCommand::Status(flags) => ProcessCommand::Status(flags),
        ForgerCommand::Log(flags) => ProcessCommand::Log(flags),
    };
    process_command(Role::Forger, process)
}
