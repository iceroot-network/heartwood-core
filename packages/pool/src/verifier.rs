//! The processor's first loop: each entry decoded and its signature checked, on up to
//! `worker_count` threads, the results in entry order.
//!
//! Bytes are decoded as the reference's strict `fromBytes` does, JSON as its `fromData` does,
//! under the rules of the tip plus one. A failure is `ERR_BAD_DATA` with the message
//! `Invalid transaction data: <reason>`. A signature that does not verify does not refuse the
//! entry here: the sender state refuses it later
//! ([`heartwood_crypto::Transaction::is_verified`]). A transaction of a removed type is kept as
//! [`Checked::Removed`], which the second loop refuses with `ERR_OTHER`, where the reference's
//! handler registry refuses it.

use heartwood_crypto::{Params, Transaction};

use crate::errors::Refusal;
use crate::processor::Entry;

/// An entry after the first loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Checked {
    /// A decoded transaction.
    Decoded(Box<Transaction>),
    /// A transaction of a removed type.
    Removed {
        /// Its id: the SHA-256 of the received bytes, or for JSON the given `id` (the entry's
        /// index when it has none).
        id: String,
        /// The type group.
        type_group: u32,
        /// The type.
        kind: u16,
    },
}

impl Checked {
    /// The id that the cooldown and the second loop use.
    pub fn id(&self) -> String {
        match self {
            Checked::Decoded(transaction) => transaction.id().to_hex(),
            Checked::Removed { id, .. } => id.clone(),
        }
    }
}

/// Decode and check `entries` under `params` on up to `worker_count` threads; the results are in
/// entry order and equal to those of one thread.
pub fn check_entries(
    entries: &[Entry],
    params: &Params,
    worker_count: usize,
) -> Vec<Result<Checked, Refusal>> {
    let _ = (entries, params, worker_count);
    todo!("decode and check the entries")
}
