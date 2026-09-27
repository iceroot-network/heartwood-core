//! The forge replay (`difftest run l2 DIR --forge`): every block of a corpus rebuilt from its own
//! inputs, and compared byte for byte with the stored block.
//!
//! - **Reference to Heartwood** (the default): each block's forger key is derived from the corpus
//!   seed, the forger aux is the manifest's when it records one, else the one derived from the seed
//!   (`heartwood_crypto::genesis::derive::aux(tag, "forger", seed)`, the devnet tooling's
//!   default); the previous block, the transactions in block order, the timestamp and the reward
//!   go through `heartwood_forger::Delegate::forge`. A corpus whose first forged block does not
//!   rebuild with that aux is reported as "aux unknown", never as passed.
//! - **Heartwood to reference** (`--oracle`): the oracle's `forger.forge` rebuilds every block of
//!   a chain Heartwood forged, from the same inputs.

use crate::corpus::Corpus;
use crate::errors::Result;
use crate::oracle::OracleServer;
use crate::report::Report;

/// Which side rebuilds the corpus's blocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Heartwood rebuilds a chain the reference forged.
    Heartwood,
    /// The reference rebuilds a chain Heartwood forged.
    Reference,
}

/// What a forge replay covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ForgeOptions {
    /// Which side rebuilds the blocks.
    pub direction: Direction,
    /// The last height replayed.
    pub to: Option<u32>,
}

/// Replay `corpus`'s blocks through the forging side of `options`; the oracle is needed for
/// [`Direction::Reference`].
pub fn run(
    corpus: &Corpus,
    options: &ForgeOptions,
    oracle: Option<&mut OracleServer>,
) -> Result<Report> {
    let _ = (corpus, options, oracle);
    todo!("the forge replay")
}
