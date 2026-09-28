//! `difftest corpus from-blocks`: a Heartwood relay's block files (the applied blocks' bytes,
//! one file per start) turned into a corpus that the judge replays, as `corpus import` turns the
//! devnet tooling's archives into one.

use std::path::{Path, PathBuf};

use crate::errors::{Error, Result};

/// Write into `out` the corpus of the block files `blocks` (in start order) of the chain whose
/// crypto files are in `crypto`, with the chain's key-derivation `seed`.
pub fn from_blocks(blocks: &[PathBuf], crypto: &Path, seed: &str, out: &Path) -> Result<()> {
    let _ = (blocks, crypto, seed, out);
    Err(Error::NotYet(
        "corpus from-blocks is not written yet".into(),
    ))
}
