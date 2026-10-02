//! The lockstep producer (`difftest lockstep make`): a chain forged with Heartwood's genesis, state,
//! pool and forger from a seed, a genesis profile and a [`Recipe`], with every transaction built by
//! the reference and submitted to both pools and every block applied by the reference beside it,
//! written as an ordinary corpus (blocks, manifest with the forger's aux, revert and restart plans)
//! for `corpus judge`, `l2`, `l2-processor`, `l2-restart` and `forge-replay`.
//!
//! A chain is at most 5 rounds unless it is asked for as a long chain, and at most 50 rounds in
//! any case. [`batch`] makes many chains at once, each with a drawn recipe
//! (`difftest lockstep batch`).

pub mod batch;
pub mod heart;
mod labels;
mod output;
mod producer;
pub mod recipe;
pub mod reference;

use std::path::{Path, PathBuf};

use thiserror::Error;

pub use self::recipe::{Attested, BuildSpec, Recipe, Step};
use crate::cli::{LockstepArgs, OracleArgs};
use crate::errors::{Error, Result};

/// The most rounds of a chain that is not a long chain.
pub const SHORT_ROUNDS: u32 = 5;

/// The most rounds of a long chain.
pub const LONG_ROUNDS: u32 = 50;

/// A chain the producer made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MadeChain {
    /// The corpus directory written.
    pub dir: PathBuf,
    /// The last height.
    pub tip: u32,
    /// The transactions the chain holds.
    pub transactions: u64,
    /// The slots missed.
    pub missed: u64,
}

/// Why the producer stopped.
#[derive(Debug, Error)]
pub enum LockstepError {
    /// The recipe cannot be followed (a label without a wallet, a step beyond the chain).
    #[error("the recipe: {0}")]
    Recipe(String),
    /// The oracle failed.
    #[error("the oracle: {0}")]
    Oracle(String),
    /// The reference's digest line of a block differs from Heartwood's, or it refused the block.
    #[error("the reference does not agree at height {height}: {detail}")]
    Block {
        /// The block's height.
        height: u32,
        /// What differs.
        detail: String,
    },
    /// The pools' verdicts on a submitted transaction differ.
    #[error("the pools do not agree on transaction {id}: {detail}")]
    Pool {
        /// The transaction's id.
        id: String,
        /// What differs.
        detail: String,
    },
    /// Heartwood's own code failed.
    #[error("{0}")]
    Heartwood(String),
    /// A file could not be read or written.
    #[error("{path}: {source}")]
    Io {
        /// The file.
        path: PathBuf,
        /// The error.
        source: std::io::Error,
    },
}

/// The chain's epoch: the profile's genesis time, and the first time of the pools' clock.
const EPOCH: &str = "2026-01-01T00:00:00Z";

/// The epoch in milliseconds since 1970.
const EPOCH_MS: i64 = 1_767_225_600_000;

/// The label of the forger's aux.
const AUX_LABEL: &str = "forger";

/// The genesis profile named `profile`: a file, or the name of a profile of the CLI.
fn profile_path(profile: &str) -> PathBuf {
    let direct = PathBuf::from(profile);
    if direct.is_file() {
        return direct;
    }
    let name = profile.strip_suffix(".json").unwrap_or(profile);
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../packages/cli/src/profiles")
        .join(format!("{name}.json"))
}

/// Make the chain of `recipe` for `rounds` rounds from `seed` and the genesis profile `profile`
/// into the corpus directory `out`, with the reference where `oracle` says.
pub fn make(
    seed: &str,
    profile: &str,
    rounds: u32,
    recipe: &Recipe,
    out: &Path,
    oracle: &OracleArgs,
) -> std::result::Result<MadeChain, LockstepError> {
    let oracle_error = |e: Error| LockstepError::Oracle(e.to_string());
    let mut server = oracle
        .resolve(|k| std::env::var(k).ok())
        .and_then(|resolved| resolved.server())
        .map_err(oracle_error)?;
    let mut reference = reference::OracleReference::new(&mut server);
    make_with(&mut reference, seed, profile, rounds, recipe, out)
}

/// As [`make`], with the reference `reference` beside Heartwood.
pub fn make_with(
    reference: &mut dyn reference::Reference,
    seed: &str,
    profile: &str,
    rounds: u32,
    recipe: &Recipe,
    out: &Path,
) -> std::result::Result<MadeChain, LockstepError> {
    use heartwood_crypto::genesis::derive;
    use heartwood_kernel::config::crypto::{self, CryptoTexts};

    let heartwood = |e: &dyn std::fmt::Display| LockstepError::Heartwood(e.to_string());
    if rounds == 0 {
        return Err(LockstepError::Recipe(
            "a chain has at least one round".into(),
        ));
    }
    let path = profile_path(profile);
    let built = crate::genesis::Built::new(&path, seed, &crate::genesis::normalise_epoch(EPOCH))
        .map_err(|e| heartwood(&e))?;
    let definition = &built.definition;
    let files = crypto::parse(&CryptoTexts {
        network: Some(definition.network_json.clone()),
        milestones: Some(definition.milestones_json.clone()),
        genesis: Some(definition.genesis_block_json.clone()),
        exceptions: Some(definition.exceptions_json.clone()),
    })
    .map_err(|e| heartwood(&e))?;
    let round_length =
        u32::try_from(files.milestones.at(1).active_delegates()).map_err(|e| heartwood(&e))?;
    let tip = rounds
        .checked_mul(round_length)
        .ok_or_else(|| LockstepError::Recipe("too many rounds".into()))?;
    producer::check(recipe, rounds, tip)?;
    let labels = labels::Labels::new(&built, seed, files.network.pub_key_hash, recipe)?;
    let heart = heart::Heart::open(&files, &definition.block)?;
    let texts = [
        ("network", definition.network_json.clone()),
        ("milestones", definition.milestones_json.clone()),
        ("genesisBlock", definition.genesis_block_json.clone()),
        ("exceptions", definition.exceptions_json.clone()),
    ];
    let crypto_files: serde_json::Map<String, serde_json::Value> = texts
        .iter()
        .map(|(name, text)| {
            Ok((
                (*name).to_owned(),
                serde_json::from_str(text).map_err(|e| heartwood(&e))?,
            ))
        })
        .collect::<std::result::Result<_, LockstepError>>()?;
    let genesis_line = reference.open(
        &serde_json::Value::Object(crypto_files),
        &serde_json::json!({}),
    )?;
    let aux = derive::aux(&built.profile.derivation_tag, AUX_LABEL, seed);
    let made = producer::Producer::new(heart, reference, labels, recipe, aux, EPOCH_MS)
        .run(&genesis_line, tip);
    let closed = reference.close();
    let made = made?;
    closed?;
    output::write(&made, &texts, seed, &aux, round_length, out)?;
    Ok(MadeChain {
        dir: out.to_path_buf(),
        tip,
        transactions: made.blocks.iter().map(|b| b.transactions as u64).sum(),
        missed: made.missed(),
    })
}

/// `difftest lockstep make`: the chain of `args`.
pub fn run(args: &LockstepArgs) -> Result<MadeChain> {
    let most = if args.long { LONG_ROUNDS } else { SHORT_ROUNDS };
    if args.rounds > most {
        return Err(Error::Usage(format!(
            "lockstep: {} rounds is more than {most}{}",
            args.rounds,
            if args.long {
                ""
            } else {
                " (a longer chain needs --long, and 50 is the most)"
            }
        )));
    }
    let text = std::fs::read_to_string(&args.recipe).map_err(|e| Error::io(&args.recipe, e))?;
    let recipe: Recipe = serde_json::from_str(&text)
        .map_err(|e| Error::Usage(format!("{}: {e}", args.recipe.display())))?;
    make(
        &args.seed,
        &args.profile,
        args.rounds,
        &recipe,
        &args.out,
        &args.oracle,
    )
    .map_err(command_error)
}

/// The harness error of a stop: a recipe the chain cannot follow is a usage error; a file that
/// cannot be written stays an I/O error; a disagreement with the reference, or a failure of either
/// side, stops the command as a failure (exit 1, without the usage text) that names the height or
/// the transaction.
fn command_error(e: LockstepError) -> Error {
    match e {
        LockstepError::Recipe(m) => Error::Usage(format!("lockstep: the recipe: {m}")),
        LockstepError::Io { path, source } => Error::Io { path, source },
        LockstepError::Oracle(m) => Error::Oracle(m),
        other => Error::Oracle(format!("lockstep make stopped: {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_disagreement_stops_the_command_as_a_failure_and_a_bad_recipe_as_a_usage_error() {
        let stop = command_error(LockstepError::Block {
            height: 7,
            detail: "$.s differs".into(),
        });
        assert!(
            matches!(&stop, Error::Oracle(m) if m.contains("height 7") && m.contains("$.s differs")),
            "{stop}"
        );
        let pool = command_error(LockstepError::Pool {
            id: "ab".repeat(32),
            detail: "another verdict".into(),
        });
        assert!(
            matches!(&pool, Error::Oracle(m) if m.contains(&"ab".repeat(32))),
            "{pool}"
        );
        let recipe = command_error(LockstepError::Recipe("no wallet is labelled x".into()));
        assert!(
            matches!(&recipe, Error::Usage(m) if m.contains("labelled x")),
            "{recipe}"
        );
        let io = command_error(LockstepError::Io {
            path: PathBuf::from("out"),
            source: std::io::Error::other("full"),
        });
        assert!(
            matches!(&io, Error::Io { path, .. } if path == Path::new("out")),
            "{io}"
        );
    }
}
