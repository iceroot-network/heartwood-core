//! `heartwood pool clear` and `heartwood state clear`: the reference's markers for the relay's
//! next start, written by [`heartwood_cli::recovery`].

use std::process::ExitCode;

use clap::{Args, Subcommand};
use heartwood_cli::instance::InstanceFlags;
use heartwood_cli::recovery::{self, Marker};

/// A pool command.
#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
pub enum PoolCommand {
    /// Clear the relay's pool at its next start.
    Clear(ClearFlags),
}

/// A state command.
#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
pub enum StateCommand {
    /// Clear the relay's state at its next start: it is replayed from the genesis block.
    Clear(ClearFlags),
}

/// The flags of `pool clear` and `state clear`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Args)]
pub struct ClearFlags {
    /// The instance.
    #[command(flatten)]
    pub instance: InstanceFlags,
    /// Write the marker without asking.
    #[arg(long)]
    pub force: bool,
}

/// Run `command`; the process's exit status.
pub fn run_pool(command: PoolCommand) -> ExitCode {
    let PoolCommand::Clear(flags) = command;
    recovery::clear(&flags.instance, Marker::ClearPool, flags.force)
}

/// Run `command`; the process's exit status.
pub fn run_state(command: StateCommand) -> ExitCode {
    let StateCommand::Clear(flags) = command;
    recovery::clear(&flags.instance, Marker::ClearSavedStates, flags.force)
}
