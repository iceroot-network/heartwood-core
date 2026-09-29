//! `heartwood forger run|start|stop|restart|status|log`.

use std::ffi::OsString;
use std::process::ExitCode;

use clap::{ArgAction, Args, Subcommand};
use heartwood_cli::Role;
use heartwood_cli::instance::InstanceFlags;

use super::{Manage, on_instance};

/// A forger's command.
#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
pub enum ForgerCommand {
    /// Run a forger in the foreground.
    Run(ForgerRun),
    /// Start a forger under its supervisor.
    Start(ForgerStart),
    /// Stop, restart, show or tail a started forger.
    #[command(flatten)]
    Manage(Manage),
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
#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct ForgerStart {
    /// The forger's flags.
    #[command(flatten)]
    pub run: ForgerRun,
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
pub fn run(command: ForgerCommand) -> ExitCode {
    match command {
        ForgerCommand::Run(run) => crate::exit(crate::app::forger::run_forger(&run)),
        ForgerCommand::Start(start) => {
            let (token, network) = (start.run.token, start.run.network);
            let args = (start.run.env.into_iter()).flat_map(|env| ["--env".into(), env.into()]);
            let args: Vec<OsString> = args.collect();
            let flags = InstanceFlags { token, network };
            on_instance(Role::Forger, &flags, |i| {
                super::start(i, args, start.daemon)
            })
        }
        ForgerCommand::Manage(command) => super::manage(Role::Forger, command),
    }
}
