//! The rollback replay (`difftest run l2 DIR --rollback`): per corpus, the store at the tip built
//! once by the processor replay; for every rollback target of the class's list that the corpus
//! reaches, a copy of that store rolled back through `heartwood_blockchain::rollback`, its lines,
//! target and deleted rows compared with the oracle's `chain.rollback` on the same corpus; the
//! next start, which must take the replay path for want of a state and write the rows of the round
//! after the target as the oracle's restart saves them; then the blocks above the target imported
//! again, every line equal to the continuous line and the store's anchors holding. A refusal must
//! leave the store as it was, byte for byte.

use crate::corpus::Corpus;
use crate::errors::{Error, Result};
use crate::oracle::Oracle;
use crate::report::Report;

/// What a rollback replay covers.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RollbackOptions {
    /// The corpus's chain cut at this height, as the judge cut it.
    pub to: Option<u32>,
}

/// The rollback replay of `corpus` against `oracle`.
pub fn run(corpus: &Corpus, options: &RollbackOptions, oracle: &Oracle) -> Result<Report> {
    let _ = (corpus, options, oracle);
    Err(Error::NotYet(
        "the rollback replay has no Rust implementation yet".into(),
    ))
}
