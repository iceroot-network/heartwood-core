//! Pool entry: the checks a transaction must pass before it enters the pool, beyond the ones it
//! must pass to be applied.
//!
//! They read two views that the state does not hold: the pool's pending transactions
//! ([`PoolQuery`]) and which delegates operate a node on the network ([`OnlineDelegates`]). Each
//! has exactly one implementation in a node, chosen at compile time.

use heartwood_crypto::identities::Address;
use heartwood_crypto::transactions::types::Asset;
use heartwood_crypto::{Transaction, TransactionType};
use heartwood_state::ApplyContext;
use heartwood_state::wallets::WalletRepository;

use crate::errors::PoolError;
use crate::handlers::core::{
    delegate_registration, delegate_resignation, second_signature_registration,
};
use crate::handlers::group2::vote;

/// The pool's pending transactions, as the pool-entry checks read them.
pub trait PoolQuery {
    /// Whether the pool holds a transaction of `kind` from `sender`.
    fn has_sender_kind(&self, sender: &Address, kind: TransactionType) -> bool;
    /// Whether the pool holds a delegate registration for `username`.
    fn has_pending_username(&self, username: &str) -> bool;
}

/// Which delegates operate a node on the network: the node-local record of the delegates whose
/// forgers have shown themselves in recent rounds.
pub trait OnlineDelegates {
    /// Whether the delegate named `username` operates a node.
    fn is_online(&self, username: &str) -> bool;
}

/// Refuse `transaction` at pool entry, as the reference's `throwIfCannotEnterPool` of its type
/// does. Transfers and burns have no pool-entry rule. `ctx` is the pool's: the rules of the tip
/// plus one.
pub fn throw_if_cannot_enter_pool<P: PoolQuery, O: OnlineDelegates>(
    ctx: &ApplyContext<'_>,
    wallets: &WalletRepository,
    transaction: &Transaction,
    pool: &P,
    online: &O,
) -> Result<(), PoolError> {
    match &transaction.data().asset {
        Asset::Transfer { .. } | Asset::Burn { .. } => Ok(()),
        Asset::SecondSignature { .. } => second_signature_registration::throw_if_cannot_enter_pool(
            ctx,
            wallets,
            transaction,
            pool,
            online,
        ),
        Asset::DelegateRegistration { username } => {
            delegate_registration::throw_if_cannot_enter_pool(
                ctx,
                wallets,
                transaction,
                username,
                pool,
                online,
            )
        }
        Asset::DelegateResignation { resignation_type } => {
            delegate_resignation::throw_if_cannot_enter_pool(
                ctx,
                wallets,
                transaction,
                *resignation_type,
                pool,
                online,
            )
        }
        Asset::Vote { votes } => {
            vote::throw_if_cannot_enter_pool(ctx, wallets, transaction, votes, pool, online)
        }
    }
}
