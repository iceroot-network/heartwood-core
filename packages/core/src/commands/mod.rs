//! The command tree of the `heartwood` binary, one module per family: [`relay`] and [`forger`]
//! (`run`, `start`, `stop`, `restart`, `status`, `log`), [`network`] (`generate`), `devnet`
//! (feature `devnet`: `init`, `start`, `stop`, `status`, `logs`, `stop-node`, `start-node`,
//! `restart-node`, `reset`, `wait`, `env`) and [`supervise`] (the hidden supervisor), with
//! `version` and clap's `help`. The reference's colon spellings (`relay:start`) are rewritten
//! first ([`heartwood_cli::argv::rewrite`]); flags keep the reference's camelCase with kebab-case
//! aliases.

#[cfg(feature = "devnet")]
pub mod devnet;
pub mod forger;
pub mod network;
pub mod relay;
pub mod supervise;

use std::ffi::OsString;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use heartwood_cli::instance::InstanceFlags;

/// The parsed command line.
#[derive(Debug, Clone, PartialEq, Eq, Parser)]
#[command(name = "heartwood", version = crate::VERSION, about = "Heartwood Core")]
pub struct Cli {
    /// The command.
    #[command(subcommand)]
    pub command: Command,
}

/// A command.
#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
pub enum Command {
    /// A relay's commands.
    #[command(subcommand)]
    Relay(relay::RelayCommand),
    /// A forger's commands.
    #[command(subcommand)]
    Forger(forger::ForgerCommand),
    /// A network's files.
    #[command(subcommand)]
    Network(network::NetworkCommand),
    /// A local devnet on this host.
    #[cfg(feature = "devnet")]
    #[command(subcommand)]
    Devnet(devnet::DevnetCommand),
    /// Supervise a started role (written by `start`).
    #[command(hide = true)]
    Supervise(supervise::Supervise),
    /// Print the version.
    Version,
}

/// The flags of `stop`, `restart` and `status`: the instance.
#[derive(Debug, Clone, Default, PartialEq, Eq, Args)]
pub struct ProcessFlags {
    /// The instance.
    #[command(flatten)]
    pub instance: InstanceFlags,
}

/// The flags of `log`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Args)]
pub struct LogFlags {
    /// The instance.
    #[command(flatten)]
    pub instance: InstanceFlags,
    /// The lines printed before following.
    #[arg(long, default_value_t = heartwood_cli::process::LOG_LINES)]
    pub lines: usize,
    /// The error capture instead of the output.
    #[arg(long)]
    pub error: bool,
}

/// Parse `args` (the program first), the colon spellings rewritten.
pub fn parse(args: Vec<OsString>) -> Result<Command, clap::Error> {
    Cli::try_parse_from(heartwood_cli::argv::rewrite(args)).map(|cli| cli.command)
}

/// Run `command`; the process's exit status.
pub fn run(command: Command) -> ExitCode {
    match command {
        Command::Relay(command) => relay::run(command),
        Command::Forger(command) => forger::run(command),
        Command::Network(command) => network::run(command),
        #[cfg(feature = "devnet")]
        Command::Devnet(command) => devnet::run(command),
        Command::Supervise(command) => supervise::run(command),
        Command::Version => {
            println!("{}", crate::VERSION);
            ExitCode::SUCCESS
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(text: &str) -> Result<Command, clap::Error> {
        parse(text.split_whitespace().map(OsString::from).collect())
    }

    #[test]
    fn every_family_parses_and_the_colon_spellings_are_the_same_commands() {
        let run = line("heartwood relay run --token solar --network devnet --networkStart");
        let alias = line("heartwood relay:run --token solar --network devnet --network-start");
        assert_eq!(run.unwrap(), alias.unwrap());
        let start = line("heartwood relay start --network devnet --daemon").unwrap();
        assert_eq!(
            line("heartwood relay:start --network devnet --daemon").unwrap(),
            start
        );
        for text in [
            "heartwood forger run --token node1 --network devnet",
            "heartwood forger:stop --network devnet",
            "heartwood relay log --network devnet --lines 5 --error",
            "heartwood network generate --network devnet --seed s --out o --chainOnly",
            "heartwood supervise --role relay --token t --network n -- heartwood relay run",
            "heartwood version",
        ] {
            assert!(line(text).is_ok(), "{text}");
        }
        #[cfg(feature = "devnet")]
        for text in [
            "heartwood devnet init --validators 1 --failover 1:1,6,7",
            "heartwood devnet stop-node 3 --role relay --kill",
            "heartwood devnet wait --height 20",
        ] {
            assert!(line(text).is_ok(), "{text}");
        }
        assert!(line("heartwood relay run --colour red").is_err());
    }
}
