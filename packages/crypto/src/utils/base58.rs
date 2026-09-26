//! Base58 and Base58Check with the Bitcoin alphabet, as the reference encodes addresses.
//!
//! The Base58Check checksum is the first four bytes of the double SHA-256 of the payload. Decoding
//! checks the checksum only; the payload may have any length, as in the reference.

#![expect(
    unused_variables,
    reason = "the bodies are not written yet; remove this once they are"
)]

use crate::errors::Base58Error;

/// Base58 of `bytes` (leading zero bytes become leading `1` characters).
pub fn encode(bytes: &[u8]) -> String {
    todo!()
}

/// The bytes that the Base58 `text` encodes.
pub fn decode(text: &str) -> Result<Vec<u8>, Base58Error> {
    todo!()
}

/// Base58 of `payload` followed by its four checksum bytes.
pub fn encode_check(payload: &[u8]) -> String {
    todo!()
}

/// The payload of Base58Check `text`, after checking its checksum.
pub fn decode_check(text: &str) -> Result<Vec<u8>, Base58Error> {
    todo!()
}
