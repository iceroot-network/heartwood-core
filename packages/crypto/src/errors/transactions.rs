//! Errors of the transaction codec, the transaction rules and the builders.

use thiserror::Error;

use super::{AddressError, HexError, KeyError, SigError, UnexpectedEnd};

/// Transaction bytes were refused at decode.
///
/// The reference wraps most decode failures in one error class; the variants here say why, and
/// [`DecodeError::Schema`] marks a refusal by the transaction rules rather than by the codec.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DecodeError {
    /// The bytes end inside a field.
    #[error(transparent)]
    Truncated(#[from] UnexpectedEnd),
    /// Only version 3 is accepted (the wire byte 0 reads as version 3).
    #[error("unsupported transaction version {0}")]
    UnsupportedVersion(u8),
    /// Only the standard header is accepted.
    #[error("unsupported header type {0}")]
    UnsupportedHeaderType(u8),
    /// A type that the reference registers but that this implementation does not carry.
    #[error("transaction type {type_group}/{type_id} is removed")]
    RemovedType {
        /// Wire type group.
        type_group: u32,
        /// Wire type.
        type_id: u16,
    },
    /// A type that no implementation registers.
    #[error("unknown transaction type {type_group}/{type_id}")]
    UnknownType {
        /// Wire type group.
        type_group: u32,
        /// Wire type.
        type_id: u16,
    },
    /// The sender key is not a public key the reference accepts.
    #[error("invalid sender public key: {0}")]
    SenderKey(KeyError),
    /// The memo is not canonical UTF-8.
    #[error("the memo is not valid UTF-8")]
    InvalidMemo,
    /// The vote asset is over 1,024 bytes.
    #[error("the vote asset is {size} bytes, over 1024")]
    VoteAssetTooLarge {
        /// Size of the vote asset in bytes.
        size: usize,
    },
    /// The bytes after the asset are not a whole number of signatures.
    #[error("signature buffer not exhausted")]
    SignatureBytes,
    /// A recipient address has another network byte, so the id cannot be computed.
    #[error(transparent)]
    AddressNetwork(AddressError),
    /// The decoded transaction breaks a transaction rule.
    #[error(transparent)]
    Schema(#[from] SchemaError),
}

/// A transaction could not be serialised.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum EncodeError {
    /// The serialised transaction does not fit the serialiser's buffer for this height.
    #[error("the transaction is {size} bytes, over the limit of {limit}")]
    TooLarge {
        /// Size the transaction needs.
        size: usize,
        /// The limit at this height.
        limit: usize,
    },
    /// The vote asset is over 1,024 bytes.
    #[error("the vote asset is {size} bytes, over 1024")]
    VoteAssetTooLarge {
        /// Size of the vote asset in bytes.
        size: usize,
    },
    /// A recipient address has another network byte.
    #[error(transparent)]
    AddressNetwork(AddressError),
}

/// A transaction breaks one of the rules the reference writes as JSON schemas.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SchemaError {
    /// A field has a value the rules refuse.
    #[error("invalid {field}: {reason}")]
    Invalid {
        /// Path of the field, in the reference's JSON naming.
        field: String,
        /// What the rule requires.
        reason: &'static str,
    },
    /// A required field is missing.
    #[error("missing {field}")]
    Missing {
        /// Path of the field, in the reference's JSON naming.
        field: String,
    },
    /// The vote percentages do not add up to exactly 100.
    #[error("votes add up to {basis_points} basis points, not 10000")]
    VoteSum {
        /// The sum, in basis points.
        basis_points: u32,
    },
}

/// An amount in decimal text was refused.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum AmountError {
    /// The text is not a decimal integer.
    #[error("not a decimal integer: {0:?}")]
    NotAnInteger(String),
    /// The value does not fit 64 bits.
    #[error("amount out of range: {0}")]
    OutOfRange(String),
}

/// A transaction could not be built or read from JSON.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum BuildError {
    /// The transaction breaks a transaction rule.
    #[error(transparent)]
    Schema(#[from] SchemaError),
    /// The transaction cannot be serialised.
    #[error(transparent)]
    Encode(#[from] EncodeError),
    /// The serialised transaction does not decode.
    #[error(transparent)]
    Decode(#[from] DecodeError),
    /// Signing failed.
    #[error(transparent)]
    Sign(#[from] SigError),
    /// A key in the JSON is refused.
    #[error(transparent)]
    Key(#[from] KeyError),
    /// An address in the JSON is refused.
    #[error(transparent)]
    Address(#[from] AddressError),
    /// An amount in the JSON is refused.
    #[error(transparent)]
    Amount(#[from] AmountError),
    /// A byte field in the JSON is not hex.
    #[error(transparent)]
    Hex(#[from] HexError),
    /// The JSON does not have the shape of a transaction.
    #[error("invalid transaction JSON: {0}")]
    Json(String),
}
