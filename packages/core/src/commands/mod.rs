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
use heartwood_cli::{Instance, Role, process, process_table};

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

/// This program, which a supervisor runs as the node.
pub(crate) fn program() -> Result<PathBuf, String> {
    std::env::current_exe().map_err(|error| format!("the program's path: {error}"))
}

/// The instance of `role` that `flags` name, with the process's environment, home directory and
/// working directory.
pub(crate) fn instance(role: Role, flags: &InstanceFlags) -> Result<Instance, String> {
    let env = |name: &str| std::env::var(name).ok();
    let home = std::env::home_dir().ok_or("HOME is not set")?;
    let cwd = std::env::current_dir().map_err(|error| error.to_string())?;
    let resolved = heartwood_cli::instance::resolve(role, flags, &env, &home, &cwd);
    resolved.map_err(|error| error.to_string())
}

/// A role's process command, as `relay` and `forger` share them.
pub(crate) enum ProcessCommand {
    /// Start under a supervisor, the node run with the flags `args` after `<role> run --token
    /// <token> --network <network>`, as a daemon or not.
    Start(InstanceFlags, Vec<OsString>, bool),
    /// Stop.
    Stop(ProcessFlags),
    /// Restart.
    Restart(ProcessFlags),
    /// The process table's row.
    Status(ProcessFlags),
    /// The log.
    Log(LogFlags),
}

/// Run `command` on `role`'s instance.
pub(crate) fn process_command(role: Role, command: ProcessCommand) -> ExitCode {
    let flags = match &command {
        ProcessCommand::Start(flags, ..) => flags,
        ProcessCommand::Stop(flags)
        | ProcessCommand::Restart(flags)
        | ProcessCommand::Status(flags) => &flags.instance,
        ProcessCommand::Log(flags) => &flags.instance,
    };
    let instance = match instance(role, flags) {
        Ok(instance) => instance,
        Err(error) => return crate::exit(Err(error)),
    };
    let done = match command {
        ProcessCommand::Start(_, args, daemon) => program().and_then(|program| {
            let mut argv: Vec<OsString> = vec![program.into(), role.name().into(), "run".into()];
            argv.extend(
                ["--token", &instance.token, "--network", &instance.network].map(Into::into),
            );
            argv.extend(args);
            process::start(&instance, &argv, &Vec::new(), daemon).map_err(|e| e.to_string())
        }),
        ProcessCommand::Stop(_) => process::stop(&instance).map_err(|e| e.to_string()),
        ProcessCommand::Restart(_) => {
            process::restart(&instance, &Vec::new()).map_err(|e| e.to_string())
        }
        ProcessCommand::Status(_) => process::status(&instance)
            .map(|row| print!("{}", process_table::render(&[row])))
            .map_err(|e| e.to_string()),
        ProcessCommand::Log(log) => {
            process::log(&instance, log.lines, log.error, true).map_err(|e| e.to_string())
        }
    };
    crate::exit(done)
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
