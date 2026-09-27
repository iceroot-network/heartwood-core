//! The pool's settings, with the reference's defaults.

use std::path::{Path, PathBuf};

use heartwood_crypto::identities::Address;
use indexmap::IndexMap;

/// The pool's own fee rules, which apply when the milestone's dynamic fees are off.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PoolFees {
    /// Whether the pool's floor applies; when not, the milestone's static fee must be paid
    /// exactly.
    pub enabled: bool,
    /// The price of a byte to enter the pool.
    pub min_fee_pool: u64,
    /// The price of a byte to be broadcast.
    pub min_fee_broadcast: u64,
    /// The bytes added to each type's size, by type key, in the reference's order.
    pub addon_bytes: IndexMap<String, u64>,
}

/// The pool's settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    /// Whether the pool runs.
    pub enabled: bool,
    /// The file of the pool's storage, `pool.sqlite`.
    pub storage: PathBuf,
    /// When the pool holds this many transactions, a new one enters only with a fee above the
    /// lowest, which it then evicts.
    pub max_transactions_in_pool: usize,
    /// The most transactions a sender may have in the pool, unless it is in `allowed_senders`.
    pub max_transactions_per_sender: usize,
    /// The senders without a per-sender limit.
    pub allowed_senders: Vec<Address>,
    /// The most transactions one request may carry (read by the API and P2P).
    pub max_transactions_per_request: usize,
    /// How many blocks a transaction may stay in the pool.
    pub max_transaction_age: u32,
    /// The longest transaction the pool takes, in bytes.
    pub max_transaction_bytes: usize,
    /// The pool's own fee rules.
    pub dynamic_fees: PoolFees,
    /// The threads that decode and verify a batch: `min(max(1, ⌊cpus / 2⌋), 3)` by default.
    pub worker_count: usize,
}

impl Options {
    /// The reference's defaults for a node with data directory `data_dir` and `cpus` processors,
    /// reading the variables the reference's defaults read (`CORE_TRANSACTION_POOL_DISABLED`,
    /// `CORE_MAX_TRANSACTIONS_IN_POOL`, `CORE_TRANSACTION_POOL_MAX_PER_SENDER`,
    /// `CORE_TRANSACTION_POOL_MAX_PER_REQUEST`) through `env`.
    pub fn defaults(env: &dyn Fn(&str) -> Option<String>, data_dir: &Path, cpus: usize) -> Options {
        let _ = (env, data_dir, cpus);
        todo!("the reference's defaults")
    }
}
