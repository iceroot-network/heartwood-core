//! `heartwood network generate` against the genesis vectors (`V10-genesis`): for each record's
//! seed, epoch and profile, the CLI's generator writes the chain definition into a temporary
//! directory, and its four crypto files, keys and summary must equal the record's, as the devnet
//! generator wrote them.

use crate::errors::Result;
use crate::report::Report;
use crate::vectors::VectorSet;

/// Compare `heartwood_cli::network::generate` with every genesis record of `vectors`.
pub fn check(vectors: &VectorSet) -> Result<Report> {
    let _ = vectors;
    todo!("network generate against the genesis vectors")
}
