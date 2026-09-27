//! The signed peer status: what a node answers to getStatus, and what a node checks in an
//! answer.
//!
//! Signer side. The relay builds the tree `{state, config}` ([`SignedState`], [`PeerConfig`]) and
//! its canonical JSON ([`signing_preimage`]): the kernel's key-sorted `stringify`, keys ordered by
//! UTF-16 code units at every level, no white space. The forger signs the preimage with each of its
//! keys ([`attest()`], legacy Schnorr with no hash), a key listed twice signing twice; the relay
//! assembles the response ([`response`]). Before the network's epoch the slot is negative: the
//! preimage carries the negative number and the wire its unsigned 32-bit value.
//!
//! Verifier side. [`decode_response`] is the getStatus response codec; [`check_reply`] is the
//! reference's reply schema for getStatus, with the changes it makes to the reply (it drops plugin
//! names outside 4 to 64 UTF-16 code units and lowercases the generator's key), and gives the
//! accepted reply back as a [`CheckedStatus`]; [`verifying_preimage`] rebuilds the signed bytes
//! from a checked reply only, turning the slot negative again when the verifier's own slot is;
//! [`verify_attestation`] checks one pair of key and signature. What a node then does with the
//! verified keys (the slot window, the delegate list) is not part of the codec.
//!
//! Forger side. A Heartwood relay offers its forger the canonical text of each new status to
//! attest; [`parse_preimage`] reads that text back to its tree, and refuses any text that is not
//! exactly the canonical form of a status tree, so the forger knows what it signs.

mod attest;
mod json;
mod parse;
mod reply;
mod tree;

#[cfg(test)]
mod tests;

pub use self::attest::{Attestation, attest, response, verify_attestation};
pub use self::parse::parse_preimage;
pub use self::reply::{
    CheckedStatus, ReceivedStatus, check_reply, decode_response, encode_response,
    verifying_preimage,
};
pub(crate) use self::reply::{header_passes, is_hex};
pub use self::tree::{
    NetworkConfig, PeerConfig, PluginConfig, SignedState, StatusHeaderData, TokenConfig,
    signing_preimage,
};
