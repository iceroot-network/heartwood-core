//! The lockstep producer (`difftest lockstep make`): a chain forged with Heartwood's genesis, state,
//! pool and forger from a seed, a genesis profile and a [`Recipe`], with every transaction built by
//! the reference and submitted to both pools and every block applied by the reference beside it,
//! written as an ordinary corpus (blocks, manifest with the forger's aux, revert and restart plans)
//! for `corpus judge`, `l2`, `l2-processor`, `l2-restart` and `forge-replay`.
//!
//! A chain is at most 5 rounds unless it is asked for as a long chain, and at most 50 rounds in
//! any case.

pub mod recipe;

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
    let _ = (seed, profile, rounds, recipe, out, oracle);
    todo!("the lockstep producer")
}

/// `difftest lockstep make`: the chain of `args`.
pub fn run(args: &LockstepArgs) -> Result<MadeChain> {
    let _ = args;
    Err(Error::NotYet(
        "lockstep make has no Rust implementation yet".into(),
    ))
}
