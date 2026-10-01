//! The command tree of the `heartwood` binary, one module per family: [`relay`] and [`forger`]
//! (`run`, `start`, `stop`, `restart`, `status`, `log`), [`network`] (`generate`), `devnet`
//! (feature `devnet`: `init`, `start`, `stop`, `status`, `logs`, `stop-node`, `start-node`,
//! `restart-node`, `reset`, `wait`, `env`), [`snapshot`] (`rollback`), [`clear`] (`pool clear` and
//! `state clear`), [`doctor`] (read-only diagnostics, `checks`, `explain`, `mcp`), [`top`] (the
//! token's running instances), [`mod@env`] (`get`, `list`, `set`, `paths`), [`config`] (`publish`,
//! `forger`), [`service`] (`install`, `uninstall`), [`mod@core`] (the combined role's `run`, `start`,
//! `stop`, `restart`, `status`, `log`) and [`supervise`] (the hidden supervisor), with `version`
//! (`--verbose`) and clap's `help`. A command that is not built yet says so and exits with 1. The reference's colon spellings
//! (`relay:start`) are rewritten first ([`heartwood_cli::argv::rewrite`]); flags keep the
//! reference's camelCase with kebab-case
//! aliases.

pub mod clear;
pub mod config;
pub mod core;
#[cfg(feature = "devnet")]
pub mod devnet;
pub mod doctor;
pub mod env;
pub mod forger;
pub mod network;
pub mod relay;
pub mod service;
pub mod snapshot;
pub mod supervise;
pub mod top;

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{ArgAction, Args, Parser, Subcommand};
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
    /// The relay's stored chain: `snapshot rollback`.
    #[command(subcommand)]
    Snapshot(snapshot::SnapshotCommand),
    /// The relay's pool: `pool clear`.
    #[command(subcommand)]
    Pool(clear::PoolCommand),
    /// The relay's state: `state clear`.
    #[command(subcommand)]
    State(clear::StateCommand),
    /// Read-only diagnostics of this host's node.
    Doctor(doctor::DoctorLine),
    /// The token's running instances.
    Top(top::Top),
    /// An instance's environment file and directories.
    #[command(subcommand)]
    Env(env::EnvCommand),
    /// An instance's configuration files and the forger's key.
    #[command(subcommand)]
    Config(config::ConfigCommand),
    /// A systemd unit for one role of an instance.
    #[command(subcommand)]
    Service(service::ServiceCommand),
    /// The combined role's commands.
    #[command(subcommand)]
    Core(core::CoreCommand),
    /// Supervise a started role (written by `start`).
    #[command(hide = true)]
    Supervise(supervise::Supervise),
    /// Print the version.
    Version {
        /// Also the target and the enabled features.
        #[arg(long)]
        verbose: bool,
    },
}

/// A role command was refused before its process operation.
#[derive(Debug, thiserror::Error)]
pub(crate) enum RoleError {
    /// A process operation failed.
    #[error(transparent)]
    Process(#[from] ProcessError),
    /// The signing keys could not be loaded.
    #[error(transparent)]
    Keys(#[from] heartwood_forger::KeysError),
}

/// The flags of `stop`: the instance and an accepted compatibility flag.
#[derive(Debug, Clone, Default, PartialEq, Eq, Args)]
pub struct StopFlags {
    /// The instance.
    #[command(flatten)]
    pub process: ProcessFlags,
    /// Accepted for compatibility; stopping always ends the supervisor.
    #[arg(
        long, default_value_t = false, action = ArgAction::Set,
        num_args = 0..=1, default_missing_value = "true"
    )]
    pub daemon: bool,
}

/// The flags of `restart` and `status`: the instance.
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
pub(crate) fn on_instance<E: std::fmt::Display>(
    role: Role,
    flags: &InstanceFlags,
    act: impl FnOnce(&Instance) -> Result<(), E>,
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
    Stop(StopFlags),
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
        Manage::Stop(flags) => on_instance(role, &flags.process.instance, process::stop),
        Manage::Restart(flags) => {
            on_instance(role, &flags.instance, |i| process::restart(i, &Vec::new()))
        }
        Manage::Status(flags) => on_instance(role, &flags.instance, status),
        Manage::Log(log) => on_instance(role, &log.instance, |i| {
            process::log(i, log.lines, log.error, true)
        }),
    }
}

/// The answer of a command that is not built yet: the message, and the exit status 1.
pub(crate) fn not_built(what: &str) -> ExitCode {
    crate::exit(Err(format!("{what} is not built yet")))
}

/// The version, with the platform this build runs on and the features it was built with (a test
/// seam is named only in a build that has it).
pub(crate) fn verbose_version() -> String {
    let features = [
        ("devnet", cfg!(feature = "devnet")),
        ("difftest", cfg!(feature = "difftest")),
    ];
    let mut on = features.iter().filter(|(_, on)| *on).map(|(name, _)| *name);
    let listed = on.next().map_or_else(
        || "none".to_owned(),
        |first| on.fold(first.to_owned(), |all, name| format!("{all}, {name}")),
    );
    let (arch, os) = (std::env::consts::ARCH, std::env::consts::OS);
    format!(
        "{}\ntarget: {arch}-{os}\nfeatures: {listed}",
        crate::VERSION
    )
}

/// Run `command`; the process's exit status.
pub fn run(command: Command) -> ExitCode {
    match command {
        Command::Relay(command) => relay::run(command),
        Command::Forger(command) => forger::run(command),
        Command::Network(command) => network::run(command),
        #[cfg(feature = "devnet")]
        Command::Devnet(command) => devnet::run(command),
        Command::Snapshot(command) => snapshot::run(command),
        Command::Pool(command) => clear::run_pool(command),
        Command::State(command) => clear::run_state(command),
        Command::Doctor(line) => doctor::run(line),
        Command::Top(top) => top::run(top),
        Command::Env(command) => env::run(command),
        Command::Config(command) => config::run(command),
        Command::Service(command) => service::run(command),
        Command::Core(command) => core::run(command),
        Command::Supervise(command) => supervise::run(command),
        Command::Version { verbose: false } => {
            println!("{}", crate::VERSION);
            ExitCode::SUCCESS
        }
        Command::Version { verbose: true } => {
            println!("{}", verbose_version());
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

    /// Whether `relay start` and `forger start` with `flags` run as a daemon.
    fn daemon(role: &str, flags: &str) -> bool {
        let text = format!("heartwood {role} start --network devnet {flags}");
        match line(&text).unwrap() {
            Command::Relay(relay::RelayCommand::Start(start)) => start.daemon,
            Command::Forger(forger::ForgerCommand::Start(start)) => start.daemon,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_verbose_version_has_the_version_the_target_and_the_features_of_this_build() {
        let lines: Vec<String> = verbose_version().lines().map(str::to_owned).collect();
        assert_eq!(lines[0], crate::VERSION);
        assert!(lines[1].starts_with("target: "), "{lines:?}");
        let features = lines[2].strip_prefix("features: ").unwrap();
        assert_eq!(features.contains("devnet"), cfg!(feature = "devnet"));
        assert_eq!(features.contains("difftest"), cfg!(feature = "difftest"));
        assert_eq!(lines.len(), 3);
    }

    #[test]
    fn start_is_a_daemon_unless_the_reference_spellings_of_the_flag_say_otherwise() {
        for role in ["relay", "forger"] {
            let on = ["", "--daemon", "--daemon=true", "--daemon true"];
            for flags in on.into_iter().chain(["--daemon --token solar"]) {
                assert!(daemon(role, flags), "{role} {flags}");
            }
            let off = ["--daemon=false", "--no-daemon", "--daemon false"];
            for flags in off.into_iter().chain(["--daemon false --token solar"]) {
                assert!(!daemon(role, flags), "{role} {flags}");
            }
        }
    }

    #[test]
    fn compatibility_boolean_flags_accept_all_value_forms_only_on_their_commands() {
        for role in ["core", "forger"] {
            for verb in ["run", "start"] {
                for flag in ["--skipPrompts", "--skip-prompts"] {
                    for value in ["", "=true", "=false", " true", " false"] {
                        let command = format!("heartwood {role}:{verb} {flag}{value}");
                        assert!(line(&command).is_ok(), "{command}");
                    }
                }
            }
        }
        for role in ["relay", "forger", "core"] {
            for flag in [
                "--daemon",
                "--daemon=true",
                "--daemon=false",
                "--daemon true",
                "--daemon false",
                "--no-daemon",
            ] {
                let command = format!("heartwood {role}:stop {flag}");
                assert!(line(&command).is_ok(), "{command}");
                for verb in ["restart", "status"] {
                    assert!(line(&format!("heartwood {role} {verb} {flag}")).is_err());
                }
            }
        }
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
            "heartwood version --verbose",
            "heartwood top --token node1",
            "heartwood env get --network devnet --key CORE_DB_HOST",
            "heartwood env set --network devnet --key CORE_LOG_LEVEL --value debug",
            "heartwood env list --network devnet",
            "heartwood env paths --token node1 --network devnet",
            "heartwood config publish --network devnet --force",
            "heartwood config forger --network devnet --method privateKey",
            "heartwood config forger private-key --network devnet",
            "heartwood service install --network devnet --role core --system --out o",
            "heartwood service uninstall --network devnet --role relay",
            "heartwood core run --token node1 --network devnet --networkStart",
            "heartwood core start --network devnet --daemon=false",
            "heartwood core stop --network devnet",
            "heartwood core log --network devnet --lines 5",
            "heartwood supervise --role core --token t --network n -- heartwood core run",
        ] {
            assert!(line(text).is_ok(), "{text}");
        }
        #[cfg(feature = "devnet")]
        for text in [
            "heartwood devnet init --validators 1 --failover 1:1,6,7",
            "heartwood devnet stop-node 3 --role relay --kill",
            "heartwood devnet wait --height 20",
            "heartwood devnet init --validators 3 --core",
            "heartwood devnet start --keep-epoch --no-wait",
            "heartwood devnet status --json",
            "heartwood devnet status --resources",
            "heartwood devnet logs --follow --node 2",
            "heartwood devnet run s.json -- --seed 1",
        ] {
            assert!(line(text).is_ok(), "{text}");
        }
        assert!(line("heartwood relay run --colour red").is_err());
    }
}
