//! Error types of the crate.

use thiserror::Error;

/// Why a block does not chain onto the one before it. The checks run in this order, and the
/// first that fails is reported, as in the reference's message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum NotChained {
    /// The block's `previousBlock` is not the id of the block before it.
    #[error("previous block id mismatch")]
    PreviousBlock,
    /// The block's height is not the previous height plus one.
    #[error("height is not plus one")]
    HeightNotPlusOne,
    /// The block's slot is not after the previous block's slot.
    #[error("previous slot is not smaller: {previous} VS {next}")]
    SlotNotAfterPrevious {
        /// The slot of the previous block.
        previous: i64,
        /// The slot of the block.
        next: i64,
    },
}

/// An epoch timestamp that cannot be written as a date.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum TimestampError {
    /// The milestone epoch is not a date and time the crate reads.
    #[error("the epoch {0} is not a date and time with a time zone")]
    Epoch(String),
    /// The date is outside the range of a JavaScript `Date`.
    #[error("{epoch_stamp} seconds after the epoch is outside the range of a date")]
    OutOfRange {
        /// Seconds after the network epoch.
        epoch_stamp: i64,
    },
}

/// A component failed to boot.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{package} failed to boot: {reason}")]
pub struct BootError {
    /// The component's package name, as `app.json` lists it.
    pub package: &'static str,
    /// What failed.
    pub reason: String,
}
