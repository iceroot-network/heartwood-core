//! `heartwood service install|uninstall`: a systemd unit for one role of an instance, written
//! only.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Subcommand};
use heartwood_cli::Role;
use heartwood_cli::instance::InstanceFlags;

/// A `service` command.
#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
pub enum ServiceCommand {
    /// Write the unit of a role.
    Install(ServiceFlags),
    /// Remove the unit of a role.
    Uninstall(ServiceFlags),
}

/// The flags of `service install` and `uninstall`.
#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct ServiceFlags {
    /// The instance.
    #[command(flatten)]
    pub instance: InstanceFlags,
    /// The role the unit runs: relay, forger or core.
    #[arg(long)]
    pub role: Role,
    /// A system unit instead of a user unit.
    #[arg(long)]
    pub system: bool,
    /// The directory the unit is written to or removed from, instead of the manager's own.
    #[arg(long)]
    pub out: Option<PathBuf>,
}

/// Run `command`.
pub fn run(command: ServiceCommand) -> ExitCode {
    let what = match command {
        ServiceCommand::Install(_) => "service install",
        ServiceCommand::Uninstall(_) => "service uninstall",
    };
    super::not_built(what)
}
