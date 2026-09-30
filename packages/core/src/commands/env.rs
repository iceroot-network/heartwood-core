//! `heartwood env get|list|set|paths`: an instance's environment file and directories.

use std::process::ExitCode;

use clap::{Args, Subcommand};
use heartwood_cli::instance::InstanceFlags;

/// An `env` command.
#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
pub enum EnvCommand {
    /// The value of an environment variable.
    Get(EnvKey),
    /// Every environment variable, in the file's order.
    List(EnvFlags),
    /// Set an environment variable.
    Set(EnvSet),
    /// The instance's directories.
    Paths(EnvFlags),
}

/// The instance an `env` command reads.
#[derive(Debug, Clone, Default, PartialEq, Eq, Args)]
pub struct EnvFlags {
    /// The instance.
    #[command(flatten)]
    pub instance: InstanceFlags,
}

/// The flags of `env get`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Args)]
pub struct EnvKey {
    /// The instance.
    #[command(flatten)]
    pub instance: InstanceFlags,
    /// The variable's name.
    #[arg(long)]
    pub key: String,
}

/// The flags of `env set`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Args)]
pub struct EnvSet {
    /// The instance.
    #[command(flatten)]
    pub instance: InstanceFlags,
    /// The variable's name.
    #[arg(long)]
    pub key: String,
    /// Its new value.
    #[arg(long)]
    pub value: String,
}

/// Run `command`.
pub fn run(command: EnvCommand) -> ExitCode {
    let what = match command {
        EnvCommand::Get(_) => "env get",
        EnvCommand::List(_) => "env list",
        EnvCommand::Set(_) => "env set",
        EnvCommand::Paths(_) => "env paths",
    };
    super::not_built(what)
}
