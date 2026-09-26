//! Error types of the crate.
//!
//! Each area keeps its error enums in its own file below `errors/`, and this module re-exports
//! them all, so callers write `heartwood_crypto::errors::DecodeError` whatever file defines it.
//! The two small errors of the shared byte utilities are defined here.
//!
//! Error classes are compared with the reference only where they are observable (an accept or
//! reject verdict, and the stage that rejects); messages never are.

use thiserror::Error;

mod blocks;
mod config;
#[cfg(feature = "genesis")]
mod genesis;
mod primitives;
mod transactions;

pub use self::blocks::*;
pub use self::config::*;
#[cfg(feature = "genesis")]
pub use self::genesis::*;
pub use self::primitives::*;
pub use self::transactions::*;

/// A read went past the end of the input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("unexpected end of input at offset {offset}: {needed} bytes needed, {remaining} left")]
pub struct UnexpectedEnd {
    /// Offset of the read that failed.
    pub offset: usize,
    /// Bytes the read needed.
    pub needed: usize,
    /// Bytes that were left.
    pub remaining: usize,
}

/// Text that is not the hex encoding of the expected bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum HexError {
    /// The text has an odd number of characters.
    #[error("hex text has an odd length")]
    OddLength,
    /// A character is not a hex digit.
    #[error("invalid hex character at index {index}")]
    InvalidCharacter {
        /// Index of the character in the text.
        index: usize,
    },
    /// The text decodes to the wrong number of bytes.
    #[error("expected {expected} bytes, got {actual}")]
    WrongLength {
        /// Bytes expected.
        expected: usize,
        /// Bytes decoded.
        actual: usize,
    },
}
