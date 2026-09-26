//! Errors of the transaction codec, the transaction rules and the builders.

use thiserror::Error;

use super::{AddressError, HexError, KeyError, SigError, UnexpectedEnd};

/// Transaction bytes were refused at decode.
///
/// The reference wraps most decode failures in one error class; the variants here say why.
/// [`DecodeError::Schema`] marks a refusal by the transaction rules rather than by the codec, and
/// [`DecodeError::UnsupportedVersion`] a refused version.
///
/// Four refusals are deliberate differences from the reference, which decodes these forms. Three
/// of them it refuses later, when the transaction is applied, so a block that holds one is refused
/// either way: an extended header ([`DecodeError::UnsupportedHeaderType`]), a removed type
/// ([`DecodeError::RemovedType`]) and multi-signature entries ([`DecodeError::MultiSignature`]).
/// The fourth, a memo that is not valid UTF-8 ([`DecodeError::InvalidMemo`]), the reference reads
/// as replacement characters and accepts, in a block too: a known consensus difference. The copy
/// of such a transaction with the replacement characters has the same id and signature, and both
/// implementations accept it.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DecodeError {
    /// The bytes end inside a field.
    #[error(transparent)]
    Truncated(#[from] UnexpectedEnd),
    /// Only version 3 is accepted (the wire byte 0 reads as version 3).
    #[error("unsupported transaction version {0}")]
    UnsupportedVersion(u8),
    /// Only the standard header (header type 0) is accepted.
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
    /// The memo is not valid UTF-8. The reference reads it as replacement characters instead (see
    /// the type's documentation).
    #[error("the memo is not valid UTF-8")]
    InvalidMemo,
    /// The vote asset is over 1,024 bytes.
    #[error("the vote asset is {size} bytes, over 1024")]
    VoteAssetTooLarge {
        /// Size of the vote asset in bytes.
        size: usize,
    },
    /// The bytes after the asset are neither signatures nor multi-signature entries.
    #[error("signature buffer not exhausted")]
    SignatureBytes,
    /// The bytes after the signatures are multi-signature entries (65 bytes each). No sender can
    /// hold a multi-signature wallet, so no such transaction can be applied.
    #[error("multi-signature entries are not accepted")]
    MultiSignature,
    /// The decoded fields do not serialise again, so the id cannot be computed: a transfer to an
    /// address of another network, or a value that does not fit its field.
    #[error("cannot compute the transaction id: {0}")]
    Id(#[source] EncodeError),
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
    /// A value does not fit its wire field: a nonce or fee over `2^63 − 1`, a length over its
    /// length byte, or a sender key that is not 33 bytes.
    #[error("{field} does not fit its wire field")]
    OutOfRange {
        /// Name of the field, in the reference's JSON naming.
        field: &'static str,
    },
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

impl SchemaError {
    /// A refused value of `field`.
    pub(crate) fn invalid(field: impl Into<String>, reason: &'static str) -> SchemaError {
        SchemaError::Invalid {
            field: field.into(),
            reason,
        }
    }

    /// A missing `field`.
    pub(crate) fn missing(field: impl Into<String>) -> SchemaError {
        SchemaError::Missing {
            field: field.into(),
        }
    }
}

/// A memo was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum MemoError {
    /// An empty memo is no memo: leave it out instead.
    #[error("a memo is 1 to 255 bytes; an empty memo is written as no memo")]
    Empty,
    /// The memo is over 255 bytes of UTF-8.
    #[error("the memo is {length} bytes, over 255")]
    TooLong {
        /// Length of the memo in bytes.
        length: usize,
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
    /// A memo is refused.
    #[error(transparent)]
    Memo(#[from] MemoError),
    /// A transfer has more items than the milestone allows.
    #[error("{count} transfer items, over the maximum of {maximum}")]
    TooManyTransfers {
        /// Items in the transfer.
        count: usize,
        /// The most items the milestone allows.
        maximum: usize,
    },
    /// The JSON does not have the shape of a transaction.
    #[error("invalid transaction JSON: {0}")]
    Json(#[from] JsonError),
}

/// The JSON form of a transaction or a block does not have the reference's shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum JsonError {
    /// The value is not a JSON object.
    #[error("the value is not a JSON object")]
    NotAnObject,
    /// A required key is missing.
    #[error("{field} is missing")]
    Missing {
        /// The key.
        field: &'static str,
    },
    /// A key has a value of the wrong type or range.
    #[error("{field} must be {expected}")]
    Invalid {
        /// The key.
        field: &'static str,
        /// What the value must be.
        expected: &'static str,
    },
}
