//! BIP39 English mnemonics from entropy: only what the key derivation needs.
//!
//! The word list is the standard BIP39 English list, 2,048 words, one per line. The mnemonic of
//! 16 bytes of entropy is 12 words: the entropy followed by the first 4 bits of its SHA-256, read
//! as twelve 11-bit indexes into the list, joined by single spaces.

#![expect(
    unused_variables,
    reason = "the bodies are not written yet; remove this once they are"
)]

use zeroize::Zeroizing;

/// The BIP39 English word list.
pub const WORDLIST: &str = include_str!("bip39_english.txt");

/// The 12-word mnemonic of 16 bytes of entropy.
pub fn entropy_to_mnemonic(entropy: &[u8; 16]) -> Zeroizing<String> {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::WORDLIST;
    use crate::crypto::hash::sha256;
    use crate::utils::hex;

    #[test]
    fn word_list_is_the_standard_list() {
        assert_eq!(
            hex::encode(&sha256(WORDLIST.as_bytes())),
            "2f5eed53a4727b4bf8880d8f3f199efc90e58503646d9ff8eff3a2ed3b24dbda"
        );
        assert_eq!(WORDLIST.lines().count(), 2048);
    }
}
