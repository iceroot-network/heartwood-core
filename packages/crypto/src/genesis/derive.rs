//! Key and aux derivation from the seed.
//!
//! A role's passphrase is the 12-word BIP39 English mnemonic of the first 16 bytes of
//! `SHA-256(UTF-8(tag + role + "/" + seed))`, and its key is the passphrase key. The aux of a
//! signature with label `label` is `SHA-256(UTF-8(tag + "aux/" + label + "/" + seed))`.

#![expect(
    unused_variables,
    reason = "the bodies are not written yet; remove this once they are"
)]

use zeroize::Zeroizing;

use crate::errors::GenesisError;
use crate::genesis::DerivedKey;

/// The passphrase of `role`.
pub fn role_passphrase(tag: &str, role: &str, seed: &str) -> Zeroizing<String> {
    todo!()
}

/// The key material of `role`, with its address for the network byte `network`.
pub fn role_key(
    tag: &str,
    role: &str,
    seed: &str,
    network: u8,
) -> Result<DerivedKey, GenesisError> {
    todo!()
}

/// The aux bytes of the signature labelled `label`.
pub fn aux(tag: &str, label: &str, seed: &str) -> [u8; 32] {
    todo!()
}
