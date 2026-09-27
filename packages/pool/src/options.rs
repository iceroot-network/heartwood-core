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

impl PoolFees {
    /// The reference's defaults: enabled, 3,000 per byte to enter and to be broadcast, and its
    /// table of added bytes (which still names the types this implementation does not carry).
    pub fn defaults() -> PoolFees {
        let addon_bytes = [
            ("legacyTransfer", 100),
            ("secondSignature", 250),
            ("delegateRegistration", 400_000),
            ("vote", 100),
            ("multiSignature", 500),
            ("ipfs", 250),
            ("transfer", 500),
            ("delegateResignation", 100),
            ("htlcLock", 100),
            ("htlcClaim", 0),
            ("htlcRefund", 0),
        ]
        .into_iter()
        .map(|(key, bytes)| (key.to_owned(), bytes))
        .collect();
        PoolFees {
            enabled: true,
            min_fee_pool: 3_000,
            min_fee_broadcast: 3_000,
            addon_bytes,
        }
    }
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
    ///
    /// Any non-empty `CORE_TRANSACTION_POOL_DISABLED` disables the pool, as the reference's
    /// `!process.env...` does. A limit variable that is set and non-empty replaces the default
    /// when it is a decimal integer of at least 1; any other value leaves the default here, and
    /// the node's options loader refuses it, as the reference's option schema does.
    pub fn defaults(env: &dyn Fn(&str) -> Option<String>, data_dir: &Path, cpus: usize) -> Options {
        let limit = |name: &str, default: usize| {
            env(name)
                .and_then(|value| value.trim().parse::<usize>().ok())
                .filter(|value| *value >= 1)
                .unwrap_or(default)
        };
        Options {
            enabled: env("CORE_TRANSACTION_POOL_DISABLED").is_none_or(|value| value.is_empty()),
            storage: data_dir.join("pool").join("pool.sqlite"),
            max_transactions_in_pool: limit("CORE_MAX_TRANSACTIONS_IN_POOL", 15_000),
            max_transactions_per_sender: limit("CORE_TRANSACTION_POOL_MAX_PER_SENDER", 150),
            allowed_senders: Vec::new(),
            max_transactions_per_request: limit("CORE_TRANSACTION_POOL_MAX_PER_REQUEST", 40),
            max_transaction_age: 2_700,
            max_transaction_bytes: 2_000_000,
            dynamic_fees: PoolFees::defaults(),
            worker_count: (cpus / 2).clamp(1, 3),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_reference_defaults() {
        let options = Options::defaults(&|_| None, Path::new("/data"), 8);
        assert!(options.enabled);
        assert_eq!(options.storage, Path::new("/data/pool/pool.sqlite"));
        assert_eq!(options.max_transactions_in_pool, 15_000);
        assert_eq!(options.max_transactions_per_sender, 150);
        assert!(options.allowed_senders.is_empty());
        assert_eq!(options.max_transactions_per_request, 40);
        assert_eq!(options.max_transaction_age, 2_700);
        assert_eq!(options.max_transaction_bytes, 2_000_000);
        assert_eq!(options.worker_count, 3);
        let fees = &options.dynamic_fees;
        assert!(fees.enabled);
        assert_eq!((fees.min_fee_pool, fees.min_fee_broadcast), (3_000, 3_000));
        assert_eq!(fees.addon_bytes.len(), 11);
        assert_eq!(
            fees.addon_bytes.get_index(0),
            Some((&"legacyTransfer".to_owned(), &100))
        );
        assert_eq!(fees.addon_bytes["delegateRegistration"], 400_000);
        assert_eq!(fees.addon_bytes["transfer"], 500);
    }

    #[test]
    fn worker_count_follows_the_processors() {
        let count = |cpus| Options::defaults(&|_| None, Path::new("d"), cpus).worker_count;
        assert_eq!(
            [0, 1, 2, 3, 4, 5, 6, 7, 64].map(count),
            [1, 1, 1, 1, 2, 2, 3, 3, 3]
        );
    }

    #[test]
    fn variables_replace_the_defaults() {
        let env = |name: &str| match name {
            "CORE_TRANSACTION_POOL_DISABLED" => Some("1".to_owned()),
            "CORE_MAX_TRANSACTIONS_IN_POOL" => Some("500".to_owned()),
            "CORE_TRANSACTION_POOL_MAX_PER_SENDER" => Some("x".to_owned()),
            "CORE_TRANSACTION_POOL_MAX_PER_REQUEST" => Some("0".to_owned()),
            _ => None,
        };
        let options = Options::defaults(&env, Path::new("d"), 1);
        assert!(!options.enabled);
        assert_eq!(options.max_transactions_in_pool, 500);
        assert_eq!(options.max_transactions_per_sender, 150);
        assert_eq!(options.max_transactions_per_request, 40);
        let empty = |name: &str| (name == "CORE_TRANSACTION_POOL_DISABLED").then(String::new);
        assert!(Options::defaults(&empty, Path::new("d"), 1).enabled);
    }
}
