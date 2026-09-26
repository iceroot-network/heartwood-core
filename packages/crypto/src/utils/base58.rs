//! Base58 and Base58Check with the Bitcoin alphabet, as the reference encodes addresses.
//!
//! The Base58Check checksum is the first four bytes of the double SHA-256 of the payload. Decoding
//! checks the checksum only; the payload may have any length, as in the reference.

use crate::crypto::hash::hash256;
use crate::errors::Base58Error;

/// Bytes of checksum after a Base58Check payload.
const CHECKSUM_LEN: usize = 4;

/// Base58 of `bytes` (leading zero bytes become leading `1` characters).
pub fn encode(bytes: &[u8]) -> String {
    bs58::encode(bytes).into_string()
}

/// The bytes that the Base58 `text` encodes.
pub fn decode(text: &str) -> Result<Vec<u8>, Base58Error> {
    bs58::decode(text).into_vec().map_err(|error| match error {
        bs58::decode::Error::InvalidCharacter { index, .. }
        | bs58::decode::Error::NonAsciiCharacter { index } => {
            Base58Error::InvalidCharacter { index }
        }
        // Decoding into a growable buffer fails on nothing but a character outside the alphabet;
        // the decoder's error type is non-exhaustive, so any other error lands here.
        _ => Base58Error::InvalidCharacter { index: 0 },
    })
}

/// Base58 of `payload` followed by its four checksum bytes.
pub fn encode_check(payload: &[u8]) -> String {
    let checksum = hash256(payload);
    let mut bytes = Vec::with_capacity(payload.len() + CHECKSUM_LEN);
    bytes.extend_from_slice(payload);
    bytes.extend(checksum.iter().take(CHECKSUM_LEN));
    encode(&bytes)
}

/// The payload of Base58Check `text`, after checking its checksum.
///
/// Text that decodes to fewer than four bytes has no checksum and is refused, as the reference
/// refuses it; exactly four bytes are an empty payload with its checksum.
pub fn decode_check(text: &str) -> Result<Vec<u8>, Base58Error> {
    let mut bytes = decode(text)?;
    let payload_len = bytes
        .len()
        .checked_sub(CHECKSUM_LEN)
        .ok_or(Base58Error::TooShort)?;
    let checksum = bytes.split_off(payload_len);
    if hash256(&bytes).get(..CHECKSUM_LEN) != Some(checksum.as_slice()) {
        return Err(Base58Error::InvalidChecksum);
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain() {
        assert_eq!(encode(&[]), "");
        assert_eq!(encode(&[0, 0, 1]), "112");
        assert_eq!(decode("112").unwrap(), [0, 0, 1]);
        assert_eq!(decode("").unwrap(), Vec::<u8>::new());
        assert_eq!(decode("11").unwrap(), [0, 0]);
        assert_eq!(encode(b"hello world"), "StV1DL6CwTryKyV");
        for (text, index) in [("0", 0), ("1O", 1), ("12I", 2), ("123l", 3), ("1é", 1)] {
            assert_eq!(
                decode(text),
                Err(Base58Error::InvalidCharacter { index }),
                "{text}"
            );
        }
    }

    #[test]
    fn check() {
        let payload = [0x5a, 1, 2, 3];
        let text = encode_check(&payload);
        assert_eq!(decode_check(&text).unwrap(), payload);
        assert_eq!(decode_check(&encode_check(&[])).unwrap(), Vec::<u8>::new());
        assert_eq!(decode_check(""), Err(Base58Error::TooShort));
        assert_eq!(
            decode_check(&encode(&[1, 2, 3])),
            Err(Base58Error::TooShort)
        );
        let mut bytes = decode(&text).unwrap();
        if let Some(last) = bytes.last_mut() {
            *last ^= 1;
        }
        assert_eq!(
            decode_check(&encode(&bytes)),
            Err(Base58Error::InvalidChecksum)
        );
    }
}
