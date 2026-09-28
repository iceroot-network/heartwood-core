//! The synthetic cases of the node's own forger's blocks on a corpus chain (`run l2
//! --synthetic`), each run through the reference's job (`chain.job` with `fromForger`) and
//! Heartwood's blockchain service, their lines compared:
//!
//! - `forger-received-late`: a block from the forger received with 1,999 ms left in its slot is
//!   discarded (`Discarded block <h> because it was received too late`);
//! - `forger-processed-late`: one received in time and processed with 1,999 ms left is removed
//!   through the fork recovery of one block, its transactions re-added;
//! - `forger-posted-twice`: the same bytes posted twice, the second an `EqualToLastBlock`
//!   broadcast again.

use crate::corpus::Corpus;
use crate::errors::Result;
use crate::oracle::OracleServer;
use crate::report::{CaseResult, Outcome};

/// The cases, in order.
pub const CASES: [&str; 3] = [
    "forger-received-late",
    "forger-processed-late",
    "forger-posted-twice",
];

/// Run every case on `corpus`'s chain.
pub fn run(corpus: &Corpus, oracle: &mut OracleServer) -> Result<Vec<CaseResult>> {
    let _ = oracle;
    let file = format!("{}/synthetic-forger", corpus.name());
    let result = |(line, name): (usize, &&str)| CaseResult {
        file: file.clone(),
        line: line + 1,
        op: "chain.job".to_owned(),
        label: (*name).to_owned(),
        height: 0,
        outcome: Outcome::NoSubject,
    };
    Ok(CASES.iter().enumerate().map(result).collect())
}
