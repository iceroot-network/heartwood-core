//! The synthetic processor and fork cases on a corpus chain's prefix, each against the oracle's
//! `chain.process`, `chain.job` and session operations: every unchained status, a double-forged
//! block and its recovery at fixed depths, a refused block after a slot gap (the missed rows of a
//! refused block), an unchained first block, a milestone cut, conflicting and already forged
//! transactions, a removal across a round end with pool re-adds, a future slot, a removed type in
//! a block (as a job's first block and after an accepted one), a vote at a round's end with a
//! competing branch, and a milestone at exactly the last downloaded height.

use crate::corpus::Corpus;
use crate::errors::Result;
use crate::oracle::OracleServer;
use crate::report::CaseResult;

/// Run every synthetic processor case on `corpus`'s chain.
pub fn run(corpus: &Corpus, oracle: &mut OracleServer) -> Result<Vec<CaseResult>> {
    let _ = (corpus, oracle);
    todo!()
}
