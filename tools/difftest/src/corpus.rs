//! The chains that L2 replays.
//!
//! A corpus directory holds a chain produced by a reference devnet, in Heartwood's format and with
//! no machine path in it:
//!
//! - `crypto/`: `network.json`, `milestones.json`, `genesisBlock.json`, `exceptions.json`;
//! - `blocks.jsonl`: one `{h, id, hex}` per block after the genesis block, `hex` the full block
//!   bytes (header, signature, transaction lengths and transactions);
//! - `reference/`: the devnet node's round rows, missed slots, block metadata and API checkpoints,
//!   which anchor the oracle's replay to the node;
//! - `revert-plan.json`: the reverts both sides run;
//! - `ts-digests.jsonl` and `anchors.json`: the oracle's digest lines and anchor results;
//! - `corpus.json`: the reference commit and tree, the oracle image, heights and counts;
//! - `MANIFEST.sha256`: the SHA-256 of every other file.

use std::path::{Path, PathBuf};

use heartwood_crypto::{Block, BlockId, Milestones, Network};

use crate::errors::Result;

/// A corpus, loaded.
#[derive(Debug, Clone)]
pub struct Corpus {
    /// The corpus directory.
    pub dir: PathBuf,
    /// The network description.
    pub network: Network,
    /// The milestones.
    pub milestones: Milestones,
    /// The genesis block.
    pub genesis: Block,
    /// The blocks after the genesis block, in height order.
    pub blocks: Vec<CorpusBlock>,
    /// The reverts to run.
    pub revert_plan: RevertPlan,
}

/// One block of a corpus, as the devnet node stored it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CorpusBlock {
    /// The height.
    pub height: u32,
    /// The id.
    pub id: BlockId,
    /// The full block bytes.
    pub bytes: Vec<u8>,
}

/// The reverts of a replay: after the block at `after_height` is applied, revert `blocks` blocks,
/// then apply them again.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RevertPlan {
    /// The reverts, in height order.
    pub steps: Vec<RevertStep>,
}

/// One revert of a plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RevertStep {
    /// The height after which the revert runs.
    pub after_height: u32,
    /// The number of blocks reverted, then applied again.
    pub blocks: u32,
}

impl RevertPlan {
    /// The plan for a chain of `tip` blocks with rounds of `round_length` blocks, from `seed`:
    /// one block at every round end, 2 and a round and one block at every tenth round end, and 1
    /// to 102 blocks at 20 random heights.
    pub fn generate(seed: u64, tip: u32, round_length: u32) -> RevertPlan {
        let _ = (seed, tip, round_length);
        todo!()
    }
}

impl Corpus {
    /// Load the corpus in `dir`, checking its manifest.
    pub fn load(dir: &Path) -> Result<Corpus> {
        let _ = dir;
        todo!()
    }
}

/// Join the devnet archives `archives` (one or more increments of the same chain, in order) into
/// a corpus at `out`, checking that each increment continues the one before it.
pub fn import(archives: &[PathBuf], out: &Path) -> Result<()> {
    let _ = (archives, out);
    todo!()
}

/// Check the manifest of the corpus in `dir`.
pub fn verify(dir: &Path) -> Result<()> {
    let _ = dir;
    todo!()
}
