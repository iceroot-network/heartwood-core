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

use heartwood_crypto::{Params, Transaction, TransactionType};

/// The dynamic fee floor of `transaction` under `params`: 0 for burns and delegate resignations,
/// and 0 when the milestone's dynamic fees are absent or disabled; otherwise
/// `(addonBytes[key] + round(size / 2)) × max(minFee, 1)`, where `size` is the byte length of
/// the transaction as it was decoded and the sum is a JavaScript number addition.
pub fn minimum_fee(transaction: &Transaction, params: &Params) -> i128 {
    let _ = (transaction, params);
    todo!()
}

/// Whether transactions of `kind` may be applied under `params`. Every type this implementation
/// carries is always active.
pub fn is_activated(kind: TransactionType, params: &Params) -> bool {
    let _ = (kind, params);
    todo!()
}
