//! `heartwood config publish|forger`: an instance's configuration from the built-in template, and
//! the forger's key.

use std::process::ExitCode;

use clap::{Args, Subcommand};
use heartwood_cli::instance::InstanceFlags;

/// A `config` command.
#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
pub enum ConfigCommand {
    /// Write the configuration files from the built-in template.
    Publish(Publish),
    /// Configure the forging delegate's key.
    Forger(Forger),
}

/// The flags of `config publish`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Args)]
pub struct Publish {
    /// The instance.
    #[command(flatten)]
    pub instance: InstanceFlags,
    /// Overwrite an existing configuration.
    #[arg(long)]
    pub force: bool,
}

/// The flags of `config forger`, and its methods as subcommands.
#[derive(Debug, Clone, Default, PartialEq, Eq, Args)]
pub struct Forger {
    /// The instance.
    #[command(flatten)]
    pub instance: InstanceFlags,
    /// The configuration method (`privateKey`; `bip39` names its deferral).
    #[arg(long)]
    pub method: Option<String>,
    /// A 64-character hexadecimal private key.
    #[arg(long = "privateKey", alias = "private-key")]
    pub private_key: Option<String>,
    /// A passphrase (the deferred method).
    #[arg(long)]
    pub bip39: Option<String>,
    /// Skip the passphrase's validation (the deferred method).
    #[arg(long = "skipValidation", alias = "skip-validation")]
    pub skip_validation: bool,
    /// A method as its own command (`config forger private-key`).
    #[command(subcommand)]
    pub with: Option<ForgerMethod>,
}

/// `config forger`'s methods as commands.
#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
pub enum ForgerMethod {
    /// Configure the key from a private key.
    PrivateKey(PrivateKey),
}

/// The flags of `config forger private-key`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Args)]
pub struct PrivateKey {
    /// The instance.
    #[command(flatten)]
    pub instance: InstanceFlags,
    /// A 64-character hexadecimal private key.
    #[arg(long = "privateKey", alias = "private-key")]
    pub private_key: Option<String>,
}

/// Run `command`.
pub fn run(command: ConfigCommand) -> ExitCode {
    let what = match command {
        ConfigCommand::Publish(_) => "config publish",
        ConfigCommand::Forger(_) => "config forger",
    };
    super::not_built(what)
}
