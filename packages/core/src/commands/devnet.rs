//! `heartwood devnet ...`: a local devnet on this host, mode B (one loopback address per node).

use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use clap::{Args, Subcommand};
use heartwood_cli::devnet::{self, Failover, InitOptions, LogStream, NodeSelection, Tools};
use heartwood_cli::http::LoopbackApi;
use heartwood_cli::network::NetworkGenerator;
use heartwood_cli::process::Supervisors;
use heartwood_cli::{DevnetError, Role, process_table};
use heartwood_kernel::SystemClock;

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

/// How long each step of a read of a relay's API may take.
const API_TIMEOUT: Duration = Duration::from_secs(2);

impl Nodes {
    fn selection(&self) -> NodeSelection {
        NodeSelection {
            nodes: self.node.clone(),
            role: self.role,
        }
    }
}

impl Init {
    /// The options of `devnet init` these flags give, over the defaults.
    fn options(&self) -> Result<InitOptions, String> {
        let defaults = InitOptions::default();
        let address_prefix = match &self.address_prefix {
            None => defaults.address_prefix,
            Some(text) => prefix(text)?,
        };
        Ok(InitOptions {
            validators: self.validators,
            relays: self.relays,
            seed: self.seed.clone().unwrap_or(defaults.seed),
            address_prefix,
            failover: self.failover.clone(),
            epoch_delay: self.epoch_delay.unwrap_or(defaults.epoch_delay),
            min_network_reach: self.min_network_reach.unwrap_or(defaults.min_network_reach),
            log_level: self.log_level.clone().unwrap_or(defaults.log_level),
            force: self.force,
        })
    }
}

/// The address prefix of `text`, three bytes such as `127.0.0`.
fn prefix(text: &str) -> Result<[u8; 3], String> {
    let invalid = || format!("{text:?} is not an address prefix such as 127.0.0");
    let bytes: Vec<u8> = (text.split('.'))
        .map(|part| part.parse().map_err(|_| invalid()))
        .collect::<Result<_, _>>()?;
    bytes.try_into().map_err(|_| invalid())
}

/// Run `command`.
pub fn run(command: DevnetCommand) -> ExitCode {
    let binary = match super::program() {
        Ok(binary) => binary,
        Err(error) => return crate::exit(Err(error)),
    };
    let ops = Supervisors { binary };
    let api = LoopbackApi {
        timeout: API_TIMEOUT,
    };
    let tools = Tools {
        ops: &ops,
        api: &api,
        clock: &SystemClock,
        pause: &std::thread::sleep,
    };
    crate::exit(devnet_command(command, &tools))
}

fn devnet_command(command: DevnetCommand, tools: &Tools<'_>) -> Result<(), String> {
    let text = |error: DevnetError| error.to_string();
    let mut out = std::io::stdout().lock();
    match command {
        DevnetCommand::Init(init) => {
            let options = init.options()?;
            let manifest = devnet::init(&init.root.dir, &options, &NetworkGenerator, tools);
            let manifest = manifest.map_err(text)?;
            let (nodes, epoch) = (manifest.nodes.len(), &manifest.epoch);
            println!(
                "Created a devnet of {nodes} nodes in {}",
                init.root.dir.display()
            );
            println!("  nethash {}, epoch {epoch}", manifest.nethash);
            Ok(())
        }
        DevnetCommand::Start(nodes) => {
            devnet::start(&nodes.root.dir, &nodes.selection(), tools).map_err(text)
        }
        DevnetCommand::Stop(stop) => {
            let (root, selection) = (&stop.nodes.root.dir, stop.nodes.selection());
            devnet::stop(root, &selection, stop.kill, tools).map_err(text)
        }
        DevnetCommand::Status(root) => {
            let status = devnet::status(&root.dir, tools).map_err(text)?;
            let rows = status.nodes.iter().flat_map(|node| {
                let forger = node.forger.iter().cloned();
                std::iter::once(node.relay.clone()).chain(forger)
            });
            print!("{}", process_table::render(&rows.collect::<Vec<_>>()));
            for node in &status.nodes {
                let height = node.api.as_ref().map(|api| api.last_block.height);
                let height = height.map_or("no answer".to_owned(), |h| format!("height {h}"));
                let peers = node.api.as_ref().map_or(0, |api| api.peers);
                println!("node{}: {height}, {peers} peers", node.node);
            }
            println!("{status}");
            Ok(())
        }
        DevnetCommand::Logs(logs) => {
            let stream = match logs.error {
                true => LogStream::Error,
                false => LogStream::File,
            };
            let selection = logs.nodes.selection();
            let root = &logs.nodes.root.dir;
            devnet::logs(root, &selection, stream, logs.lines, &mut out).map_err(text)
        }
        DevnetCommand::StopNode(node) => {
            let (root, k) = (&node.root.dir, node.node);
            devnet::stop_node(root, k, node.role, node.kill, tools).map_err(text)
        }
        DevnetCommand::StartNode(node) => {
            devnet::start_node(&node.root.dir, node.node, node.role, tools).map_err(text)
        }
        DevnetCommand::RestartNode(node) => {
            devnet::restart_node(&node.root.dir, node.node, node.role, tools).map_err(text)
        }
        DevnetCommand::Reset(node) => devnet::reset(&node.root.dir, node.node, tools).map_err(text),
        DevnetCommand::Wait(wait) => {
            let timeout = Duration::from_secs(wait.timeout);
            let height = devnet::wait(&wait.root.dir, wait.height, wait.node, timeout, tools);
            println!("height {}", height.map_err(text)?);
            Ok(())
        }
        DevnetCommand::Env(node) => {
            for (name, value) in devnet::env(&node.root.dir, node.node).map_err(text)? {
                let line = format!("{}={}", name.to_string_lossy(), value.to_string_lossy());
                writeln!(out, "{line}").map_err(|error| error.to_string())?;
            }
            Ok(())
        }
    }
}
