//! Error types of the crate.
//!
//! [`TransactionError`] has one variant per refusal of the handlers, and
//! [`TransactionError::class`] names the reference's error class for it, which the differential
//! tests compare (never the message). [`PoolError`] is a refusal at pool entry, with the pool's
//! error code.

use heartwood_crypto::TransactionType;
use heartwood_crypto::identities::Address;
use heartwood_state::Rejection;
use heartwood_state::errors::WalletError;
use thiserror::Error;

/// A transaction was refused by its handler.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum TransactionError {
    /// The sender has no indexed key and no balance.
    #[error("Wallet is not allowed to spend before funding is confirmed")]
    ColdWallet,
    /// The fee is below the dynamic fee floor.
    #[error("Transaction fee is too low (fee {fee} < {minimum})")]
    FeeTooLow {
        /// The fee.
        fee: u64,
        /// The floor.
        minimum: i128,
    },
    /// The nonce is not the sender's nonce plus one (apply), or not the sender's nonce (revert).
    #[error("Cannot {} a transaction with nonce {nonce}: the sender has nonce {wallet_nonce}", if *.reverting { "revert" } else { "apply" })]
    UnexpectedNonce {
        /// The transaction's nonce.
        nonce: u64,
        /// The sender's nonce.
        wallet_nonce: u64,
        /// Whether the transaction was being reverted.
        reverting: bool,
    },
    /// The balance does not cover the amount and the fee.
    #[error(
        "Insufficient balance in the wallet: tried to spend {needed} but the wallet only has {balance} available"
    )]
    InsufficientBalance {
        /// What the transaction spends.
        needed: u128,
        /// The sender's balance.
        balance: u64,
    },
    /// The sender key or address does not match the wallet.
    #[error("Failed to apply transaction, because the public key does not match the wallet")]
    SenderWalletMismatch,
    /// A second signature without a registered second key, or a registered key the wallet of the
    /// state does not have.
    #[error("Failed to apply transaction, because the wallet does not have a second signature")]
    UnexpectedSecondSignature,
    /// The second signature is missing or does not verify with the registered key.
    #[error("Failed to apply transaction, because the second signature could not be verified")]
    InvalidSecondSignature,
    /// The sender already has a second key.
    #[error(
        "Failed to apply transaction, because a second signature is already enabled on this wallet"
    )]
    SecondSignatureAlreadyRegistered,
    /// The second key is the sender's own key.
    #[error(
        "Failed to apply transaction, because that public key is already associated with this wallet"
    )]
    PublicKeyAlreadyAssociatedWithWallet,
    /// The sender is already a delegate.
    #[error("Failed to apply transaction, because the wallet already registered as a delegate")]
    WalletIsAlreadyDelegate,
    /// Another delegate has the name.
    #[error(
        "Failed to apply transaction, because the delegate name '{username}' is already registered"
    )]
    WalletUsernameAlreadyRegistered {
        /// The name.
        username: String,
    },
    /// The sender is not a delegate.
    #[error("Failed to apply transaction, because the wallet is not a delegate")]
    WalletNotADelegate,
    /// A permanent resignation by a delegate that resigned permanently.
    #[error(
        "Failed to apply transaction, because the wallet already permanently resigned as a delegate"
    )]
    WalletAlreadyPermanentlyResigned,
    /// Any other resignation by a delegate that resigned permanently.
    #[error("Failed to apply transaction, because the wallet permanently resigned as a delegate")]
    IrrevocableResignation,
    /// A temporary resignation by a delegate that resigned temporarily.
    #[error(
        "Failed to apply transaction, because the wallet already temporarily resigned as a delegate"
    )]
    WalletAlreadyTemporarilyResigned,
    /// A revoke before the resignation's waiting time has passed.
    #[error(
        "Failed to apply transaction, because {blocks} more blocks must be produced before the resignation can be revoked"
    )]
    NotEnoughTimeSinceResignation {
        /// The blocks still to wait.
        blocks: u64,
    },
    /// A revoke by a delegate whose resignation transaction is not found up to the last block;
    /// the reference reads a field of the missing lookup result.
    #[error("Failed to apply transaction, because the delegate's resignation was not found")]
    ResignationNotFound,
    /// A revoke by a delegate that has not resigned.
    #[error("Failed to apply transaction, because the wallet has not resigned as a delegate")]
    WalletNotResigned,
    /// A resignation would leave fewer delegates than a round needs.
    #[error(
        "Failed to apply transaction, because not enough delegates are registered to allow resignation"
    )]
    NotEnoughDelegates,
    /// The burned amount is below the milestone's minimum.
    #[error(
        "Failed to apply transaction, because burn amount ({amount}) is below the minimum ({minimum})."
    )]
    InsufficientBurnAmount {
        /// The amount.
        amount: u64,
        /// The minimum, `burn.txAmount`.
        minimum: u64,
    },
    /// More votes than a round has delegates.
    #[error("Failed to apply transaction, because there are more than {maximum} votes")]
    VotedForTooManyDelegates {
        /// `activeDelegates`.
        maximum: u64,
    },
    /// An empty vote by a wallet that votes for no one.
    #[error("Failed to apply transaction, because the wallet has not voted")]
    NoVote,
    /// The same votes as the wallet's current votes.
    #[error("Failed to apply transaction, because these votes are identical to the existing votes")]
    AlreadyVotedForSameDelegates,
    /// A vote for a name no delegate has.
    #[error("Failed to apply transaction, because '{vote}' is not a delegate")]
    VotedForNonDelegate {
        /// The name.
        vote: String,
    },
    /// The undo record given to a revert is not of the transaction's type.
    #[error("The undo record does not belong to the transaction")]
    UndoMismatch,
    /// A wallet value left its range, or a wallet or delegate is missing where the reference
    /// throws.
    #[error(transparent)]
    State(#[from] WalletError),
}

impl TransactionError {
    /// The reference's error class for this refusal.
    pub fn class(&self) -> &'static str {
        match self {
            TransactionError::ColdWallet => "ColdWalletError",
            TransactionError::FeeTooLow { .. } => "TransactionFeeTooLowError",
            TransactionError::UnexpectedNonce { .. } => "UnexpectedNonceError",
            TransactionError::InsufficientBalance { .. } => "InsufficientBalanceError",
            TransactionError::SenderWalletMismatch => "SenderWalletMismatchError",
            TransactionError::UnexpectedSecondSignature => "UnexpectedSecondSignatureError",
            TransactionError::InvalidSecondSignature => "InvalidSecondSignatureError",
            TransactionError::SecondSignatureAlreadyRegistered => {
                "SecondSignatureAlreadyRegisteredError"
            }
            TransactionError::PublicKeyAlreadyAssociatedWithWallet => {
                "PublicKeyAlreadyAssociatedWithWalletError"
            }
            TransactionError::WalletIsAlreadyDelegate => "WalletIsAlreadyDelegateError",
            TransactionError::WalletUsernameAlreadyRegistered { .. } => {
                "WalletUsernameAlreadyRegisteredError"
            }
            TransactionError::WalletNotADelegate => "WalletNotADelegateError",
            TransactionError::WalletAlreadyPermanentlyResigned => {
                "WalletAlreadyPermanentlyResignedError"
            }
            TransactionError::IrrevocableResignation => "IrrevocableResignationError",
            TransactionError::WalletAlreadyTemporarilyResigned => {
                "WalletAlreadyTemporarilyResignedError"
            }
            TransactionError::NotEnoughTimeSinceResignation { .. } => {
                "NotEnoughTimeSinceResignationError"
            }
            TransactionError::ResignationNotFound => "TypeError",
            TransactionError::WalletNotResigned => "WalletNotResignedError",
            TransactionError::NotEnoughDelegates => "NotEnoughDelegatesError",
            TransactionError::InsufficientBurnAmount { .. } => "InsufficientBurnAmountError",
            TransactionError::VotedForTooManyDelegates { .. } => "VotedForTooManyDelegatesError",
            TransactionError::NoVote => "NoVoteError",
            TransactionError::AlreadyVotedForSameDelegates => "AlreadyVotedForSameDelegatesError",
            TransactionError::VotedForNonDelegate { .. } => "VotedForNonDelegateError",
            TransactionError::UndoMismatch => "Error",
            TransactionError::State(error) => error.class(),
        }
    }
}

impl Rejection for TransactionError {
    fn class(&self) -> &'static str {
        TransactionError::class(self)
    }
}

/// A transaction may not enter the pool.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PoolError {
    /// The sender already has a transaction of this type in the pool.
    #[error("{sender} already has a transaction of type {} in the pool", kind.key())]
    SenderPending {
        /// The sender.
        sender: Address,
        /// The type.
        kind: TransactionType,
    },
    /// A delegate registration for this name is already in the pool.
    #[error("Delegate registration for '{username}' already in the pool")]
    UsernamePending {
        /// The name.
        username: String,
    },
    /// The delegate does not operate a node on the network.
    #[error("{username} is not operating a node on the network")]
    Offline {
        /// The delegate's name.
        username: String,
    },
}

impl PoolError {
    /// The pool's error code.
    pub fn code(&self) -> &'static str {
        match self {
            PoolError::SenderPending { .. } | PoolError::UsernamePending { .. } => "ERR_PENDING",
            PoolError::Offline { .. } => "ERR_OFFLINE",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classes_are_the_reference_names() {
        let error = TransactionError::VotedForNonDelegate {
            vote: "nobody".into(),
        };
        assert_eq!(Rejection::class(&error), "VotedForNonDelegateError");
        assert!(error.to_string().contains("'nobody' is not a delegate"));
        let negative = TransactionError::from(WalletError::NegativeBalance {
            address: heartwood_crypto::identities::Address::from_bytes([90; 21]),
        });
        assert_eq!(negative.class(), "AssertionError");
        assert_eq!(
            TransactionError::FeeTooLow { fee: 1, minimum: 2 }.class(),
            "TransactionFeeTooLowError"
        );
    }

    #[test]
    fn pool_codes() {
        let offline = PoolError::Offline {
            username: "genesis_1".into(),
        };
        assert_eq!(offline.code(), "ERR_OFFLINE");
        let pending = PoolError::UsernamePending {
            username: "genesis_1".into(),
        };
        assert_eq!(pending.code(), "ERR_PENDING");
    }
}
