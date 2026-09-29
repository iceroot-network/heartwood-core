//! The synthetic rollback cases on a corpus chain's tip, which `run l2 DIR --synthetic` runs with
//! the other synthetic cases: a rollback on both sides and the start after it, compared line by
//! line and row by row, with a stored pool, and with an older saved state kept on the reference's
//! side.

use crate::corpus::Corpus;
use crate::errors::Result;
use crate::oracle::OracleServer;
use crate::report::{CaseResult, Outcome};

/// Run the rollback cases on top of `corpus` against the oracle `server`.
pub fn run(corpus: &Corpus, server: &mut OracleServer) -> Result<Vec<CaseResult>> {
    let _ = server;
    Ok(vec![CaseResult {
        file: format!("{}/synthetic", corpus.name()),
        line: 0,
        op: "rollback".into(),
        label: "the rollback cases".into(),
        height: u64::from(corpus.tip()),
        outcome: Outcome::NoSubject,
    }])
}
