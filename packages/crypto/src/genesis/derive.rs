//! Key and aux derivation from the seed.
//!
//! A role's passphrase is the 12-word BIP39 English mnemonic of the first 16 bytes of
//! `SHA-256(UTF-8(tag + role + "/" + seed))`, and its key is the passphrase key. The aux of a
//! signature with label `label` is `SHA-256(UTF-8(tag + "aux/" + label + "/" + seed))`.
//!
//! Anyone who knows the seed can derive every key, so a chain definition built from a published
//! seed is for tests only.

use zeroize::Zeroizing;

use crate::crypto::hash::sha256;
use crate::errors::GenesisError;
use crate::genesis::{DerivedKey, bip39};
use crate::identities::{Address, KeyPair};

/// The passphrase of `role`.
pub fn role_passphrase(tag: &str, role: &str, seed: &str) -> Zeroizing<String> {
    let text = Zeroizing::new(format!("{tag}{role}/{seed}"));
    let digest = Zeroizing::new(sha256(text.as_bytes()));
    let (entropy, _) = digest.split_at(16);
    bip39::encode(entropy)
}

/// The key material of `role`, with its address for the network byte `network`.
pub fn role_key(
    tag: &str,
    role: &str,
    seed: &str,
    network: u8,
) -> Result<DerivedKey, GenesisError> {
    let passphrase = role_passphrase(tag, role, seed);
    let keys = KeyPair::from_passphrase(&passphrase)?;
    let address = Address::from_public_key(keys.public_key(), network);
    Ok(DerivedKey {
        role: role.to_owned(),
        passphrase,
        keys,
        address,
    })
}

/// The aux bytes of the signature labelled `label`.
pub fn aux(tag: &str, label: &str, seed: &str) -> [u8; 32] {
    let text = Zeroizing::new(format!("{tag}aux/{label}/{seed}"));
    sha256(text.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::hex;

    // Known answers computed with the bip39 package that the reference implementation uses.
    #[test]
    fn passphrase_known_answers() {
        assert_eq!(
            role_passphrase("tag-v1/", "some-role", "seed").as_str(),
            "solve anger hammer fetch black edge move oak category transfer immense thumb"
        );
        assert_eq!(
            role_passphrase("tag-v1/", "r\u{f6}le/\u{fc}", "seed \u{2713}").as_str(),
            "lawn fall narrow blush estate purse profit only expose purse license pitch"
        );
    }

    #[test]
    fn passphrase_is_the_mnemonic_of_the_hashed_role() {
        let digest = sha256(b"tag-v1/some-role/seed");
        let expected = bip39::entropy_to_mnemonic(digest.get(..16).unwrap()).unwrap();
        assert_eq!(role_passphrase("tag-v1/", "some-role", "seed"), expected);
        assert_ne!(role_passphrase("tag-v1/", "other-role", "seed"), expected);
        assert_ne!(role_passphrase("tag-v1/", "some-role", "seed2"), expected);
        assert_ne!(role_passphrase("tag-v2/", "some-role", "seed"), expected);
    }

    #[test]
    fn aux_known_answer() {
        assert_eq!(
            hex::encode(&aux("tag-v1/", "block", "seed")),
            "889b6ea8756113201590021035e79b0102453274433f95fee02ba0ee1a190f68"
        );
        assert_ne!(aux("tag-v1/", "0", "seed"), aux("tag-v1/", "1", "seed"));
    }
}
