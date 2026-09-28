//! `heartwood devnet ...`: a local devnet on this host, mode B (one loopback address per node).

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use clap::{Args, Subcommand};
use heartwood_cli::devnet::{self, Failover, InitOptions, LogStream, NodeSelection, Tools};
use heartwood_cli::http::LoopbackApi;
use heartwood_cli::network::NetworkGenerator;
use heartwood_cli::process::Supervisors;
use heartwood_cli::{DevnetError, Role, process_table};
use heartwood_kernel::SystemClock;

/// `devnet`: the devnet's directory, and a command on it.
#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct Devnet {
    /// The devnet's directory.
    #[arg(long, global = true, default_value = "devnet")]
    pub dir: PathBuf,
    /// The command.
    #[command(subcommand)]
    pub command: DevnetCommand,
}

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
    Status,
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

/// The flags of `init`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Args)]
pub struct Init {
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
    #[arg(long, value_parser = prefix)]
    pub address_prefix: Option<[u8; 3]>,
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

/// How long each step of a read of a relay's API may take.
const API_TIMEOUT: Duration = Duration::from_secs(2);

impl Nodes {
    fn selection(&self) -> NodeSelection {
        let (nodes, role) = (self.node.clone(), self.role);
        NodeSelection { nodes, role }
    }
}

impl Init {
    /// The options of `devnet init` these flags give, over the defaults.
    fn options(&self) -> InitOptions {
        let defaults = InitOptions::default();
        InitOptions {
            validators: self.validators,
            relays: self.relays,
            seed: self.seed.clone().unwrap_or(defaults.seed),
            address_prefix: self.address_prefix.unwrap_or(defaults.address_prefix),
            failover: self.failover.clone(),
            epoch_delay: self.epoch_delay.unwrap_or(defaults.epoch_delay),
            min_network_reach: self.min_network_reach.unwrap_or(defaults.min_network_reach),
            log_level: self.log_level.clone().unwrap_or(defaults.log_level),
            force: self.force,
        }
    }
}

/// The address prefix of `text`, three bytes such as `127.0.0`.
fn prefix(text: &str) -> Result<[u8; 3], String> {
    let bytes: Result<Vec<u8>, _> = text.split('.').map(str::parse).collect();
    let prefix = bytes.ok().and_then(|bytes| bytes.try_into().ok());
    prefix.ok_or_else(|| format!("{text:?} is not an address prefix such as 127.0.0"))
}

/// Run `command`.
pub fn run(devnet: Devnet) -> ExitCode {
    crate::exit(super::program().and_then(|binary| {
        let (ops, api) = (
            Supervisors { binary },
            LoopbackApi {
                timeout: API_TIMEOUT,
            },
        );
        let (clock, pause) = (&SystemClock, &std::thread::sleep);
        let tools = Tools {
            ops: &ops,
            api: &api,
            clock,
            pause,
        };
        devnet_command(&devnet.dir, devnet.command, &tools).map_err(|error| error.to_string())
    }))
}

fn devnet_command(dir: &Path, command: DevnetCommand, tools: &Tools) -> Result<(), DevnetError> {
    match command {
        DevnetCommand::Init(init) => {
            let options = init.options();
            let made = devnet::init(dir, &options, &NetworkGenerator, tools)?;
            let (dir, count) = (dir.display(), made.nodes.len());
            println!(
                "Created a devnet of {count} nodes in {dir}, nethash {}",
                made.nethash
            );
        }
        DevnetCommand::Start(n) => devnet::start(dir, &n.selection(), tools)?,
        DevnetCommand::Stop(s) => {
            devnet::stop(dir, &s.nodes.selection(), s.kill, tools)?;
        }
        DevnetCommand::Status => {
            let status = devnet::status(dir, tools)?;
            let rows =
                (status.nodes.iter()).flat_map(|n| std::iter::once(&n.relay).chain(&n.forger));
            print!(
                "{}",
                process_table::render(&rows.cloned().collect::<Vec<_>>())
            );
            for node in &status.nodes {
                let height = node.api.as_ref().map(|api| api.last_block.height);
                println!("node{}: height {height:?}", node.node);
            }
            println!("{status}");
        }
        DevnetCommand::Logs(l) => {
            let stream = if l.error {
                LogStream::Error
            } else {
                LogStream::File
            };
            let out = &mut std::io::stdout().lock();
            devnet::logs(dir, &l.nodes.selection(), stream, l.lines, out)?;
        }
        DevnetCommand::StopNode(n) => devnet::stop_node(dir, n.node, n.role, n.kill, tools)?,
        DevnetCommand::StartNode(n) => devnet::start_node(dir, n.node, n.role, tools)?,
        DevnetCommand::RestartNode(n) => devnet::restart_node(dir, n.node, n.role, tools)?,
        DevnetCommand::Reset(n) => devnet::reset(dir, n.node, tools)?,
        DevnetCommand::Wait(w) => {
            let timeout = Duration::from_secs(w.timeout);
            println!(
                "height {}",
                devnet::wait(dir, w.height, w.node, timeout, tools)?
            );
        }
        DevnetCommand::Env(n) => {
            for (name, value) in devnet::env(dir, n.node)? {
                println!("{}={}", name.to_string_lossy(), value.to_string_lossy());
            }
        }
    }
    Ok(())
}
