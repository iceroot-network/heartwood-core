//! Errors of the primitives: keys, signatures, Base58 and addresses.

use thiserror::Error;

use super::HexError;

/// A secret or public key was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum KeyError {
    /// The 32 bytes are not a valid secret key (zero, or not below the curve order).
    #[error("invalid secret key")]
    InvalidSecretKey,
    /// The bytes are not a public key the reference accepts.
    #[error("invalid public key")]
    InvalidPublicKey,
    /// The key text is not hex of the right length.
    #[error("invalid key encoding: {0}")]
    Hex(#[from] HexError),
}

/// A signature could not be made or parsed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum SigError {
    /// A signature is always 64 bytes.
    #[error("a signature is 64 bytes, got {actual}")]
    InvalidLength {
        /// Bytes given.
        actual: usize,
    },
    /// The operating system could not supply random bytes for [`crate::Aux::Random`].
    #[error("no random bytes available for the signing aux")]
    Randomness,
    /// No signature exists for this key and message: the legacy scheme derived a nonce of zero,
    /// a chance of about 2^-256. The reference fails in the same case.
    #[error("signing failed: no signature exists for this key and message")]
    SigningFailed,
    /// The signature text is not hex.
    #[error("invalid signature encoding: {0}")]
    Hex(#[from] HexError),
}

/// Base58 or Base58Check text was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum Base58Error {
    /// A character is outside the Base58 alphabet.
    #[error("invalid Base58 character at index {index}")]
    InvalidCharacter {
        /// Index of the character in the text.
        index: usize,
    },
    /// The four checksum bytes do not match the payload.
    #[error("invalid checksum")]
    InvalidChecksum,
    /// The decoded bytes are too short to hold a checksum.
    #[error("Base58Check text too short")]
    TooShort,
}

/// An address was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum AddressError {
    /// The text is not valid Base58Check.
    #[error("invalid address: {0}")]
    Base58(#[from] Base58Error),
    /// The payload is not 21 bytes (network byte and 20-byte hash).
    #[error("an address payload is 21 bytes, got {actual}")]
    InvalidLength {
        /// Payload bytes decoded.
        actual: usize,
    },
    /// The address belongs to another network.
    #[error("expected address network byte {expected}, but got {actual}")]
    WrongNetwork {
        /// The network byte in force.
        expected: u8,
        /// The address's network byte.
        actual: u8,
    },
}
