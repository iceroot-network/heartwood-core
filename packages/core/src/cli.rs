//! The command line: `heartwood relay run` and the colon alias `heartwood relay:run`, with the
//! reference's flags in camelCase and their kebab-case aliases. `config.json` fills a missing
//! `--token` or `--network` and never overrides a given one ([`crate::app`]).

#[cfg(feature = "difftest")]
use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

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
    Relay(RelayCommand),
    /// Run a relay in the foreground (the reference's `relay:run`).
    #[command(name = "relay:run")]
    RelayRun(RelayRun),
}

/// A relay's command.
#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
pub enum RelayCommand {
    /// Run a relay in the foreground.
    Run(RelayRun),
}

/// The flags of `relay run`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Args)]
pub struct RelayRun {
    /// The token of the node's directories (`solar` by default).
    #[arg(long)]
    pub token: Option<String>,
    /// The network's name.
    #[arg(long)]
    pub network: Option<String>,
    /// The environment's name.
    #[arg(long)]
    pub env: Option<String>,
    /// Start a new network.
    #[arg(long = "networkStart", alias = "network-start")]
    pub network_start: bool,
    /// Accept no peer.
    #[arg(long = "disableDiscovery", alias = "disable-discovery")]
    pub disable_discovery: bool,
    /// Skip the discovery at boot.
    #[arg(long = "skipDiscovery", alias = "skip-discovery")]
    pub skip_discovery: bool,
    /// Ignore the minimum network reach.
    #[arg(
        long = "ignoreMinimumNetworkReach",
        alias = "ignore-minimum-network-reach"
    )]
    pub ignore_minimum_network_reach: bool,
    /// The launch mode (`seed`).
    #[arg(long = "launchMode", alias = "launch-mode")]
    pub launch_mode: Option<String>,
    /// Write one state digest line per applied, reverted and restarted state to this file.
    #[cfg(feature = "difftest")]
    #[arg(long = "difftest-digest")]
    pub difftest_digest: Option<PathBuf>,
    /// Write the bytes of every applied block to this file.
    #[cfg(feature = "difftest")]
    #[arg(long = "difftest-blocks")]
    pub difftest_blocks: Option<PathBuf>,
    /// One seed, as `ip:port`, that skips the address rules (loopback and the host's own addresses
    /// allowed), for a mock peer on the same host.
    #[cfg(feature = "difftest")]
    #[arg(long = "difftest-seed")]
    pub difftest_seed: Option<String>,
}

impl Command {
    /// The flags of the relay run, whichever spelling asked for it.
    pub fn relay_run(&self) -> &RelayRun {
        match self {
            Command::Relay(RelayCommand::Run(run)) | Command::RelayRun(run) => run,
        }
    }
}

/// Parse `args` (with the program name first).
pub fn parse(args: &[String]) -> Result<Command, clap::Error> {
    Cli::try_parse_from(args).map(|cli| cli.command)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_line(line: &str) -> Result<Command, clap::Error> {
        let args: Vec<String> = line.split_whitespace().map(str::to_owned).collect();
        parse(&args)
    }

    #[test]
    fn both_spellings_and_both_flag_forms_parse() {
        let run = parse_line("heartwood relay run --token solar --network testnet --networkStart")
            .unwrap();
        let alias =
            parse_line("heartwood relay:run --token solar --network testnet --network-start")
                .unwrap();
        assert_eq!(run.relay_run(), alias.relay_run());
        assert!(run.relay_run().network_start);
        assert_eq!(run.relay_run().token.as_deref(), Some("solar"));
        assert!(parse_line("heartwood relay run --colour red").is_err());
    }
}
