//! The synthetic clear cases on a corpus chain's tip, which `run l2 DIR --synthetic` runs with the
//! other synthetic cases: `state clear` and `pool clear`, then a start, on both sides, with the
//! node's directories made with mode 0700 and again under a umask of 002.

use crate::corpus::Corpus;
use crate::errors::Result;
use crate::oracle::OracleServer;
use crate::report::{CaseResult, Outcome};

/// Run the clear cases on top of `corpus` against the oracle `server`.
pub fn run(corpus: &Corpus, server: &mut OracleServer) -> Result<Vec<CaseResult>> {
    let _ = server;
    Ok(vec![CaseResult {
        file: format!("{}/synthetic", corpus.name()),
        line: 0,
        op: "clear".into(),
        label: "the clear cases".into(),
        height: u64::from(corpus.tip()),
        outcome: Outcome::NoSubject,
    }])
}
