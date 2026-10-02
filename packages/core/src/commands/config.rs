//! `heartwood config publish|forger`: an instance's configuration from the built-in template, and
//! the forger's key.

use std::process::ExitCode;

use clap::{Args, Subcommand};
use heartwood_cli::Role;
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
    use heartwood_cli::config as c;
    match command {
        ConfigCommand::Publish(p) => super::on_instance(Role::Relay, &p.instance, |i| {
            c::publish(i, p.force).map(|done| print!("{done}"))
        }),
        ConfigCommand::Forger(f) => {
            let (flags, method, key) = match f.with {
                Some(ForgerMethod::PrivateKey(k)) => {
                    let flags = c::merged(k.instance, f.instance);
                    (flags, None, k.private_key.or(f.private_key))
                }
                None => (
                    f.instance,
                    f.method.or(f.bip39.map(|_| "bip39".into())),
                    f.private_key,
                ),
            };
            super::on_instance(Role::Relay, &flags, |i| {
                let wrote = c::forger(i, method.as_deref(), key.as_deref());
                wrote.map(|file| println!("Wrote the private key to {}", file.display()))
            })
        }
    }
}
