//! The components' options that the relay reads itself (the pool's, the database's, the
//! logger's and the blockchain's) and the parts of the peer-to-peer options that the environment and the command line
//! give, each read as the reference's option schema reads it over its defaults.
//!
//! The reference's defaults read some variables with `||`, so a variable that is set and not
//! empty stands in for the default as text, which the schema then converts or refuses. The entry's
//! options in `app.json` are merged over that, and over the command line's options, as the
//! reference merges a plugin's options.

use std::path::{Path, PathBuf};

use heartwood_blockchain::BlockchainOptions;
use heartwood_crypto::identities::Address;
use heartwood_kernel::config::app::integer;
use heartwood_kernel::config::merge::deep_merge;
use heartwood_kernel::config::{Env, OptionsError};
use heartwood_logger::{Level, LoggerOptions};
use heartwood_p2p::P2pOptions;
use heartwood_pool::Options as PoolOptions;
use serde_json::{Map, Value};

/// The peer-to-peer package.
pub const P2P: &str = "@solar-network/p2p";
/// The pool package.
pub const POOL: &str = "@solar-network/pool";
/// The database package.
pub const DATABASE: &str = "@solar-network/database";
/// The logger package.
pub const LOGGER: &str = "@solar-network/logger";

/// The variables the peer-to-peer defaults read, and the option each stands in for.
const P2P_VARIABLES: &[(&str, &str)] = &[
    ("CORE_P2P_BAN_SECONDS", "server.banSeconds"),
    ("CORE_P2P_HOST", "server.hostname"),
    ("CORE_P2P_PORT", "server.port"),
    ("CORE_P2P_MIN_NETWORK_REACH", "minimumNetworkReach"),
    ("CORE_P2P_MAX_PEERS_SAME_SUBNET", "maxSameSubnetPeers"),
    (
        "CORE_P2P_MAX_PEER_SEQUENTIAL_ERRORS",
        "maxPeerSequentialErrors",
    ),
    ("CORE_P2P_RATE_LIMIT", "rateLimit"),
    (
        "CORE_P2P_RATE_LIMIT_POST_TRANSACTIONS",
        "rateLimitPostTransactions",
    ),
];

/// The variables the pool's defaults read as numbers.
const POOL_VARIABLES: &[(&str, &str)] = &[
    ("CORE_MAX_TRANSACTIONS_IN_POOL", "maxTransactionsInPool"),
    (
        "CORE_TRANSACTION_POOL_MAX_PER_SENDER",
        "maxTransactionsPerSender",
    ),
    (
        "CORE_TRANSACTION_POOL_MAX_PER_REQUEST",
        "maxTransactionsPerRequest",
    ),
];

/// A component's options, read key by key (a key is a dotted path); a refusal names the package
/// and the key.
pub struct Reader<'a> {
    package: &'static str,
    value: &'a Value,
}

impl<'a> Reader<'a> {
    /// A reader of `package`'s options `value`.
    pub fn new(package: &'static str, value: &'a Value) -> Reader<'a> {
        Reader { package, value }
    }

    /// The value at `key`, if it is there.
    pub fn get(&self, key: &str) -> Option<&'a Value> {
        key.split('.')
            .try_fold(self.value, |value, part| value.get(part))
    }

    fn refuse(&self, key: &str, reason: &str) -> OptionsError {
        let (package, key, reason) = (self.package, key.to_owned(), reason.to_owned());
        OptionsError {
            package,
            key,
            reason,
        }
    }

    fn read<T>(
        &self,
        key: &str,
        reason: &str,
        read: impl Fn(&Value) -> Option<T>,
    ) -> Result<Option<T>, OptionsError> {
        let value = self
            .get(key)
            .map(|value| read(value).ok_or_else(|| self.refuse(key, reason)));
        value.transpose()
    }

    /// A whole number of at least `min` (`Joi.number().integer().min(min)`).
    pub fn integer(&self, key: &str, min: i64) -> Result<Option<u64>, OptionsError> {
        let whole =
            |value: &Value| integer(value, min, i64::MAX).and_then(|n| u64::try_from(n).ok());
        self.read(
            key,
            &format!("must be a whole number of at least {min}"),
            whole,
        )
    }

    /// A boolean, or `true` or `false` text in any case (`Joi.bool()`).
    pub fn boolean(&self, key: &str) -> Result<Option<bool>, OptionsError> {
        self.read(key, "must be a boolean", |value| match value {
            Value::String(text) => ["false", "true"]
                .iter()
                .position(|b| text.eq_ignore_ascii_case(b))
                .map(|b| b == 1),
            value => value.as_bool(),
        })
    }

    /// Non-empty text (`Joi.string()`).
    pub fn text(&self, key: &str) -> Result<Option<String>, OptionsError> {
        let text = |value: &Value| {
            value
                .as_str()
                .filter(|text| !text.is_empty())
                .map(str::to_owned)
        };
        self.read(key, "must be text", text)
    }

    /// A list of non-empty text.
    pub fn texts(&self, key: &str) -> Result<Option<Vec<String>>, OptionsError> {
        let items = |value: &Value| -> Option<Vec<String>> {
            let text = |item: &Value| item.as_str().filter(|t| !t.is_empty()).map(str::to_owned);
            value.as_array()?.iter().map(text).collect()
        };
        self.read(key, "must be a list of text", items)
    }

    /// Refuse a value at `key` that is not an object.
    pub fn object(&self, key: &str) -> Result<(), OptionsError> {
        self.read(key, "must be an object", |value| {
            value.as_object().map(|_| ())
        })
        .map(|_| ())
    }
}

/// The variables of `env` among `variables` that are set and not empty, each as text at its
/// option's key path.
fn from_env(env: &Env, variables: &[(&str, &str)]) -> Value {
    let mut options = Value::Object(Map::new());
    for (variable, path) in variables {
        let Some(text) = env.get(variable).filter(|text| !text.is_empty()) else {
            continue;
        };
        let mut at = &mut options;
        for part in path.split('.') {
            let Value::Object(map) = at else { break };
            at = map.entry(part).or_insert_with(|| Value::Object(Map::new()));
        }
        *at = Value::String(text.to_owned());
    }
    options
}

/// The peer-to-peer options: `entry` (the entry's options over the command line's) over the
/// variables of `env`; the node's version, seeds and sources are the caller's to set.
pub fn p2p(entry: &Value, env: &Env) -> Result<P2pOptions, OptionsError> {
    let value = deep_merge(&from_env(env, P2P_VARIABLES), entry);
    let mut options = P2pOptions::from_json(&value).map_err(|error| OptionsError {
        package: P2P,
        key: error.key,
        reason: error.reason,
    })?;
    options.allow_private_ip_ranges = env.is_set("CORE_P2P_ALLOW_PRIVATE_IP_RANGES");
    Ok(options)
}

/// The pool's options for a node whose data directory is `data`, on `cpus` processors.
pub fn pool(
    entry: &Value,
    env: &Env,
    data: &Path,
    cpus: usize,
) -> Result<PoolOptions, OptionsError> {
    let value = deep_merge(&from_env(env, POOL_VARIABLES), entry);
    let read = Reader::new(POOL, &value);
    let mut o = PoolOptions::defaults(&env.lookup(), data, cpus);
    let size = |n: u64| usize::try_from(n).unwrap_or(usize::MAX);
    o.enabled = read.boolean("enabled")?.unwrap_or(o.enabled);
    o.storage = read.text("storage")?.map_or(o.storage, PathBuf::from);
    o.max_transactions_in_pool = read
        .integer("maxTransactionsInPool", 1)?
        .map_or(o.max_transactions_in_pool, size);
    o.max_transactions_per_sender = read
        .integer("maxTransactionsPerSender", 1)?
        .map_or(o.max_transactions_per_sender, size);
    o.max_transactions_per_request = read
        .integer("maxTransactionsPerRequest", 1)?
        .map_or(o.max_transactions_per_request, size);
    if let Some(senders) = read.texts("allowedSenders")? {
        // A text that is no address can match no sender, as in the reference.
        o.allowed_senders = senders
            .iter()
            .filter_map(|sender| Address::from_base58(sender).ok())
            .collect();
    }
    let age = read.integer("maxTransactionAge", 1)?;
    o.max_transaction_age = age.map_or(o.max_transaction_age, |n| {
        u32::try_from(n).unwrap_or(u32::MAX)
    });
    o.max_transaction_bytes = read
        .integer("maxTransactionBytes", 1)?
        .map_or(o.max_transaction_bytes, size);
    read.object("dynamicFees")?;
    let fees = &mut o.dynamic_fees;
    fees.enabled = read.boolean("dynamicFees.enabled")?.unwrap_or(fees.enabled);
    fees.min_fee_pool = read
        .integer("dynamicFees.minFeePool", 0)?
        .unwrap_or(fees.min_fee_pool);
    fees.min_fee_broadcast = read
        .integer("dynamicFees.minFeeBroadcast", 0)?
        .unwrap_or(fees.min_fee_broadcast);
    read.object("dynamicFees.addonBytes")?;
    let addon = read
        .get("dynamicFees.addonBytes")
        .and_then(Value::as_object);
    for key in addon.into_iter().flat_map(Map::keys) {
        let bytes = read
            .integer(&format!("dynamicFees.addonBytes.{key}"), 0)?
            .unwrap_or_default();
        fees.addon_bytes.insert(key.clone(), bytes);
    }
    read.object("workerPool")?;
    o.worker_count = read
        .integer("workerPool.workerCount", 1)?
        .map_or(o.worker_count, size);
    Ok(o)
}

/// The database's options, checked as the reference's schema checks the keys they give; its
/// `apiConnectionTimeout` in milliseconds. The connection's options are the reference's store's
/// and mean nothing to this one's, and its user name, which the reference takes from `USER`, is
/// not required.
pub fn database(entry: &Value) -> Result<u64, OptionsError> {
    let read = Reader::new(DATABASE, entry);
    let timeout = read.integer("apiConnectionTimeout", 1)?.unwrap_or(3_000);
    for key in ["connection", "connection.extra"] {
        read.object(key)?;
    }
    for key in ["database", "entityPrefix", "type", "username", "extra.host"] {
        read.text(&format!("connection.{key}"))?;
    }
    for key in ["connection.logging", "connection.synchronise"] {
        read.boolean(key)?;
    }
    Ok(timeout)
}

/// The blockchain's options over its defaults: `databaseRollback` an object whose
/// `maxBlockRewind` and `steps` are whole numbers of at least 1, `networkStart` a boolean, any
/// other key allowed without a check. `missedBlocksLookback` has no rule in the reference's schema
/// and no reader before the productivity tables; a value that is not a whole number keeps the
/// default. A rewind bound above 2^32 - 1 is refused (the reference takes one up to 2^53 - 1).
pub fn blockchain(entry: &Value) -> Result<BlockchainOptions, OptionsError> {
    let (read, mut options) = (
        Reader::new(BlockchainOptions::PACKAGE, entry),
        BlockchainOptions::default(),
    );
    read.object("databaseRollback")?;
    let bounds = &mut options.database_rollback;
    for (key, field) in [
        (
            "databaseRollback.maxBlockRewind",
            &mut bounds.max_block_rewind,
        ),
        ("databaseRollback.steps", &mut bounds.steps),
    ] {
        if let Some(given) = read.integer(key, 1)? {
            let refused = |_| read.refuse(key, "must be at most 4294967295");
            *field = u32::try_from(given).map_err(refused)?;
        }
    }
    options.network_start = read.boolean("networkStart")?.unwrap_or(false);
    let lookback = read.get("missedBlocksLookback").and_then(Value::as_u64);
    options.missed_blocks_lookback = lookback.unwrap_or(options.missed_blocks_lookback);
    Ok(options)
}

/// The logger's options: its levels (`CORE_LOG_LEVEL`, `CORE_LOG_LEVEL_FILE`, `info` and `debug`
/// by default, by the reference's level names) and whether emoji codes become emoji.
pub fn logger(entry: &Value, env: &Env) -> Result<LoggerOptions, OptionsError> {
    let variables = [
        ("CORE_LOG_LEVEL", "levels.console"),
        ("CORE_LOG_LEVEL_FILE", "levels.file"),
    ];
    let value = deep_merge(&from_env(env, &variables), entry);
    let read = Reader::new(LOGGER, &value);
    let level = |key: &str, default: Level| match read.text(key)? {
        Some(name) => Level::from_name(&name)
            .map(Some)
            .ok_or_else(|| read.refuse(key, "is not a log level")),
        None => Ok(Some(default)),
    };
    // The reference reads `emojify` as JavaScript's truthiness.
    let falsy = |value: &Value| {
        matches!(value, Value::Null | Value::Bool(false)) || *value == 0 || value == ""
    };
    Ok(LoggerOptions {
        console: level("levels.console", Level::Info)?,
        file: level("levels.file", Level::Debug)?,
        emojify: read.get("emojify").is_none_or(|value| !falsy(value)),
        ..LoggerOptions::default()
    })
}
