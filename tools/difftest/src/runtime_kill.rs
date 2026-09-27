//! The kill test through the full runtime: a child relay imports a corpus from a mock peer on
//! this host (named with the relay's seed switch of the harness) and is killed with SIGKILL at
//! random moments; each restart must restore a committed tip whose digest equals the continuous
//! line, and the import must resume to the end with equal lines. In half the kills the mock peer
//! answers its first getBlocks with nothing, so the chunk size falls to a tenth.

use std::path::Path;
use std::time::Duration;

use crate::corpus::Corpus;
use crate::errors::Result;
use crate::report::Report;

/// `difftest store crash --runtime`: kill a child relay `kills` times after up to `max_delay`,
/// with the delays drawn from `seed`; `exe` is this program, which runs the child relay.
pub fn crash(
    corpus: &Corpus,
    exe: &Path,
    kills: u32,
    seed: u64,
    max_delay: Duration,
) -> Result<Report> {
    let _ = (corpus, exe, kills, seed, max_delay);
    todo!()
}
