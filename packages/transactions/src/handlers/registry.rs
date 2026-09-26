//! The closed registry of handlers: one per transaction type, chosen by a `match` on the type.

use heartwood_crypto::{Transaction, TransactionType};
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
    /// checks, then the base checks (the reference's `throwIfCannotBeApplied`).
    pub fn throw_if_cannot_be_applied(
        &self,
        ctx: &ApplyContext<'_>,
        wallets: &WalletRepository,
        transaction: &Transaction,
    ) -> Result<(), TransactionError> {
        match transaction.kind() {
            TransactionType::Transfer => transfer::check(ctx, wallets, transaction),
            TransactionType::SecondSignature => {
                second_signature_registration::check(ctx, wallets, transaction)
            }
            TransactionType::DelegateRegistration => {
                delegate_registration::check(ctx, wallets, transaction)
            }
            TransactionType::DelegateResignation => {
                delegate_resignation::check(ctx, wallets, transaction)
            }
            TransactionType::Burn => burn::check(ctx, wallets, transaction),
            TransactionType::Vote => vote::check(ctx, wallets, transaction),
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
        let owed = match transaction.kind() {
            TransactionType::Transfer => transfer::bootstrap(ctx, wallets, transaction, issuance),
            TransactionType::SecondSignature => {
                second_signature_registration::bootstrap(ctx, wallets, transaction)
            }
            TransactionType::DelegateRegistration => {
                delegate_registration::bootstrap(ctx, wallets, transaction)
            }
            TransactionType::DelegateResignation => {
                delegate_resignation::bootstrap(ctx, wallets, transaction)
            }
            TransactionType::Burn => burn::bootstrap(ctx, wallets, transaction),
            TransactionType::Vote => vote::bootstrap(ctx, wallets, transaction),
        };
        Ok(owed?)
    }

    fn apply(
        &self,
        ctx: &ApplyContext<'_>,
        wallets: &mut WalletRepository,
        transaction: &Transaction,
    ) -> Result<TxUndo, TransactionRejected> {
        let undo = match transaction.kind() {
            TransactionType::Transfer => transfer::apply(ctx, wallets, transaction),
            TransactionType::SecondSignature => {
                second_signature_registration::apply(ctx, wallets, transaction)
            }
            TransactionType::DelegateRegistration => {
                delegate_registration::apply(ctx, wallets, transaction)
            }
            TransactionType::DelegateResignation => {
                delegate_resignation::apply(ctx, wallets, transaction)
            }
            TransactionType::Burn => burn::apply(ctx, wallets, transaction),
            TransactionType::Vote => vote::apply(ctx, wallets, transaction),
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
        let reverted = match transaction.kind() {
            TransactionType::Transfer => transfer::revert(ctx, wallets, transaction, undo),
            TransactionType::SecondSignature => {
                second_signature_registration::revert(ctx, wallets, transaction, undo)
            }
            TransactionType::DelegateRegistration => {
                delegate_registration::revert(ctx, wallets, transaction, undo)
            }
            TransactionType::DelegateResignation => {
                delegate_resignation::revert(ctx, wallets, transaction, undo)
            }
            TransactionType::Burn => burn::revert(ctx, wallets, transaction, undo),
            TransactionType::Vote => vote::revert(ctx, wallets, transaction, undo),
        };
        Ok(reverted?)
    }
}
