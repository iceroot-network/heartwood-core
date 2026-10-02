//! The lockstep batch (`difftest lockstep batch`): many lockstep chains, each with a recipe drawn
//! from the batch's seed, made at once and written into one corpus directory beside a batch file
//! that names them.
//!
//! A drawn recipe uses every kept type, missed-slot patterns, revert plans across round ends,
//! restart points, a milestone that activates inside the chain and the online steps; a long
//! chain's recipe also stops attesting one delegate after round 1, so that its online entry has
//! expired at round 7's start and a vote for it in round 7 is refused `ERR_OFFLINE` by both pools.
//! Each chain is made by [`super::make`] on run 1's [`Recipe`], so it is an ordinary corpus that
//! `corpus judge`, `l2`, `l2-processor`, `l2-restart` and `forge-replay` read as they are.
//!
//! The batch file `<out>/batch-<seed>.json` lists the chains (`batch-<seed>-<index>`, so that
//! every chain's name is its own in the whole corpus directory), their seeds, rounds and recipes,
//! and what each made; `difftest run l2-batch` reads it.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{MadeChain, Recipe};
use crate::cli::LockstepBatchArgs;
use crate::errors::{Error, Result};

/// The batch file's format.
pub const BATCH_FORMAT: &str = "heartwood-lockstep-batch/1";

/// One chain of a batch, as drawn from the batch's seed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DrawnChain {
    /// The chain's place in the batch, from 1.
    pub index: u32,
    /// The chain's corpus name, `batch-<seed>-<index>`.
    pub name: String,
    /// The chain's own seed, drawn from the batch's.
    pub seed: String,
    /// The chain's rounds.
    pub rounds: u32,
    /// Whether the chain is a long one (more than 5 rounds, the online entry's expiry shown).
    pub long: bool,
    /// The drawn recipe.
    pub recipe: Recipe,
}

/// What one chain of a batch made.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BatchChain {
    /// The chain as drawn.
    pub drawn: DrawnChain,
    /// The last height, when the chain was made.
    pub tip: Option<u32>,
    /// The transactions the chain holds.
    pub transactions: u64,
    /// The slots missed.
    pub missed: u64,
    /// Why the producer stopped, when it did.
    pub stopped: Option<String>,
}

/// A batch: its file's contents.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Batch {
    /// [`BATCH_FORMAT`].
    pub format: String,
    /// The batch's seed.
    pub seed: String,
    /// The genesis profile of every chain.
    pub profile: String,
    /// Every chain, in index order.
    pub chains: Vec<BatchChain>,
}

impl BatchChain {
    /// The chain as the producer made it, `out` being the corpus directory.
    pub fn made(&self, out: &Path) -> Option<MadeChain> {
        Some(MadeChain {
            dir: out.join(&self.drawn.name),
            tip: self.tip?,
            transactions: self.transactions,
            missed: self.missed,
        })
    }
}

/// The batch file of the batch `seed` in the corpus directory `out`.
pub fn batch_file(out: &Path, seed: &str) -> PathBuf {
    out.join(format!("batch-{seed}.json"))
}

/// Draw the recipe of the chain `index` (from 1) of the batch `seed`, of `rounds` rounds (a long
/// chain when `long`). The same arguments always draw the same chain.
pub fn draw(seed: &str, index: u32, rounds: u32, long: bool) -> Result<DrawnChain> {
    let _ = (seed, index, rounds, long);
    Err(Error::NotYet(
        "lockstep batch is not built yet: drawing a recipe".into(),
    ))
}

/// `difftest lockstep batch`: draw and make the chains of `args` at once, write the batch file,
/// and return it. A chain the producer stops is kept in the batch with its reason, and fails it.
pub fn run(args: &LockstepBatchArgs) -> Result<Batch> {
    let _ = args;
    Err(Error::NotYet("lockstep batch is not built yet".into()))
}
