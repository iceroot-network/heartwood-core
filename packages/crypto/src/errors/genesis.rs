//! Errors of the chain-definition builder.

use thiserror::Error;

use super::{BlockError, BuildError, ConfigError, KeyError};
use crate::blocks::VerifyFailure;

/// The chain definition could not be built from its inputs.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum GenesisError {
    /// The seed is empty.
    #[error("the seed is empty")]
    EmptySeed,
    /// A role name is empty.
    #[error("a role name is empty")]
    EmptyRole,
    /// There are no delegates.
    #[error("there are no delegates")]
    NoDelegates,
    /// An allocation of 0: a transfer item is at least 1 base unit, in the genesis block too.
    #[error("the allocation to role {role:?} is 0")]
    ZeroAllocation {
        /// The role of the allocation.
        role: String,
    },
    /// A role signs more than one genesis transaction with the same nonce: every signing key
    /// must be a different key.
    #[error("role {role:?} signs more than once")]
    DuplicateSigner {
        /// The role.
        role: String,
    },
    /// A delegate name is registered twice.
    #[error("delegate name {name:?} is registered twice")]
    DuplicateUsername {
        /// The name.
        name: String,
    },
    /// Donations start before height 2: the block at height 1 has no generating delegate.
    #[error("donations start at height {height}; height 2 is the earliest")]
    DonationsTooEarly {
        /// The height given.
        height: u32,
    },
    /// A sum of amounts does not fit 64 bits.
    #[error("an amount adds up to more than 2^64 - 1")]
    AmountOverflow,
    /// The milestone's `transfer.maximum` is 0, so the allocations cannot be issued.
    #[error("transfer.maximum is 0")]
    ZeroTransferMaximum,
    /// The allocations do not split into transfers of `transfer.minimum` to `transfer.maximum`
    /// items.
    #[error("the allocations do not split into transfers of {minimum} to {maximum} items")]
    TransferSplit {
        /// The milestone's `transfer.minimum`.
        minimum: usize,
        /// The milestone's `transfer.maximum`.
        maximum: usize,
    },
    /// The genesis block would hold more transactions than a header can count.
    #[error("too many genesis transactions")]
    TooManyTransactions,
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
    #[error("the genesis block does not verify: {}", Failures(.0))]
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

/// The failures of a verification, as the reference's texts separated by `; `.
struct Failures<'a>(&'a [VerifyFailure]);

impl std::fmt::Display for Failures<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (index, failure) in self.0.iter().enumerate() {
            if index > 0 {
                f.write_str("; ")?;
            }
            write!(f, "{failure}")?;
        }
        Ok(())
    }
}
