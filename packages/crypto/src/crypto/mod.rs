//! Hashes, signatures, slot arithmetic and the domain tags of Heartwood's own hashes.
//!
//! [`sig`] is the only module that calls the signature backends; the backends themselves are
//! private. [`domain`] is the one registry of the tags that Heartwood's own structures hash with.

mod bip340;
pub mod domain;
pub mod hash;
#[cfg(feature = "legacy-schnorr")]
mod legacy_schnorr;
#[cfg(feature = "legacy-schnorr")]
mod legendre;
pub mod sig;
pub mod slots;
