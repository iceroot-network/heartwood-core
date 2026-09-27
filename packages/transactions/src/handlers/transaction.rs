//! What every transaction type shares: the base checks, the sender's side of apply and revert,
//! the activation of the types, and the dynamic fee floor.
//!
//! The base checks, in the reference's order, after the type's own checks: the cold-wallet check,
//! the fee floor at the transaction's height, the nonce (the sender's nonce plus one), the balance
//! (amount plus fee), the sender key and address, then the second signature (required and valid
//! when the wallet has a second key, refused when it has none). Apply binds the sender's key when
//! the wallet has none, sets the nonce and takes the amount and the fee; revert gives them back,
//! requires the sender's nonce to be the transaction's, lowers the nonce and forgets the key at
//! nonce 0.
//!
//! The reference binds the sender's key before it runs the checks, and keeps it bound when a check
//! refuses the transaction. Here the checks run first, on the sender as apply would see it (with
//! the transaction's key when the wallet has none), and a refused transaction changes nothing.

use std::borrow::Cow;

use heartwood_crypto::identities::{Address, PublicKey};
use heartwood_crypto::utils::fee_floor;
use heartwood_crypto::{Params, Transaction, TransactionType};
use heartwood_state::ApplyContext;
use heartwood_state::errors::WalletError;
use heartwood_state::wallets::{Wallet, WalletRepository};

use crate::errors::TransactionError;

/// The dynamic fee floor of `transaction` under `params`, computed by
/// [`heartwood_crypto::utils::fee_floor::minimum_fee`] from the transaction's type and its byte
/// length as it was decoded: 0 for burns and delegate resignations, and 0 when the milestone's
/// dynamic fees are absent or disabled; otherwise
/// `(addonBytes[key] + round(size / 2)) × max(minFee, 1)`, the sum a JavaScript number addition.
pub fn minimum_fee(transaction: &Transaction, params: &Params) -> i128 {
    fee_floor::minimum_fee(transaction.kind(), transaction.bytes().len(), params)
}

/// Whether transactions of `kind` may be applied under `params`. Every type this implementation
/// carries is always active: the burn is active while `burn.txAmount` is a number, which the
/// milestone loader requires at every height.
pub fn is_activated(kind: TransactionType, _params: &Params) -> bool {
    matches!(
        kind,
        TransactionType::Transfer
            | TransactionType::SecondSignature
            | TransactionType::DelegateRegistration
            | TransactionType::DelegateResignation
            | TransactionType::Burn
            | TransactionType::Vote
    )
}

/// The sender of a transaction as its checks see it: the wallet at the sender's address (an empty
/// one when none exists yet, as the reference creates it on lookup).
#[derive(Debug)]
pub(crate) struct Sender<'a> {
    /// The sender's address.
    pub(crate) address: Address,
    /// The sender's wallet.
    pub(crate) wallet: Cow<'a, Wallet>,
}

impl<'a> Sender<'a> {
    /// The sender of `transaction` in `wallets`.
    pub(crate) fn of(
        ctx: &ApplyContext<'_>,
        wallets: &'a WalletRepository,
        transaction: &Transaction,
    ) -> Sender<'a> {
        let address = transaction.data().sender_address(ctx.params);
        let wallet = wallets
            .find_by_address(&address)
            .map_or_else(|| Cow::Owned(Wallet::new(address)), Cow::Borrowed);
        Sender { address, wallet }
    }

    /// The wallet's public key, or the transaction's when the wallet has none: apply binds it
    /// before the checks run.
    pub(crate) fn public_key<'t>(&'t self, transaction: &'t Transaction) -> &'t PublicKey {
        self.wallet
            .public_key()
            .unwrap_or(&transaction.data().sender_public_key)
    }
}

/// The base checks, after the type's own checks.
pub(crate) fn check_base(
    ctx: &ApplyContext<'_>,
    wallets: &WalletRepository,
    transaction: &Transaction,
    sender: &Sender<'_>,
) -> Result<(), TransactionError> {
    let data = transaction.data();
    let wallet = &sender.wallet;
    let public_key = sender.public_key(transaction);

    // Apply indexes the key it binds, so only a wallet whose own key has no index entry is cold.
    let indexed = wallet
        .public_key()
        .is_none_or(|key| wallets.has_by_public_key(&key.to_compressed()));
    if !indexed && wallet.balance() == 0 {
        return Err(TransactionError::ColdWallet);
    }

    if ctx.params.dynamic_fees().is_some_and(|fees| fees.enabled()) {
        let minimum = minimum_fee(transaction, ctx.params);
        if i128::from(data.fee) < minimum {
            return Err(TransactionError::FeeTooLow {
                fee: data.fee,
                minimum,
            });
        }
    }

    check_nonce(wallet, data.nonce, false)?;

    let needed = u128::from(data.amount()).saturating_add(data.fee.into());
    if u128::from(wallet.balance()) < needed {
        return Err(TransactionError::InsufficientBalance {
            needed,
            balance: wallet.balance(),
        });
    }

    if public_key != &data.sender_public_key {
        return Err(TransactionError::SenderWalletMismatch);
    }

    match wallet.second_public_key() {
        Some(second) => {
            if !transaction.verify_second_signature(second.as_bytes(), ctx.params) {
                return Err(TransactionError::InvalidSecondSignature);
            }
        }
        None => {
            if data.second_signature.is_some() {
                return Err(TransactionError::UnexpectedSecondSignature);
            }
        }
    }
    Ok(())
}

/// The nonce check: the transaction's nonce is the wallet's plus one to apply it, and the
/// wallet's to revert it.
fn check_nonce(wallet: &Wallet, nonce: u64, reverting: bool) -> Result<(), TransactionError> {
    let expected = if reverting {
        Some(wallet.nonce())
    } else {
        wallet.nonce().checked_add(1)
    };
    if expected == Some(nonce) {
        Ok(())
    } else {
        Err(TransactionError::UnexpectedNonce {
            nonce,
            wallet_nonce: wallet.nonce(),
            reverting,
        })
    }
}

/// Bind the sender's key to its wallet when the wallet has none, as every path that applies a
/// transaction does first. Returns the sender's address.
pub(crate) fn bind_sender_key(
    ctx: &ApplyContext<'_>,
    wallets: &mut WalletRepository,
    transaction: &Transaction,
) -> Address {
    let data = transaction.data();
    let address = data.sender_address(ctx.params);
    if wallets
        .find_by_address(&address)
        .and_then(Wallet::public_key)
        .is_none()
    {
        wallets.bind_public_key(&address, data.sender_public_key.clone());
    }
    address
}

/// The sender's side of apply, after the checks: bind the key, set the nonce, take the amount
/// and the fee. Returns the sender's address.
pub(crate) fn apply_to_sender(
    ctx: &ApplyContext<'_>,
    wallets: &mut WalletRepository,
    transaction: &Transaction,
) -> Result<Address, TransactionError> {
    let data = transaction.data();
    let address = bind_sender_key(ctx, wallets, transaction);
    let spent = data
        .amount()
        .checked_add(data.fee)
        .ok_or(WalletError::NegativeBalance { address })?;
    let wallet = wallets.get_or_create(&address);
    wallet.debit(spent)?;
    wallet.set_nonce(data.nonce);
    Ok(address)
}

/// The sender's side of revert: the nonce must be the transaction's; give back the amount and
/// the fee, lower the nonce and forget the key at nonce 0. Returns the sender's address.
pub(crate) fn revert_for_sender(
    ctx: &ApplyContext<'_>,
    wallets: &mut WalletRepository,
    transaction: &Transaction,
) -> Result<Address, TransactionError> {
    let data = transaction.data();
    let sender = Sender::of(ctx, wallets, transaction);
    let address = sender.address;
    check_nonce(&sender.wallet, data.nonce, true)?;
    let previous = data
        .nonce
        .checked_sub(1)
        .ok_or(WalletError::NonceRange { address })?;
    let refund = u64::try_from(u128::from(data.amount()).saturating_add(data.fee.into()))
        .map_err(|_| WalletError::BalanceOverflow { address })?;
    let wallet = wallets.get_or_create(&address);
    wallet.credit(refund)?;
    wallet.set_nonce(previous);
    if previous == 0 {
        wallets.forget_public_key(&address);
    }
    Ok(address)
}

/// The wallets a transaction is about to change, as they were before, to put back when a step
/// after the checks fails. No such step fails on a state the reference can build (a balance or a
/// vote balance would have to leave the range of `u64`), but a refused transaction must leave the
/// wallets as they were.
#[derive(Debug)]
pub(crate) struct Snapshot {
    saved: Vec<(Address, Option<Wallet>)>,
}

impl Snapshot {
    /// The wallets at `addresses` in `wallets`, each once.
    pub(crate) fn take<'a>(
        wallets: &WalletRepository,
        addresses: impl IntoIterator<Item = &'a Address>,
    ) -> Snapshot {
        let mut saved: Vec<(Address, Option<Wallet>)> = Vec::new();
        for address in addresses {
            if !saved.iter().any(|(taken, _)| taken == address) {
                saved.push((*address, wallets.find_by_address(address).cloned()));
            }
        }
        Snapshot { saved }
    }

    /// Run `change` on `wallets`; when it fails, put the saved wallets back.
    pub(crate) fn guard<T>(
        self,
        wallets: &mut WalletRepository,
        change: impl FnOnce(&mut WalletRepository) -> Result<T, TransactionError>,
    ) -> Result<T, TransactionError> {
        let result = change(wallets);
        if result.is_err() {
            self.restore(wallets);
        }
        result
    }

    fn restore(self, wallets: &mut WalletRepository) {
        for (address, saved) in self.saved {
            let saved = saved.unwrap_or_else(|| Wallet::new(address));
            // Drop the index entries of what the change bound, then put the wallet back.
            if saved.public_key().is_none() {
                wallets.forget_public_key(&address);
            }
            if saved.delegate().is_none() {
                wallets.forget_delegate(&address);
            }
            *wallets.get_or_create(&address) = saved;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kept_type_is_active() {
        let milestones = heartwood_crypto::Milestones::load(
            r#"[{"height":1,"activeDelegates":1,"blockTime":8,"block":{"version":0},
            "epoch":"2026-01-01T00:00:00.000Z","burn":{"feeBasisPoints":0,"txAmount":0},
            "transfer":{}}]"#,
            &heartwood_crypto::managers::Network {
                name: "devnet".into(),
                message_prefix: String::new(),
                address_character: None,
                bip32_public: 0,
                bip32_private: 0,
                pub_key_hash: 90,
                nethash: "00".repeat(32),
                wif: 0,
                slip44: 1,
                token: String::new(),
                symbol: String::new(),
                explorer: String::new(),
            },
        )
        .unwrap();
        for kind in [
            TransactionType::Transfer,
            TransactionType::SecondSignature,
            TransactionType::DelegateRegistration,
            TransactionType::DelegateResignation,
            TransactionType::Burn,
            TransactionType::Vote,
        ] {
            assert!(is_activated(kind, milestones.at(1)));
        }
    }
}
