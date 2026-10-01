//! `heartwood env get|list|set|paths`: an instance's environment file and directories.

use std::process::ExitCode;

use clap::{Args, Subcommand};
use heartwood_cli::Role;
use heartwood_cli::instance::{InstanceFlags, resolve};

/// An `env` command.
#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
pub enum EnvCommand {
    /// The value of an environment variable.
    Get(EnvKey),
    /// Every environment variable, in the file's order.
    List(EnvFlags),
    /// Set an environment variable.
    Set(EnvSet),
    /// The instance's directories.
    Paths(EnvFlags),
}

/// The instance an `env` command reads.
#[derive(Debug, Clone, Default, PartialEq, Eq, Args)]
pub struct EnvFlags {
    /// The instance.
    #[command(flatten)]
    pub instance: InstanceFlags,
}

/// The flags of `env get`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Args)]
pub struct EnvKey {
    /// The instance.
    #[command(flatten)]
    pub instance: InstanceFlags,
    /// The variable's name.
    #[arg(long)]
    pub key: String,
}

/// The flags of `env set`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Args)]
pub struct EnvSet {
    /// The instance.
    #[command(flatten)]
    pub instance: InstanceFlags,
    /// The variable's name.
    #[arg(long, allow_negative_numbers = true)]
    pub key: Option<String>,
    /// Its new value.
    #[arg(long, allow_negative_numbers = true)]
    pub value: Option<String>,
}

/// Run `command`.
pub fn run(command: EnvCommand) -> ExitCode {
    use heartwood_cli::env as e;
    let (flags, act): (_, e::Action) = match command {
        EnvCommand::Get(k) => (k.instance, Box::new(move |i| e::get_text(i, &k.key))),
        EnvCommand::List(f) => (f.instance, Box::new(e::list_text)),
        EnvCommand::Paths(f) => (f.instance, Box::new(e::paths_text)),
        EnvCommand::Set(s) => {
            let Some(key) = s.key else {
                return crate::exit(Err(e::EnvError::Required("key")));
            };
            let Some(value) = s.value else {
                return crate::exit(Err(e::EnvError::Required("value")));
            };
            (s.instance, Box::new(move |i| e::set_text(i, &key, &value)))
        }
    };
    let env = |name: &str| std::env::var(name).ok();
    let (home, cwd) = (
        std::env::home_dir().unwrap_or_default(),
        std::env::current_dir().unwrap_or_default(),
    );
    let instance = resolve(Role::Relay, &flags, &env, &home, &cwd).map_err(|e| e.to_string());
    crate::exit(
        instance
            .and_then(|i| act(&i).map_err(|e| e.to_string()))
            .map(|text| print!("{text}")),
    )
}
