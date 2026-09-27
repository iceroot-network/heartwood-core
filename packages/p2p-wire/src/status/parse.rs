//! The canonical status text read back to its tree, for the forger's checks before it signs.
//!
//! The text is the one [`super::signing_preimage`] writes: the kernel's key-sorted `stringify` of
//! `{state, config}` with no white space. The reader accepts exactly that shape (the four state
//! keys, the 13 header keys, the configuration's keys and its plugins, nothing more) and then
//! writes the tree again: text that does not come back byte for byte is refused, so a forger
//! never signs bytes whose meaning it did not read.

use super::tree::{PeerConfig, SignedState};
use crate::errors::PreimageError;

/// Read `bytes`, an offered status text, as the tree it is the canonical form of.
///
/// Refused: text that is not UTF-8 JSON ([`PreimageError::NotJson`]); a missing or extra key, or
/// a value of the wrong type or out of range ([`PreimageError::Shape`]); a tree that
/// [`super::signing_preimage`] writes as other bytes ([`PreimageError::NotCanonical`]).
pub fn parse_preimage(bytes: &[u8]) -> Result<(SignedState, PeerConfig), PreimageError> {
    let _ = bytes;
    todo!("the status preimage reader")
}
