//! Errors of the network description and the milestones.

use thiserror::Error;

/// The network description or the milestones were refused at load.
///
/// A node refuses to start on any of these, instead of halting the chain later. Every refusal of
/// a milestone names the height and the key, as the reference's start-up checks do.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ConfigError {
    /// The text is not JSON.
    #[error("invalid JSON: {0}")]
    Json(String),
    /// The document does not have the expected shape (for example, milestones not in an array).
    #[error("invalid configuration: {0}")]
    Shape(&'static str),
    /// A milestone's `height` is missing or not a usable height.
    #[error("milestone {index} has no valid height")]
    Height {
        /// Position of the milestone in the file.
        index: usize,
    },
    /// A required key is missing from the merged milestone.
    #[error("bad milestone at height: {height}. {key} is missing")]
    Missing {
        /// Height of the merged milestone.
        height: u64,
        /// Dotted path of the key.
        key: String,
    },
    /// A key has a value the start-up checks refuse.
    #[error("bad milestone at height: {height}. {key}: {reason}")]
    Invalid {
        /// Height of the merged milestone.
        height: u64,
        /// Dotted path of the key.
        key: String,
        /// What the check requires.
        reason: &'static str,
    },
    /// `blockTime` or `activeDelegates` differs from its value at the first milestone.
    #[error("bad milestone at height: {height}. {key} cannot change after height {first_height}")]
    Changed {
        /// Height of the merged milestone.
        height: u64,
        /// `blockTime` or `activeDelegates`.
        key: &'static str,
        /// Height of the first milestone.
        first_height: u64,
    },
    /// A key that is no longer supported (for example `burn.feePercent`).
    #[error("bad milestone at height: {height}. {key} is not supported")]
    Unsupported {
        /// Height of the merged milestone.
        height: u64,
        /// Dotted path of the key.
        key: String,
    },
    /// The donation shares add up to more than 10,000 basis points.
    #[error(
        "bad milestone at height: {height}. donations add up to {sum} basis points, over 10000"
    )]
    DonationSum {
        /// Height of the merged milestone.
        height: u64,
        /// The sum of the shares.
        sum: u64,
    },
}
