//! `heartwood relay run|start|stop|restart|status|log`.

use std::ffi::OsString;
#[cfg(feature = "difftest")]
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Subcommand};
use heartwood_cli::Role;
use heartwood_cli::instance::InstanceFlags;

use super::{LogFlags, ProcessCommand, ProcessFlags, process_command};

/// A relay's command.
#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
pub enum RelayCommand {
    /// Run a relay in the foreground.
    Run(RelayRun),
    /// Start a relay under its supervisor.
    Start(RelayStart),
    /// Stop a relay.
    Stop(ProcessFlags),
    /// Restart a relay.
    Restart(ProcessFlags),
    /// A relay's process status.
    Status(ProcessFlags),
    /// A relay's log.
    Log(LogFlags),
}

/// The flags of `relay run`, the reference's in camelCase with kebab-case aliases.
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

impl RelayRun {
    /// The instance these flags name.
    pub fn instance(&self) -> InstanceFlags {
        InstanceFlags {
            token: self.token.clone(),
            network: self.network.clone(),
        }
    }

    /// The flags besides the instance's, as `relay run` takes them: what a supervisor passes on.
    pub fn args(&self) -> Vec<OsString> {
        let mut args: Vec<OsString> = Vec::new();
        let mut value = |flag: &str, value: &Option<String>| {
            if let Some(value) = value {
                args.extend([flag.into(), value.into()]);
            }
        };
        value("--env", &self.env);
        value("--launchMode", &self.launch_mode);
        #[cfg(feature = "difftest")]
        {
            let path = |path: &Option<PathBuf>| path.as_ref().map(|p| p.display().to_string());
            value("--difftest-digest", &path(&self.difftest_digest));
            value("--difftest-blocks", &path(&self.difftest_blocks));
            value("--difftest-seed", &self.difftest_seed);
        }
        let switches = [
            ("--networkStart", self.network_start),
            ("--disableDiscovery", self.disable_discovery),
            ("--skipDiscovery", self.skip_discovery),
            (
                "--ignoreMinimumNetworkReach",
                self.ignore_minimum_network_reach,
            ),
        ];
        let on = switches.into_iter().filter(|(_, on)| *on);
        args.extend(on.map(|(flag, _)| flag.into()));
        args
    }
}

/// The flags of `relay start`: those of `relay run`, and whether to leave the terminal.
#[derive(Debug, Clone, Default, PartialEq, Eq, Args)]
pub struct RelayStart {
    /// The relay's flags.
    #[command(flatten)]
    pub run: RelayRun,
    /// Run the supervisor as a daemon.
    #[arg(long)]
    pub daemon: bool,
}

/// Run `command`.
pub fn run(command: RelayCommand) -> ExitCode {
    let process = match command {
        RelayCommand::Run(run) => return crate::exit(crate::app::run_relay(&run)),
        RelayCommand::Start(start) => {
            ProcessCommand::Start(start.run.instance(), start.run.args(), start.daemon)
        }
        RelayCommand::Stop(flags) => ProcessCommand::Stop(flags),
        RelayCommand::Restart(flags) => ProcessCommand::Restart(flags),
        RelayCommand::Status(flags) => ProcessCommand::Status(flags),
        RelayCommand::Log(flags) => ProcessCommand::Log(flags),
    };
    process_command(Role::Relay, process)
}
