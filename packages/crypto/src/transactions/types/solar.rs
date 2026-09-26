//! Assets of type group 2: burn and vote.

#![expect(
    unused_variables,
    dead_code,
    reason = "the bodies are not written yet; remove this once they are"
)]

use crate::errors::{DecodeError, EncodeError};
use crate::transactions::types::{Asset, Votes};
use crate::utils::bytes::{Reader, Writer};

/// The largest vote asset in bytes: `1 + Σ (3 + name bytes)`.
pub const MAX_VOTE_ASSET_BYTES: usize = 1024;

/// Write a burn asset: the amount (`u64`).
pub(crate) fn write_burn(amount: u64, out: &mut Writer) {
    todo!()
}

/// Read a burn asset.
pub(crate) fn read_burn(reader: &mut Reader<'_>) -> Result<Asset, DecodeError> {
    todo!()
}

/// Write a vote asset: the vote count (`u8`), then per vote the name length (`u8`), the name and
/// the basis points (`u16`). Refused over [`MAX_VOTE_ASSET_BYTES`].
pub(crate) fn write_vote(votes: &Votes, out: &mut Writer) -> Result<(), EncodeError> {
    todo!()
}

/// Read a vote asset. Refused over [`MAX_VOTE_ASSET_BYTES`].
pub(crate) fn read_vote(reader: &mut Reader<'_>) -> Result<Asset, DecodeError> {
    todo!()
}
