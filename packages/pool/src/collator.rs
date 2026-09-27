//! The transactions of a block candidate, as the reference's collator chooses them.
//!
//! Under the rules of the tip plus one, with the byte budget of the milestone at the tip:
//! `maxPayload − 141` bytes. The transactions are taken from the highest priority, `exclude`
//! skipped; the collation stops at `maxTransactions`, or when the next transaction's 4-byte
//! length and bytes no longer fit (a stop, not a skip). After one of a sender's transactions
//! fails, the sender's later ones are skipped. With `validate`, each candidate is decoded again
//! strictly and applied with the handlers to one copy of the state's wallets, shared by every
//! candidate. The transactions that failed are then removed from the pool with their senders'
//! later ones.

use heartwood_crypto::identities::Address;
use heartwood_crypto::{Transaction, TransactionId};
use heartwood_state::TransactionHandlers;
use heartwood_transactions::Handlers;

use crate::errors::StorageError;
use crate::service::{Pool, PoolContext};

/// The bytes of a block header before its transactions, which the byte budget leaves out.
pub const BLOCK_HEADER_SIZE: u32 = 141;

/// What a collation chose and removed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Collated {
    /// The block's transactions, in block order.
    pub candidates: Vec<Transaction>,
    /// The transactions removed from the pool because they failed, with their senders' later
    /// ones.
    pub removed: Vec<Transaction>,
}

/// Collate a block candidate from `pool`.
///
/// A candidate that fails (with `validate`: it does not decode strictly with the same id, or its
/// handler refuses it on the shared copy of the wallets) is removed from the pool with its
/// sender's later transactions once the candidates are chosen, as the reference's collator does
/// after it returns.
pub fn collate(
    pool: &mut Pool,
    ctx: &PoolContext<'_>,
    validate: bool,
    exclude: &[TransactionId],
) -> Result<Collated, StorageError> {
    let block = ctx.milestones.at(ctx.tip);
    let params = ctx.params();
    let apply_ctx = ctx.apply_context();
    let max_transactions = block.max_transactions().map(|max| max as usize);
    // Without `maxPayload` the reference's budget is NaN, which never runs out.
    let mut bytes_left = block
        .max_payload()
        .map(|payload| i64::from(payload) - i64::from(BLOCK_HEADER_SIZE));
    let mut wallets = validate.then(|| ctx.wallets.clone());
    let mut candidates: Vec<Transaction> = Vec::new();
    let mut failed: Vec<(Address, TransactionId)> = Vec::new();
    for transaction in pool.query().from_highest_priority() {
        if exclude.contains(transaction.id()) {
            continue;
        }
        if max_transactions == Some(candidates.len()) {
            break;
        }
        let sender = transaction.data().sender_address(params);
        if failed.iter().any(|(failed, _)| *failed == sender) {
            continue;
        }
        let size = i64::try_from(transaction.bytes().len()).unwrap_or(i64::MAX);
        if bytes_left.is_some_and(|left| left - 4 - size < 0) {
            break;
        }
        let candidate = match &mut wallets {
            None => Ok(transaction.clone()),
            Some(wallets) => Transaction::decode(transaction.bytes(), params)
                .map_err(|_| ())
                .and_then(|decoded| {
                    if decoded.id() != transaction.id() {
                        return Err(());
                    }
                    Handlers
                        .apply(&apply_ctx, wallets, &decoded)
                        .map(|_| decoded)
                        .map_err(|_| ())
                }),
        };
        match candidate {
            Ok(candidate) => {
                let size = i64::try_from(candidate.bytes().len()).unwrap_or(i64::MAX);
                bytes_left = bytes_left.map(|left| left - 4 - size);
                candidates.push(candidate);
            }
            Err(()) => failed.push((sender, *transaction.id())),
        }
    }
    let mut removed = Vec::new();
    for (sender, id) in failed {
        removed.extend(pool.remove_transaction(ctx, &sender, &id)?);
    }
    Ok(Collated {
        candidates,
        removed,
    })
}
