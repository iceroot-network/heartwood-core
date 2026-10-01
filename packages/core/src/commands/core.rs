//! `heartwood core run|start|stop|restart|status|log`: the combined role, a relay and its forger in
//! one process.

use std::process::ExitCode;

use clap::{ArgAction, Args, Subcommand};
use heartwood_cli::supervisor::signals::is_locked;
use heartwood_cli::{Instance, ProcessError, Role};

use super::relay::RelayRun;
use super::{Manage, on_instance};

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

/// Refuse a start while a process of `roles` of the instance runs, under its supervisor or not,
/// naming the first that does, as the reference's starts abort on a running `<token>-<role>`: a
/// relay's or a forger's start on the core role, the core role's on itself, its forger and its
/// relay (a running core process holds the relay's and the forger's locks, so it comes first).
pub(crate) fn refuse_running(instance: &Instance, roles: &[Role]) -> Result<(), ProcessError> {
    for &role in roles {
        let other = Instance {
            role,
            ..instance.clone()
        };
        if is_locked(&other.supervisor_lock_path())? || is_locked(&other.lock_path())? {
            return Err(ProcessError::AlreadyRunning { name: other.name() });
        }
    }
    Ok(())
}

/// Run `command`.
pub fn run(command: CoreCommand) -> ExitCode {
    match command {
        CoreCommand::Run(run) => crate::exit(crate::app::core::run_core(&run)),
        CoreCommand::Start(start) => on_instance(
            Role::Core,
            &start.run.relay.instance(),
            |i| -> Result<(), super::RoleError> {
                refuse_running(i, &[Role::Core, Role::Forger, Role::Relay])?;
                heartwood_forger::load_keys(&i.paths.config)?;
                Ok(super::start(i, start.run.relay.args(), start.daemon)?)
            },
        ),
        CoreCommand::Manage(command) => super::manage(Role::Core, command),
    }
}
