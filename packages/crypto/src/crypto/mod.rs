//! Hashes, signatures and slot arithmetic.
//!
//! [`sig`] is the only module that calls the signature backends; the backends themselves are
//! private.

mod bip340;
pub mod hash;
#[cfg(feature = "legacy-schnorr")]
mod legacy_schnorr;
#[cfg(feature = "legacy-schnorr")]
mod legendre;
pub mod sig;
pub mod slots;
