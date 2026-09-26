//! Golden vectors of the primitives: hashes, keys, addresses, Base58 and both signature schemes.
//!
//! - `keys`: class V01 (passphrase keys, the public-key acceptance set, addresses).
//! - `signatures`: class V04 (BIP340 and legacy Schnorr, transaction preimages, status messages).
//! - `external`: the BIP340 and bip-schnorr parts of class V11 (published vector files).
//!
//! The legacy Schnorr records need the feature `legacy-schnorr`; run with `--all-features`.

#[macro_use]
#[path = "../common/mod.rs"]
mod common;

mod external;
mod keys;
mod signatures;
mod support;
