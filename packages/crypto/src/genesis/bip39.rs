//! BIP39 English mnemonics from entropy: only what the key derivation needs.
//!
//! The word list is the standard BIP39 English list, 2,048 words, one per line. The mnemonic of
//! `n` bytes of entropy (16 to 32, a multiple of 4) is the entropy followed by the first `n / 4`
//! bits of its SHA-256, read as 11-bit indexes into the list and joined by single spaces: 12 words
//! for 16 bytes, 24 words for 32 bytes.

use zeroize::Zeroizing;

use crate::crypto::hash::sha256;
use crate::errors::GenesisError;

/// The BIP39 English word list, one word per line.
pub const WORDLIST: &str = include_str!("bip39_english.txt");

/// The number of words in a BIP39 list.
const WORD_COUNT: usize = 2048;

/// The words of [`WORDLIST`], split at compile time. A list that does not have exactly 2,048
/// non-empty lines fails the build.
static WORDS: [&str; WORD_COUNT] = split_lines(WORDLIST);

#[allow(
    clippy::indexing_slicing,
    reason = "evaluated at compile time, where an index out of range is a build error"
)]
const fn split_lines(text: &'static str) -> [&'static str; WORD_COUNT] {
    let mut words = [""; WORD_COUNT];
    let mut rest = text;
    let mut count = 0;
    while count < WORD_COUNT {
        let bytes = rest.as_bytes();
        let mut len = 0;
        while len < bytes.len() && bytes[len] != b'\n' {
            len += 1;
        }
        let (word, tail) = rest.split_at(len);
        assert!(!word.is_empty(), "the word list has an empty line");
        words[count] = word;
        rest = tail.split_at(if tail.is_empty() { 0 } else { 1 }).1;
        count += 1;
    }
    assert!(rest.is_empty(), "the word list has more than 2,048 lines");
    words
}

/// The word with the 11-bit index `index`.
#[allow(
    clippy::indexing_slicing,
    reason = "the index is masked to 11 bits and the list has 2,048 words"
)]
fn word(index: u32) -> &'static str {
    WORDS[(index & 0x7ff) as usize]
}

/// The mnemonic of `entropy`, which must be 16 to 32 bytes long, a multiple of 4.
pub fn entropy_to_mnemonic(entropy: &[u8]) -> Result<Zeroizing<String>, GenesisError> {
    let length = entropy.len();
    if !(16..=32).contains(&length) || !length.is_multiple_of(4) {
        return Err(GenesisError::Entropy { length });
    }
    Ok(encode(entropy))
}

/// The mnemonic of entropy whose length [`entropy_to_mnemonic`] has checked, or which has one of
/// the valid lengths by its type.
pub(crate) fn encode(entropy: &[u8]) -> Zeroizing<String> {
    let [checksum, ..] = sha256(entropy);
    // One checksum bit per 32 bits of entropy: at most 8, so the first byte of the hash holds them.
    let checksum_bits = (entropy.len() / 4) as u32;
    let mut mnemonic = Zeroizing::new(String::with_capacity(entropy.len() * 7));
    // The bits not yet written, right-aligned; fewer than 11 between pushes, so at most 19 in use.
    let mut pending = Zeroizing::new(0u32);
    let mut pending_bits = 0u32;
    let mut push = |value: u32, width: u32| {
        *pending = (*pending << width) | value;
        pending_bits += width;
        while pending_bits >= 11 {
            pending_bits -= 11;
            if !mnemonic.is_empty() {
                mnemonic.push(' ');
            }
            mnemonic.push_str(word(*pending >> pending_bits));
            *pending &= (1 << pending_bits) - 1;
        }
    };
    for &byte in entropy {
        push(u32::from(byte), 8);
    }
    push(u32::from(checksum) >> (8 - checksum_bits), checksum_bits);
    mnemonic
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::hex;

    #[test]
    fn word_list_is_the_standard_list() {
        assert_eq!(
            hex::encode(&sha256(WORDLIST.as_bytes())),
            "2f5eed53a4727b4bf8880d8f3f199efc90e58503646d9ff8eff3a2ed3b24dbda"
        );
        assert_eq!(WORDLIST.lines().count(), 2048);
        assert_eq!(WORDS.first(), Some(&"abandon"));
        assert_eq!(WORDS.last(), Some(&"zoo"));
        assert!(WORDS.iter().zip(WORDLIST.lines()).all(|(a, b)| *a == b));
    }

    #[test]
    fn mnemonics_of_the_published_vectors() {
        let cases = [
            (
                "00000000000000000000000000000000",
                "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about",
            ),
            (
                "ffffffffffffffffffffffffffffffff",
                "zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo wrong",
            ),
            (
                "9e885d952ad362caeb4efe34a8e91bd2",
                "ozone drill grab fiber curtain grace pudding thank cruise elder eight picnic",
            ),
            (
                "ffffffffffffffffffffffffffffffffffffffffffffffff",
                "zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo when",
            ),
        ];
        for (entropy, mnemonic) in cases {
            let entropy = hex::decode(entropy).unwrap();
            assert_eq!(entropy_to_mnemonic(&entropy).unwrap().as_str(), mnemonic);
        }
    }

    #[test]
    fn refuses_entropy_of_other_lengths() {
        for length in [0, 4, 12, 15, 17, 18, 30, 36, 64] {
            assert_eq!(
                entropy_to_mnemonic(&vec![0; length]),
                Err(GenesisError::Entropy { length })
            );
        }
        for length in [16, 20, 24, 28, 32] {
            let words = entropy_to_mnemonic(&vec![0x5a; length]).unwrap();
            assert_eq!(words.split(' ').count(), length * 3 / 4);
        }
    }
}
