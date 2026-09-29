//! `heartwood network generate`: a network's chain definition, its keys and a single node's
//! files, from the devnet profile or a profile file.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Subcommand};
use heartwood_cli::{GenerateOptions, Profile};
use heartwood_kernel::SystemClock;

/// A network command.
#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
pub enum NetworkCommand {
    /// Generate a network's files.
    Generate(Generate),
}

/// The flags of `network generate`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Args)]
pub struct Generate {
    /// The network's name.
    #[arg(long)]
    pub network: String,
    /// The key-derivation seed.
    #[arg(long)]
    pub seed: String,
    /// The epoch, such as `2026-01-01T00:00:00.000Z`; the current second when absent.
    #[arg(long)]
    pub epoch: Option<String>,
    /// A profile file instead of the devnet profile.
    #[arg(long)]
    pub profile: Option<PathBuf>,
    /// The address byte.
    #[arg(long = "pubKeyHash", alias = "pub-key-hash")]
    pub pub_key_hash: Option<u8>,
    /// The token's name.
    #[arg(long)]
    pub token: Option<String>,
    /// The token's symbol.
    #[arg(long)]
    pub symbol: Option<String>,
    /// The explorer's URL.
    #[arg(long)]
    pub explorer: Option<String>,
    /// The output directory.
    #[arg(long)]
    pub out: PathBuf,
    /// Write the chain definition and keys only.
    #[arg(long = "chainOnly", alias = "chain-only")]
    pub chain_only: bool,
    /// Overwrite a directory that is not empty.
    #[arg(long)]
    pub force: bool,
}

impl Generate {
    /// The options of `network generate` these flags give.
    pub fn options(self) -> GenerateOptions {
        GenerateOptions {
            network: self.network,
            seed: self.seed,
            epoch: self.epoch,
            profile: self.profile.map_or(Profile::Devnet, Profile::File),
            pub_key_hash: self.pub_key_hash,
            token: self.token,
            symbol: self.symbol,
            explorer: self.explorer,
            out: self.out,
            chain_only: self.chain_only,
            force: self.force,
        }
    }
}

/// Run `command`.
pub fn run(command: NetworkCommand) -> ExitCode {
    let NetworkCommand::Generate(generate) = command;
    let generated = heartwood_cli::network::generate(&generate.options(), &SystemClock);
    crate::exit(generated.map(|generated| {
        let (out, files) = (generated.out.display(), generated.files.len());
        let (nethash, genesis) = (&generated.nethash, &generated.genesis_id);
        println!("Wrote {files} files to {out}: nethash {nethash}, genesis block {genesis}");
    }))
}
