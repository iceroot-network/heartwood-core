//! `heartwood snapshot rollback`: the relay's stored chain rolled back to the last block of the
//! round before the requested height's round, as the reference's `snapshot:rollback` does, with
//! the relay's lock checked and then held for the whole command. Rollback is the only snapshot
//! command: the snapshot formats are not built.

use std::process::ExitCode;

use clap::{Args, Subcommand};
use heartwood_cli::instance::InstanceFlags;

/// A snapshot command.
#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
pub enum SnapshotCommand {
    /// Roll the relay's stored chain back to the last block of a finished round.
    Rollback(RollbackFlags),
}

/// The flags of `snapshot rollback`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Args)]
pub struct RollbackFlags {
    /// The instance.
    #[command(flatten)]
    pub instance: InstanceFlags,
    /// The height to roll back to; its round's first block and the blocks above it go.
    #[arg(long, allow_negative_numbers = true)]
    pub height: Option<i64>,
    /// The number of blocks to roll back, when no height is given.
    #[arg(long, allow_negative_numbers = true)]
    pub number: Option<i64>,
}

/// Run `command`; the process's exit status.
pub fn run(command: SnapshotCommand) -> ExitCode {
    let _ = command;
    todo!("snapshot rollback")
}
