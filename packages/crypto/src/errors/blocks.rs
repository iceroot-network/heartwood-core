//! Errors of the block codec, the block factory and the reward rules.

use thiserror::Error;

use super::{DecodeError, EncodeError, HexError, KeyError, SchemaError, SigError, UnexpectedEnd};

/// Block bytes were refused at decode.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum BlockDecodeError {
    /// The bytes end inside the header or the transaction table.
    #[error(transparent)]
    Truncated(#[from] UnexpectedEnd),
    /// A transaction of the block does not decode.
    #[error("transaction {index} of the block: {source}")]
    Transaction {
        /// Position of the transaction in the block.
        index: usize,
        /// Why it was refused.
        source: DecodeError,
    },
    /// The header breaks one of the block rules the reference writes as a JSON schema.
    #[error("invalid block at height {height}: {reason}")]
    Schema {
        /// Height in the header.
        height: u32,
        /// What the rule requires.
        reason: &'static str,
    },
}

/// A block could not be made or read from JSON.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum BlockError {
    /// Signing the header failed.
    #[error(transparent)]
    Sign(#[from] SigError),
    /// A transaction cannot be serialised.
    #[error(transparent)]
    Encode(#[from] EncodeError),
    /// The serialised block does not decode.
    #[error(transparent)]
    Decode(#[from] BlockDecodeError),
    /// A transaction in the JSON is refused.
    #[error(transparent)]
    Transaction(#[from] SchemaError),
    /// A key in the JSON is refused.
    #[error(transparent)]
    Key(#[from] KeyError),
    /// A byte field in the JSON is not hex.
    #[error(transparent)]
    Hex(#[from] HexError),
    /// The JSON does not have the shape of a block.
    #[error("invalid block JSON: {0}")]
    Json(String),
}

/// A block reward could not be computed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum RewardError {
    /// The dynamic reward is enabled and has no entry for this rank.
    #[error("no dynamic reward configured for rank {rank}")]
    NoRankReward {
        /// The delegate rank asked for.
        rank: u32,
    },
}
