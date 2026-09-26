//! L2: a corpus chain replayed block by block in Heartwood, with a state digest line after every
//! step compared with the oracle's line of the same step.
//!
//! The replay decodes every block as a node receives it, checks the scheduled generator and the
//! reward before applying it, runs the supply check after every step, and follows the corpus's
//! revert plan. At the first difference it shows both sides' records of that step.

use crate::corpus::Corpus;
use crate::errors::Result;
use crate::report::Report;

/// What an L2 run covers.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct L2Options {
    /// Also run the synthetic cases on top of the chain (needs the oracle).
    pub synthetic: bool,
}

/// Replay `corpus` and compare every step with its recorded oracle digests.
pub fn run(corpus: &Corpus, options: &L2Options) -> Result<Report> {
    let _ = (corpus, options);
    todo!()
}
