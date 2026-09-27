//! Error types of the crate.
//!
//! Where the reference throws an untyped error (a failed `assert`, a lookup of a missing index
//! entry, too few delegates for a round), the state returns one of these typed errors instead,
//! with the same verdict: the block is refused, or the node refuses to start. None of them can
//! happen on a chain of blocks the reference accepts.

use std::fmt::Display;

use heartwood_crypto::blocks::VerifyFailure;
use heartwood_crypto::identities::{Address, PublicKeyBytes};
use heartwood_crypto::transactions::TransactionId;
use thiserror::Error;

/// A wallet or delegate value would leave its range, or a wallet or delegate is missing.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum WalletError {
    /// A credit would take the balance past `2^64 − 1`.
    #[error("the balance of {address} would overflow")]
    BalanceOverflow {
        /// The wallet.
        address: Address,
    },
    /// A debit is larger than the balance; the reference asserts that no balance goes negative.
    #[error("the balance of {address} would be negative")]
    NegativeBalance {
        /// The wallet.
        address: Address,
    },
    /// A nonce would go below 0 or past `2^64 − 1`.
    #[error("the nonce of {address} would leave its range")]
    NonceRange {
        /// The wallet.
        address: Address,
    },
    /// No wallet has this address where one must exist.
    #[error("no wallet has the address {address}")]
    UnknownWallet {
        /// The address.
        address: Address,
    },
    /// No delegate is registered with this name; the reference's lookup of a missing index entry
    /// throws.
    #[error("no delegate is registered as {username}")]
    UnknownDelegate {
        /// The name.
        username: String,
    },
    /// The wallet is not a delegate where it must be one.
    #[error("{address} is not a delegate")]
    NotADelegate {
        /// The wallet.
        address: Address,
    },
    /// Another wallet already holds this delegate name.
    #[error("the delegate name {username} is already registered")]
    UsernameTaken {
        /// The name.
        username: String,
    },
    /// A delegate's vote balance or one of its counters would go below 0 or past `2^64 − 1`.
    #[error("a value of delegate {username} would leave its range")]
    DelegateRange {
        /// The delegate's name.
        username: String,
    },
}

impl WalletError {
    /// The reference's error class where it fails in the same place: its `assert` on a negative
    /// balance throws an `AssertionError`; its other failures here (a lookup of a missing index
    /// entry, a value out of range) throw a plain `Error`.
    pub fn class(&self) -> &'static str {
        match self {
            WalletError::NegativeBalance { .. } => "AssertionError",
            _ => "Error",
        }
    }
}

/// The delegate ranking or a round's schedule failed.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RoundError {
    /// Fewer delegates are ranked than a round needs; the reference refuses to go on.
    #[error("expected to find {required} delegates but only found {ranked}")]
    NotEnoughDelegates {
        /// `activeDelegates`.
        required: u64,
        /// The delegates that are registered and not resigned.
        ranked: usize,
    },
    /// Two delegates have the same vote balance and the same public key.
    #[error("the delegates {first} and {second} have the same vote balance and public key")]
    IdenticalPublicKeys {
        /// One delegate's name.
        first: String,
        /// The other's.
        second: String,
    },
    /// A delegate of the ranking has no public key.
    #[error("the delegate {username} has no public key")]
    NoPublicKey {
        /// The delegate's name.
        username: String,
    },
    /// The block to revert is not the last block of the round state.
    #[error("the block at height {height} is not the last block of the round")]
    NotLastBlock {
        /// The height of the block.
        height: u32,
    },
    /// A wallet value left its range.
    #[error(transparent)]
    Wallet(#[from] WalletError),
}

/// The reward of a block in its round could not be computed.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RewardError {
    /// The delegate has no rank, so the rank table cannot be read.
    #[error("the delegate {username} has no rank")]
    NoRank {
        /// The delegate's name.
        username: String,
    },
    /// The rank table has no reward for the delegate's rank.
    #[error(transparent)]
    Reward(#[from] heartwood_crypto::errors::RewardError),
    /// No registered delegate has bound the generator key.
    #[error("the generator {} is not a registered delegate", generator.to_hex())]
    NotADelegate {
        /// The generator's public key.
        generator: PublicKeyBytes,
    },
}

/// Taking back part of a change after an error failed, so the state is no longer what it was
/// before the call that reports it. Such a step undoes what was just done, so no chain the
/// reference accepts leads here: a node that meets it has to stop and build its state again.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("taking back the {step} failed ({detail}); the state is no longer consistent")]
pub struct RollbackFailed {
    /// What was being taken back.
    pub step: &'static str,
    /// Why it failed.
    pub detail: String,
}

/// Check `result`, the result of a step that takes back part of a change after an error. It
/// cannot fail on a consistent state; when it does, debug builds stop here (so tests and fuzzing
/// catch it), and release builds log it as critical and return [`RollbackFailed`].
pub(crate) fn rolled_back<T, D: Display>(
    step: &'static str,
    result: Result<T, D>,
) -> Result<(), RollbackFailed> {
    debug_assert!(result.is_ok(), "taking back the {step} failed");
    result.map(drop).map_err(|error| {
        heartwood_kernel::critical!(
            "Taking back the {step} failed ({error}); the state is no longer consistent"
        );
        RollbackFailed {
            step,
            detail: error.to_string(),
        }
    })
}

/// A transaction could not be applied or reverted with the vote balances it changes.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum TransactionFailure<E> {
    /// The transaction was refused, or a wallet value left its range; the wallets are as they
    /// were.
    #[error(transparent)]
    Refused(E),
    /// Taking the transaction back after that failed.
    #[error(transparent)]
    RollbackFailed(#[from] RollbackFailed),
}

/// A block could not be applied. The state is unchanged, except after
/// [`ApplyError::RollbackFailed`]. `E` is the handlers' refusal
/// ([`crate::TransactionHandlers::Error`]).
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ApplyError<E> {
    /// The block's height is not the last block's height plus one.
    #[error("expected a block at height {expected}, got height {height}")]
    Height {
        /// The last block's height plus one.
        expected: u32,
        /// The block's height.
        height: u32,
    },
    /// The block's generator is not a registered delegate.
    #[error("the generator {} is not a registered delegate", generator.to_hex())]
    GeneratorNotDelegate {
        /// The generator's public key.
        generator: PublicKeyBytes,
    },
    /// A transaction was refused.
    #[error("transaction {index} ({id}) was refused: {rejected}")]
    Transaction {
        /// Its position in the block.
        index: usize,
        /// Its id.
        id: TransactionId,
        /// Why.
        rejected: E,
    },
    /// The block's donations or burned amounts add up to more than `2^64 − 1`, or its burned fee
    /// does not fit 64 bits; no block whose header adds up does this.
    #[error("the block's payout does not fit 64 bits")]
    PayoutRange,
    /// A wallet value left its range.
    #[error(transparent)]
    Wallet(#[from] WalletError),
    /// The round that the block ends could not be started.
    #[error(transparent)]
    Round(#[from] RoundError),
    /// Taking back the steps already taken failed.
    #[error(transparent)]
    RollbackFailed(#[from] RollbackFailed),
}

/// The last block could not be reverted. The state is unchanged, except after
/// [`RevertError::RollbackFailed`]. `E` is the handlers' refusal
/// ([`crate::TransactionHandlers::Error`]).
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RevertError<E> {
    /// The last block is the genesis block.
    #[error("the genesis block cannot be reverted")]
    Genesis,
    /// A transaction's revert was refused.
    #[error("transaction {index} ({id}) could not be reverted: {rejected}")]
    Transaction {
        /// Its position in the block.
        index: usize,
        /// Its id.
        id: TransactionId,
        /// Why.
        rejected: E,
    },
    /// The block's payout does not fit 64 bits (see [`ApplyError::PayoutRange`]); its apply
    /// would have been refused.
    #[error("the block's payout does not fit 64 bits")]
    PayoutRange,
    /// A wallet value left its range.
    #[error(transparent)]
    Wallet(#[from] WalletError),
    /// The round state could not be restored.
    #[error(transparent)]
    Round(#[from] RoundError),
    /// Taking back the steps already taken failed.
    #[error(transparent)]
    RollbackFailed(#[from] RollbackFailed),
}

/// The genesis block was refused; the node does not start. `E` is the handlers' refusal
/// ([`crate::TransactionHandlers::Error`]).
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum GenesisError<E> {
    /// The block is not at height 1.
    #[error("the genesis block is at height {height}, not 1")]
    Height {
        /// The block's height.
        height: u32,
    },
    /// The block does not verify (the timestamp rule excepted).
    #[error("the genesis block does not verify: {failures:?}")]
    Invalid {
        /// Every rule that failed, in the reference's order.
        failures: Vec<VerifyFailure>,
    },
    /// The issuance records add up to more than `2^63 − 1`.
    #[error("the genesis block issues more than 2^63 - 1")]
    Issuance,
    /// The milestone at height 1 has donations. The genesis block then carries donation entries
    /// with no forger to take them from, and the reference's start-up refuses to build the state.
    #[error("the milestone at height 1 has donations, which the genesis block cannot pay")]
    Donations,
    /// A wallet's built balance is negative.
    #[error("wallet {address} has a negative balance")]
    NegativeBalance {
        /// The wallet.
        address: Address,
    },
    /// A delegate's built vote balance is negative.
    #[error("delegate {username} has a negative vote balance")]
    NegativeVoteBalance {
        /// The delegate's name.
        username: String,
    },
    /// A genesis transaction could not be bootstrapped.
    #[error("genesis transaction {index} ({id}) was refused: {rejected}")]
    Transaction {
        /// Its position in the block.
        index: usize,
        /// Its id.
        id: TransactionId,
        /// Why.
        rejected: E,
    },
    /// A wallet value left its range.
    #[error(transparent)]
    Wallet(#[from] WalletError),
    /// Round 1 could not be scheduled.
    #[error(transparent)]
    Round(#[from] RoundError),
}

/// The supply check failed: the balances do not add up to what was issued and rewarded, less
/// what was burned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("the balances add up to {balances}, expected {expected}")]
pub struct SupplyError {
    /// The sum of every balance.
    pub balances: u128,
    /// Issued plus rewards, less burned fees and burned amounts.
    pub expected: i128,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rollback_that_succeeds_is_no_error() {
        assert_eq!(rolled_back("step", Ok::<u8, String>(1)), Ok(()));
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "taking back the step failed")]
    fn a_failed_rollback_stops_a_debug_build() {
        let _ = rolled_back("step", Err::<(), _>("no such wallet"));
    }

    #[test]
    #[cfg(not(debug_assertions))]
    fn a_failed_rollback_is_reported() {
        assert_eq!(
            rolled_back("step", Err::<(), _>("no such wallet")),
            Err(RollbackFailed {
                step: "step",
                detail: "no such wallet".into()
            })
        );
    }

    #[test]
    fn wallet_errors_name_the_reference_classes() {
        let address = Address::from_bytes([90; 21]);
        assert_eq!(
            WalletError::NegativeBalance { address }.class(),
            "AssertionError"
        );
        assert_eq!(WalletError::BalanceOverflow { address }.class(), "Error");
    }
}
