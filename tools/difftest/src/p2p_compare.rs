//! `difftest p2p compare RUN_DIR --left NODE --right NODE`: the reference-client probe's records of
//! two relays of a live run, compared case by case: equal reply verdicts, errors, socket outcomes
//! and ban effects, and answers equal after the masks (the version, the attestations, the slot
//! fields across a slot, the ports), with getPeers compared as sets under the run's route table.

use std::path::Path;

use crate::errors::{Error, Result};
use crate::report::Report;

/// Compare the probe's records of the nodes `left` and `right` of the run in `run_dir`.
pub fn compare(run_dir: &Path, left: &str, right: &str) -> Result<Report> {
    let _ = (run_dir, left, right);
    Err(Error::NotYet(
        "p2p compare has no Rust implementation yet".into(),
    ))
}
