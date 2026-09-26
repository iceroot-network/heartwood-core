//! The state digest files of L2 replays (`ts-digests.jsonl`, written by the oracle's replay) and
//! their comparison with Heartwood's digest lines.

use std::path::Path;

use heartwood_state::digest::DigestLine;
use serde_json::Value;

use crate::errors::Result;

/// The first line where two replays differ.
#[derive(Debug, Clone, PartialEq)]
pub struct LineDifference {
    /// The line's position in the files, from 1.
    pub line: usize,
    /// The oracle's line.
    pub oracle: Value,
    /// Heartwood's line.
    pub heartwood: Value,
}

/// The lines of the digest file at `path`.
pub fn read(path: &Path) -> Result<Vec<Value>> {
    let _ = path;
    todo!()
}

/// The first line of `heartwood` that differs from the same line of `oracle`, or where one side
/// has more lines.
pub fn first_difference(oracle: &[Value], heartwood: &[DigestLine]) -> Option<LineDifference> {
    let _ = (oracle, heartwood);
    todo!()
}
