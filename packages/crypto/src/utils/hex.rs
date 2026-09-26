//! Hex encoding, as the reference writes bytes into JSON: lowercase on output.
//!
//! Decoding is strict: an even number of hex digits, either case, nothing else. Where the
//! reference is more lenient with its input, the caller reproduces that before decoding.

use crate::errors::HexError;

/// Lowercase hex of `bytes`.
pub fn encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        out.push(digit(byte >> 4));
        out.push(digit(byte & 0x0f));
    }
    out
}

/// The bytes that `text` encodes.
pub fn decode(text: &str) -> Result<Vec<u8>, HexError> {
    let (pairs, rest) = text.as_bytes().as_chunks::<2>();
    if !rest.is_empty() {
        return Err(HexError::OddLength);
    }
    let mut out = Vec::with_capacity(pairs.len());
    for (pair, &[high, low]) in pairs.iter().enumerate() {
        let high = nibble(high).ok_or(HexError::InvalidCharacter { index: pair * 2 })?;
        let low = nibble(low).ok_or(HexError::InvalidCharacter {
            index: pair * 2 + 1,
        })?;
        out.push((high << 4) | low);
    }
    Ok(out)
}

/// The `N` bytes that `text` encodes; any other length is an error.
pub fn decode_array<const N: usize>(text: &str) -> Result<[u8; N], HexError> {
    let bytes = decode(text)?;
    let actual = bytes.len();
    bytes.try_into().map_err(|_| HexError::WrongLength {
        expected: N,
        actual,
    })
}

/// The lowercase hex digit of a value below 16.
fn digit(value: u8) -> char {
    char::from_digit(u32::from(value), 16).unwrap_or('0')
}

fn nibble(digit: u8) -> Option<u8> {
    match digit {
        b'0'..=b'9' => Some(digit - b'0'),
        b'a'..=b'f' => Some(digit - b'a' + 10),
        b'A'..=b'F' => Some(digit - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let bytes = [0x00, 0x01, 0x7f, 0x80, 0xab, 0xff];
        assert_eq!(encode(&bytes), "00017f80abff");
        assert_eq!(decode("00017f80abff").unwrap(), bytes);
        assert_eq!(decode("00017F80ABFF").unwrap(), bytes);
        assert_eq!(decode("").unwrap(), Vec::<u8>::new());
    }

    #[test]
    fn refusals() {
        assert_eq!(decode("abc"), Err(HexError::OddLength));
        assert_eq!(decode("0g"), Err(HexError::InvalidCharacter { index: 1 }));
        assert_eq!(decode("x0"), Err(HexError::InvalidCharacter { index: 0 }));
        assert_eq!(
            decode_array::<2>("00"),
            Err(HexError::WrongLength {
                expected: 2,
                actual: 1
            })
        );
        assert_eq!(decode_array::<2>("0102").unwrap(), [1, 2]);
    }
}
