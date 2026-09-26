//! The closed registry of handlers: one per transaction type, chosen by a `match` on the
//! transaction's asset, which the decoder makes match its type.

use heartwood_crypto::Transaction;
use heartwood_crypto::transactions::types::Asset;
use heartwood_state::undo::TxUndo;
use heartwood_state::wallets::WalletRepository;
use heartwood_state::{ApplyContext, TransactionHandlers, TransactionRejected};

use crate::errors::TransactionError;
use crate::handlers::core::{
    delegate_registration, delegate_resignation, second_signature_registration, transfer,
};
use crate::handlers::group2::{burn, vote};

/// The transaction handlers: the one implementation of [`TransactionHandlers`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Handlers;

impl Handlers {
    /// Run every check of `transaction` against `wallets` without changing them: the type's
    /// checks, then the base checks (the reference's `throwIfCannotBeApplied`, on the sender as
    /// apply sees it, with the transaction's key bound when the wallet has none).
    pub fn throw_if_cannot_be_applied(
        &self,
        ctx: &ApplyContext<'_>,
        wallets: &WalletRepository,
        transaction: &Transaction,
    ) -> Result<(), TransactionError> {
        match &transaction.data().asset {
            Asset::Transfer { transfers } => transfer::check(ctx, wallets, transaction, transfers),
            Asset::SecondSignature { public_key } => {
                second_signature_registration::check(ctx, wallets, transaction, public_key)
            }
            Asset::DelegateRegistration { username } => {
                delegate_registration::check(ctx, wallets, transaction, username)
            }
            Asset::DelegateResignation { resignation_type } => {
                delegate_resignation::check(ctx, wallets, transaction, *resignation_type)
            }
            Asset::Burn { .. } => burn::check(ctx, wallets, transaction),
            Asset::Vote { votes } => vote::check(ctx, wallets, transaction, votes),
        }
    }
}

impl TransactionHandlers for Handlers {
    fn bootstrap(
        &self,
        ctx: &ApplyContext<'_>,
        wallets: &mut WalletRepository,
        transaction: &Transaction,
        issuance: bool,
    ) -> Result<u64, TransactionRejected> {
        let owed = match &transaction.data().asset {
            Asset::Transfer { transfers } => {
                transfer::bootstrap(ctx, wallets, transaction, transfers, issuance)
            }
            Asset::SecondSignature { public_key } => {
                second_signature_registration::bootstrap(ctx, wallets, transaction, public_key)
            }
            Asset::DelegateRegistration { username } => {
                delegate_registration::bootstrap(ctx, wallets, transaction, username)
            }
            Asset::DelegateResignation { resignation_type } => {
                delegate_resignation::bootstrap(ctx, wallets, transaction, *resignation_type)
            }
            Asset::Burn { .. } => burn::bootstrap(ctx, wallets, transaction),
            Asset::Vote { votes } => vote::bootstrap(ctx, wallets, transaction, votes),
        };
        Ok(owed?)
    }

    fn apply(
        &self,
        ctx: &ApplyContext<'_>,
        wallets: &mut WalletRepository,
        transaction: &Transaction,
    ) -> Result<TxUndo, TransactionRejected> {
        let undo = match &transaction.data().asset {
            Asset::Transfer { transfers } => transfer::apply(ctx, wallets, transaction, transfers),
            Asset::SecondSignature { public_key } => {
                second_signature_registration::apply(ctx, wallets, transaction, public_key)
            }
            Asset::DelegateRegistration { username } => {
                delegate_registration::apply(ctx, wallets, transaction, username)
            }
            Asset::DelegateResignation { resignation_type } => {
                delegate_resignation::apply(ctx, wallets, transaction, *resignation_type)
            }
            Asset::Burn { .. } => burn::apply(ctx, wallets, transaction),
            Asset::Vote { votes } => vote::apply(ctx, wallets, transaction, votes),
        };
        Ok(undo?)
    }

    fn revert(
        &self,
        ctx: &ApplyContext<'_>,
        wallets: &mut WalletRepository,
        transaction: &Transaction,
        undo: &TxUndo,
    ) -> Result<(), TransactionRejected> {
        let reverted = match &transaction.data().asset {
            Asset::Transfer { transfers } => {
                transfer::revert(ctx, wallets, transaction, transfers, undo)
            }
            Asset::SecondSignature { .. } => {
                second_signature_registration::revert(ctx, wallets, transaction, undo)
            }
            Asset::DelegateRegistration { .. } => {
                delegate_registration::revert(ctx, wallets, transaction, undo)
            }
            Asset::DelegateResignation { .. } => {
                delegate_resignation::revert(ctx, wallets, transaction, undo)
            }
            Asset::Burn { .. } => burn::revert(ctx, wallets, transaction, undo),
            Asset::Vote { .. } => vote::revert(ctx, wallets, transaction, undo),
        };
        Ok(reverted?)
    }
}
