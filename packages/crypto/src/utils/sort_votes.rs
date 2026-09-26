//! The canonical order of a vote asset.

#![expect(
    unused_variables,
    reason = "the bodies are not written yet; remove this once they are"
)]

use crate::transactions::types::Votes;

/// `votes` in canonical order: basis points descending, then names in ascending byte order.
pub fn sort_votes(votes: &Votes) -> Votes {
    todo!()
}
