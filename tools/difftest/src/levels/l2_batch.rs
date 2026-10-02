//! L2 over a lockstep batch (`difftest run l2-batch`): every chain of the batch files in a corpus
//! directory judged by the reference and replayed in Heartwood, one report for them all.
//!
//! For each chain: the judge (`corpus judge` with the round restart plan and the processor's
//! lines), `l2`, `l2-processor` in both job cuts, `l2-restart` (both of Heartwood's restart paths,
//! and both of the reference's, at every restart point) and the revert symmetry check. A chain
//! the batch could not make fails the report.

use std::path::{Path, PathBuf};

use crate::cli::{L2BatchArgs, OracleArgs};
use crate::errors::{Error, Result};
use crate::report::Report;

/// The batch files `args` names in its corpus directory, in name order.
pub fn batch_files(args: &L2BatchArgs) -> Result<Vec<PathBuf>> {
    let _ = args;
    Err(Error::NotYet("run l2-batch is not built yet".into()))
}

/// Judge and replay every chain of the batch file `batch`, whose chains are in `dir`.
pub fn run_batch(batch: &Path, dir: &Path, oracle: &OracleArgs) -> Result<Report> {
    let _ = (batch, dir, oracle);
    Err(Error::NotYet("run l2-batch is not built yet".into()))
}

/// `difftest run l2-batch`: every chain of the batches `args` names, in one report.
pub fn run(args: &L2BatchArgs) -> Result<Report> {
    let _ = args;
    Err(Error::NotYet("run l2-batch is not built yet".into()))
}
