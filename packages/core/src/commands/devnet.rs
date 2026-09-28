//! `heartwood devnet ...`: a local devnet on this host, mode B (one loopback address per node).

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Subcommand};
use heartwood_cli::Role;
use heartwood_cli::devnet::Failover;

/// A devnet command.
#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
pub enum DevnetCommand {
    /// Create a devnet's files.
    Init(Init),
    /// Start the devnet's nodes.
    Start(Nodes),
    /// Stop the devnet's nodes.
    Stop(Stop),
    /// The devnet's processes, heights and quorum.
    Status(Root),
    /// The nodes' logs, merged.
    Logs(Logs),
    /// Stop one node.
    StopNode(Node),
    /// Start one node.
    StartNode(Node),
    /// Restart one node.
    RestartNode(Node),
    /// Delete one node's chain, logs and caches.
    Reset(Node),
    /// Wait until the relays reach a height.
    Wait(Wait),
    /// Print one node's environment.
    Env(Node),
}

/// The devnet's directory.
#[derive(Debug, Clone, Default, PartialEq, Eq, Args)]
pub struct Root {
    /// The devnet's directory.
    #[arg(long, default_value = "devnet")]
    pub dir: PathBuf,
}

/// The flags of `init`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Args)]
pub struct Init {
    /// The devnet's directory.
    #[command(flatten)]
    pub root: Root,
    /// The nodes with a relay and a forger.
    #[arg(long, default_value_t = 5)]
    pub validators: u32,
    /// The nodes with a relay only.
    #[arg(long, default_value_t = 0)]
    pub relays: u32,
    /// The chain's key-derivation seed.
    #[arg(long)]
    pub seed: Option<String>,
    /// The first three bytes of every node address, such as `127.0.0`.
    #[arg(long)]
    pub address_prefix: Option<String>,
    /// A forger that fails over, such as `1:1,6,7`.
    #[arg(long)]
    pub failover: Vec<Failover>,
    /// The seconds to the epoch.
    #[arg(long)]
    pub epoch_delay: Option<u32>,
    /// Every relay's minimum network reach.
    #[arg(long)]
    pub min_network_reach: Option<u32>,
    /// Every node's log level.
    #[arg(long)]
    pub log_level: Option<String>,
    /// Replace an existing devnet.
    #[arg(long)]
    pub force: bool,
}

/// The nodes and the role a command acts on.
#[derive(Debug, Clone, Default, PartialEq, Eq, Args)]
pub struct Nodes {
    /// The devnet's directory.
    #[command(flatten)]
    pub root: Root,
    /// The nodes, by number; every node when none.
    #[arg(long)]
    pub node: Vec<u32>,
    /// One role only.
    #[arg(long)]
    pub role: Option<Role>,
}

/// The flags of `stop`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Args)]
pub struct Stop {
    /// The nodes.
    #[command(flatten)]
    pub nodes: Nodes,
    /// SIGKILL instead of a graceful stop.
    #[arg(long)]
    pub kill: bool,
}

/// The flags of `logs`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Args)]
pub struct Logs {
    /// The nodes.
    #[command(flatten)]
    pub nodes: Nodes,
    /// The lines of each log.
    #[arg(long, default_value_t = heartwood_cli::process::LOG_LINES)]
    pub lines: usize,
    /// The error captures instead of the logs.
    #[arg(long)]
    pub error: bool,
}

/// One node.
#[derive(Debug, Clone, Default, PartialEq, Eq, Args)]
pub struct Node {
    /// The devnet's directory.
    #[command(flatten)]
    pub root: Root,
    /// The node, by number.
    pub node: u32,
    /// One role only.
    #[arg(long)]
    pub role: Option<Role>,
    /// SIGKILL instead of a graceful stop.
    #[arg(long)]
    pub kill: bool,
}

/// The flags of `wait`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Args)]
pub struct Wait {
    /// The devnet's directory.
    #[command(flatten)]
    pub root: Root,
    /// The height to reach.
    #[arg(long)]
    pub height: u32,
    /// One node only.
    #[arg(long)]
    pub node: Option<u32>,
    /// The longest wait, in seconds.
    #[arg(long, default_value_t = 300)]
    pub timeout: u64,
}

/// Run `command`.
pub fn run(command: DevnetCommand) -> ExitCode {
    let _ = command;
    todo!("the devnet's commands")
}
