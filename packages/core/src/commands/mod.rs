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
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use heartwood_cli::instance::InstanceFlags;
use heartwood_cli::{Instance, ProcessError, Role, process, process_table};

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
    Devnet(devnet::Devnet),
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

/// This program, which a supervisor runs as the node.
pub(crate) fn program() -> Result<PathBuf, String> {
    std::env::current_exe().map_err(|error| format!("the program's path: {error}"))
}

/// Run `act` on the instance of `role` that `flags` name, resolved with the process's
/// environment, home directory and working directory.
pub(crate) fn on_instance(
    role: Role,
    flags: &InstanceFlags,
    act: impl FnOnce(&Instance) -> Result<(), ProcessError>,
) -> ExitCode {
    let env = |name: &str| std::env::var(name).ok();
    let (home, cwd) = (std::env::home_dir(), std::env::current_dir());
    let (home, cwd) = (home.unwrap_or_default(), cwd.unwrap_or_default());
    let instance = heartwood_cli::instance::resolve(role, flags, &env, &home, &cwd);
    crate::exit(
        instance
            .map_err(|e| e.to_string())
            .and_then(|instance| act(&instance).map_err(|e| e.to_string())),
    )
}

/// Start `instance` under its supervisor, the node run as `<program> <role> run --token <token>
/// --network <network>` and `args`, as a daemon or not.
pub(crate) fn start(
    instance: &Instance,
    args: Vec<OsString>,
    daemon: bool,
) -> Result<(), ProcessError> {
    let spawn = |message| ProcessError::Spawn {
        program: "heartwood".into(),
        message,
    };
    let (token, network) = (&instance.token, &instance.network);
    let mut argv = vec![
        program().map_err(spawn)?.into(),
        instance.role.name().into(),
    ];
    argv.extend(["run", "--token", token, "--network", network].map(OsString::from));
    argv.extend(args);
    process::start(instance, &argv, &Vec::new(), daemon)
}

/// Print the process table's row of `instance`.
pub(crate) fn status(instance: &Instance) -> Result<(), ProcessError> {
    print!("{}", process_table::render(&[process::status(instance)?]));
    Ok(())
}

/// The commands of a started role, as `relay` and `forger` share them.
#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
pub enum Manage {
    /// Stop the role.
    Stop(ProcessFlags),
    /// Restart the role.
    Restart(ProcessFlags),
    /// The role's process status.
    Status(ProcessFlags),
    /// The role's log.
    Log(LogFlags),
}

/// Stop, restart, show or tail `role`'s instance as `command` asks.
pub(crate) fn manage(role: Role, command: Manage) -> ExitCode {
    match command {
        Manage::Stop(flags) => on_instance(role, &flags.instance, process::stop),
        Manage::Restart(flags) => {
            on_instance(role, &flags.instance, |i| process::restart(i, &Vec::new()))
        }
        Manage::Status(flags) => on_instance(role, &flags.instance, status),
        Manage::Log(log) => on_instance(role, &log.instance, |i| {
            process::log(i, log.lines, log.error, true)
        }),
    }
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
