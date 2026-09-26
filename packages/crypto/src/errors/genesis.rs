//! Errors of the chain-definition builder.

use thiserror::Error;

use super::{BlockError, BuildError, ConfigError, KeyError};
use crate::blocks::VerifyFailure;

/// The chain definition could not be built from its inputs.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum GenesisError {
    /// The inputs are inconsistent (for example, an empty role name or no delegates).
    #[error("invalid genesis input: {0}")]
    Input(String),
    /// The epoch is not a whole second in the form `Date.prototype.toISOString` writes.
    #[error("invalid epoch {0:?}: expected a whole second as YYYY-MM-DDTHH:MM:SS.000Z")]
    Epoch(String),
    /// BIP39 entropy must be 16 to 32 bytes long, a multiple of 4.
    #[error("BIP39 entropy must be 16 to 32 bytes long, a multiple of 4; got {length} bytes")]
    Entropy {
        /// The length given.
        length: usize,
    },
    /// The network byte gives addresses that do not all start with the same character.
    #[error("network byte {network} gives addresses starting with {low:?} or {high:?}")]
    AddressCharacter {
        /// The network byte.
        network: u8,
        /// The first character of the address with the lowest key hash.
        low: char,
        /// The first character of the address with the highest key hash.
        high: char,
    },
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
    /// The genesis block does not pass the block rules.
    #[error("the genesis block does not verify: {0:?}")]
    Verification(Vec<VerifyFailure>),
    /// The issuance counter does not equal the sum of the allocations.
    #[error("the genesis issues {issued}, but the allocations add up to {allocated}")]
    Issuance {
        /// The issuance counter.
        issued: u64,
        /// The sum of the allocations.
        allocated: u64,
    },
}
