//! Errors of the chain-definition builder.

use thiserror::Error;

use super::{BlockError, BuildError, ConfigError, KeyError};

/// The chain definition could not be built from its inputs.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum GenesisError {
    /// The inputs are inconsistent (for example, an empty role name or no delegates).
    #[error("invalid genesis input: {0}")]
    Input(String),
    /// A derived key is refused.
    #[error(transparent)]
    Key(#[from] KeyError),
    /// The generated milestones do not pass the start-up checks.
    #[error(transparent)]
    Config(#[from] ConfigError),
    /// A genesis transaction could not be built.
    #[error(transparent)]
    Transaction(#[from] BuildError),
    /// The genesis block could not be made.
    #[error(transparent)]
    Block(#[from] BlockError),
    /// The issuance counter does not equal the sum of the allocations.
    #[error("the genesis issues {issued}, but the allocations add up to {allocated}")]
    Issuance {
        /// The issuance counter.
        issued: u64,
        /// The sum of the allocations.
        allocated: u64,
    },
}
