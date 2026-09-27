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

use heartwood_crypto::{Transaction, TransactionId};

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
pub fn collate(
    pool: &mut Pool,
    ctx: &PoolContext<'_>,
    validate: bool,
    exclude: &[TransactionId],
) -> Result<Collated, StorageError> {
    let _ = (pool, ctx, validate, exclude);
    todo!("collate")
}
