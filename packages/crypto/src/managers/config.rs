//! The milestones, `milestones.json`: loading, merging and the start-up checks.
//!
//! Loading sorts the milestones by height (milestones of equal height keep their file order) and
//! merges each one onto the merged milestone before it, as the reference does:
//!
//! - objects merge key by key, and a key the earlier milestone lacks is copied;
//! - arrays, and every value that is not an object, replace the earlier value;
//! - `dynamicReward.ranks` and the top-level `donations` are replaced whole by a milestone that
//!   states them, so a later milestone can shrink the rank table or clear the donations with `{}`.
//!   Nowhere else does a key with one of these names replace instead of merge.
//!
//! Every merged milestone then passes the reference's start-up checks, and a refusal names the
//! height and the key:
//!
//! - `blockTime` and `activeDelegates`: stated by the first milestone, each a JSON number whose
//!   double value is an integer from 1 to `2^53 − 1`, and unchanged at every later milestone
//!   (compared as doubles, so a restated `8.0` is still 8);
//! - `burn`: an object; `burn.feePercent` is refused; `burn.feeBasisPoints` is an integer from 0
//!   to 10,000; `burn.txAmount` is a JSON number whose double value is an integer from 0 to
//!   `2^53 − 1`;
//! - `donations`, when present: an object of addresses of this network, each with `basisPoints`
//!   (an integer from 1 to 10,000) and an optional string `purpose`, nothing else, adding up to at
//!   most 10,000;
//! - when `dynamicReward` is an object whose `enabled` is truthy: a reward for every rank from 1
//!   to `activeDelegates`, and a `secondaryReward`, each a non-negative safe integer or a
//!   canonical decimal string that fits 64 bits.
//!
//! The loader also refuses, at start, files that the reference accepts but that would break it
//! later or that this implementation cannot represent. These are configuration-only differences:
//!
//! - the file must be a non-empty array of objects, each with a `height` whose double value is an
//!   integer from 1 to `2^32 − 1`, and the first milestone must be at height 1;
//! - no key, at any depth, may be named after a property of JavaScript's `Object.prototype`
//!   (`constructor`, `__proto__`, `toString` and the like): the reference's merge drops such keys,
//!   or turns them into an object's prototype;
//! - the first milestone's `epoch` must be a date and time with a time zone (see
//!   [`crate::crypto::slots::epoch_unix_ms`]);
//! - `block` must be an object with an integer `version`, and `maxTransactions` and `maxPayload`,
//!   when present, must be positive integers that fit 32 bits;
//! - `transfer` must be an object; `minimum` and `maximum` default to 1 and 256 when they are
//!   missing or falsy, as in the reference, and must otherwise be positive integers;
//! - `reward` defaults to 0 when missing or `null` and must otherwise be a reward amount, like the
//!   rank table's entries;
//! - every entry of an enabled rank table must be a reward amount, not only the active ranks;
//! - the text must be JSON that `serde_json` reads: `JSON.parse` also accepts numbers too large
//!   for a double (as infinity), unpaired surrogate escapes, and nesting deeper than 128 levels.
//!
//! JSON numbers are read as `JSON.parse` reads them (see [`crate::utils::js`]), so `8.0` and `8e0`
//! are 8, and `9007199254740993` is `2^53`.

use indexmap::IndexMap;
use serde_json::{Map, Value};

use crate::crypto::slots;
use crate::errors::ConfigError;
use crate::identities::Address;
use crate::managers::Network;
use crate::utils::js;

/// The own properties of JavaScript's `Object.prototype`, which no milestone key may be named.
const OBJECT_PROTOTYPE_KEYS: [&str; 12] = [
    "__proto__",
    "__defineGetter__",
    "__defineSetter__",
    "__lookupGetter__",
    "__lookupSetter__",
    "constructor",
    "hasOwnProperty",
    "isPrototypeOf",
    "propertyIsEnumerable",
    "toLocaleString",
    "toString",
    "valueOf",
];

/// 100 % in basis points.
const ALL_BASIS_POINTS: i64 = 10_000;

/// The transfer limits the reference falls back to when the milestone states none.
const DEFAULT_TRANSFER_MINIMUM: u32 = 1;
const DEFAULT_TRANSFER_MAXIMUM: u32 = 256;

const REWARD_AMOUNT: &str = "must be a non-negative safe integer or a decimal integer string of at most 18446744073709551615";

/// A donation recipient.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Donation {
    pub(crate) address: Address,
    pub(crate) basis_points: u16,
    pub(crate) purpose: Option<String>,
}

impl Donation {
    /// The recipient.
    pub fn address(&self) -> &Address {
        &self.address
    }

    /// The share of the block reward, in basis points.
    pub fn basis_points(&self) -> u16 {
        self.basis_points
    }

    /// The purpose, if the file states one.
    pub fn purpose(&self) -> Option<&str> {
        self.purpose.as_deref()
    }
}

/// The reward table by delegate rank.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DynamicReward {
    pub(crate) enabled: bool,
    pub(crate) ranks: IndexMap<String, u64>,
    pub(crate) secondary_reward: u64,
}

impl DynamicReward {
    /// Whether the table is in force.
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// The reward of `rank`, if the table has one.
    pub fn rank(&self, rank: u32) -> Option<u64> {
        self.ranks.get(&rank.to_string()).copied()
    }

    /// The secondary reward, `dynamicReward.secondaryReward` (0 when a disabled table has none).
    pub fn secondary_reward(&self) -> u64 {
        self.secondary_reward
    }
}

/// The dynamic fee table, `dynamicFees`: the fee floor of a transaction is
/// `(addonBytes[key] + round(size / 2)) × max(minFee, 1)` while it is enabled.
///
/// The loader refuses values the reference would coerce or trip over later: `enabled` must be a
/// boolean, and `minFee` and every `addonBytes` value a JSON number whose double value is a safe
/// integer. Any key is accepted inside `addonBytes`, so files that still list removed types load.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DynamicFees {
    pub(crate) enabled: bool,
    pub(crate) min_fee: i64,
    pub(crate) addon_bytes: IndexMap<String, i64>,
}

impl DynamicFees {
    /// Whether the fee floor is in force.
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// `minFee`, the price of a byte (the floor uses 1 when it is below 1).
    pub fn min_fee(&self) -> i64 {
        self.min_fee
    }

    /// `addonBytes[key]` for a transaction type key; 0 when the table has none.
    pub fn addon(&self, key: &str) -> i64 {
        self.addon_bytes.get(key).copied().unwrap_or(0)
    }
}

/// The merged milestone in force from one height: every parameter a rule reads.
#[derive(Debug, Clone, PartialEq)]
pub struct Params {
    pub(crate) height: u32,
    pub(crate) network: u8,
    pub(crate) epoch: String,
    pub(crate) block_time: u64,
    pub(crate) active_delegates: u64,
    pub(crate) block_version: u32,
    pub(crate) max_transactions: Option<u32>,
    pub(crate) max_payload: Option<u32>,
    pub(crate) transfer_minimum: u32,
    pub(crate) transfer_maximum: u32,
    pub(crate) reward: u64,
    pub(crate) dynamic_reward: Option<DynamicReward>,
    pub(crate) fee_basis_points: u16,
    pub(crate) burn_tx_amount: u64,
    pub(crate) donations: Vec<Donation>,
    pub(crate) json: Map<String, Value>,
}

impl Params {
    /// The height this merged milestone starts at.
    pub fn height(&self) -> u32 {
        self.height
    }

    /// The address network byte.
    pub fn network(&self) -> u8 {
        self.network
    }

    /// The network epoch: the first milestone's `epoch` text, which is the one the reference reads
    /// at every height.
    pub fn epoch(&self) -> &str {
        &self.epoch
    }

    /// The block time in seconds; the same at every height.
    pub fn block_time(&self) -> u64 {
        self.block_time
    }

    /// The number of active delegates; the same at every height.
    pub fn active_delegates(&self) -> u64 {
        self.active_delegates
    }

    /// The block version.
    pub fn block_version(&self) -> u32 {
        self.block_version
    }

    /// The most transactions a block may hold, if set.
    pub fn max_transactions(&self) -> Option<u32> {
        self.max_transactions
    }

    /// The largest block payload in bytes, if set.
    pub fn max_payload(&self) -> Option<u32> {
        self.max_payload
    }

    /// The fewest items a transfer may have.
    pub fn transfer_minimum(&self) -> u32 {
        self.transfer_minimum
    }

    /// The most items a transfer may have.
    pub fn transfer_maximum(&self) -> u32 {
        self.transfer_maximum
    }

    /// The block reward when the dynamic reward is off (0 when the milestone states none).
    pub fn reward(&self) -> u64 {
        self.reward
    }

    /// The reward table by rank, if the milestone has one.
    pub fn dynamic_reward(&self) -> Option<&DynamicReward> {
        self.dynamic_reward.as_ref()
    }

    /// The share of every fee that is burned, in basis points.
    pub fn fee_basis_points(&self) -> u16 {
        self.fee_basis_points
    }

    /// The smallest amount a burn transaction may burn.
    pub fn burn_tx_amount(&self) -> u64 {
        self.burn_tx_amount
    }

    /// The donation recipients, in file order.
    pub fn donations(&self) -> &[Donation] {
        &self.donations
    }

    /// The dynamic fee table; `None` when the merged milestone has no `dynamicFees`, which means
    /// no fee floor.
    pub fn dynamic_fees(&self) -> Option<&DynamicFees> {
        todo!()
    }

    /// `blocksToRevokeDelegateResignation`: the blocks a temporarily resigned delegate waits
    /// before it may revoke its resignation; `None` when the merged milestone has none, which
    /// means no wait (the reference compares with `undefined`).
    pub fn blocks_to_revoke_delegate_resignation(&self) -> Option<u64> {
        todo!()
    }

    /// The merged milestone as JSON, keys in the order the reference enumerates them.
    pub fn as_json(&self) -> &Map<String, Value> {
        &self.json
    }
}

/// The merged milestones of a network, in height order.
#[derive(Debug, Clone, PartialEq)]
pub struct Milestones {
    /// Never empty, and the first entry starts at height 1.
    merged: Vec<Params>,
}

impl Milestones {
    /// Read `milestones.json` for `network`: sort, merge, and run every start-up check (see the
    /// module documentation).
    pub fn load(json: &str, network: &Network) -> Result<Milestones, ConfigError> {
        let value: Value =
            serde_json::from_str(json).map_err(|error| ConfigError::Json(error.to_string()))?;
        let Value::Array(entries) = value else {
            return Err(ConfigError::Shape("the milestones are not an array"));
        };
        if entries.is_empty() {
            return Err(ConfigError::Shape("there are no milestones"));
        }

        let mut raw = Vec::with_capacity(entries.len());
        for (index, entry) in entries.into_iter().enumerate() {
            let Value::Object(milestone) = entry else {
                return Err(ConfigError::Shape("a milestone is not an object"));
            };
            if let Some(path) = reserved_key(&milestone, "") {
                return Err(ConfigError::ReservedKey { index, path });
            }
            let height = milestone
                .get("height")
                .and_then(milestone_height)
                .ok_or(ConfigError::Height { index })?;
            raw.push((height, normalise_object(milestone)));
        }
        // A stable sort, as the reference's: milestones of equal height keep their file order.
        raw.sort_by_key(|(height, _)| *height);

        let mut merged: Vec<(u32, Map<String, Value>)> = Vec::with_capacity(raw.len());
        for (height, milestone) in raw {
            let next = match merged.last() {
                Some((_, previous)) => merge_objects(previous, &milestone, Scope::Milestone),
                None => milestone,
            };
            merged.push((height, next));
        }
        check(merged, network)
    }

    /// The merged milestone in force at `height`: the last one that starts at or below it (the
    /// first one at height 0, which the reference reads as height 1).
    #[expect(
        clippy::indexing_slicing,
        reason = "load never builds an empty list, and the index is below its length"
    )]
    pub fn at(&self, height: u32) -> &Params {
        let starting_at_or_below = self
            .merged
            .partition_point(|params| params.height <= height);
        &self.merged[starting_at_or_below.saturating_sub(1)]
    }

    /// Every merged milestone, in height order.
    pub fn all(&self) -> &[Params] {
        &self.merged
    }
}

/// The height of a milestone: a JSON number whose double value is an integer from 1 to
/// `2^32 − 1`.
fn milestone_height(value: &Value) -> Option<u32> {
    js::safe_integer(value)
        .and_then(|height| u32::try_from(height).ok())
        .filter(|height| *height >= 1)
}

/// The dotted path of the first key, at any depth, that is named after a property of
/// `Object.prototype`.
fn reserved_key(map: &Map<String, Value>, prefix: &str) -> Option<String> {
    map.iter().find_map(|(key, value)| {
        let path = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{prefix}.{key}")
        };
        if OBJECT_PROTOTYPE_KEYS.contains(&key.as_str()) {
            Some(path)
        } else {
            reserved_key_in(value, &path)
        }
    })
}

fn reserved_key_in(value: &Value, path: &str) -> Option<String> {
    match value {
        Value::Object(map) => reserved_key(map, path),
        Value::Array(items) => items
            .iter()
            .enumerate()
            .find_map(|(index, item)| reserved_key_in(item, &format!("{path}.{index}"))),
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => None,
    }
}

/// `map` with the keys of every object in the order the reference enumerates them after
/// `JSON.parse`.
fn normalise_object(map: Map<String, Value>) -> Map<String, Value> {
    js::in_property_order(
        map.into_iter()
            .map(|(key, value)| (key, normalise(value)))
            .collect(),
    )
}

fn normalise(value: Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(normalise_object(map)),
        Value::Array(items) => Value::Array(items.into_iter().map(normalise).collect()),
        other => other,
    }
}

/// Where in a milestone a merge happens, which decides the keys that replace instead of merge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scope {
    /// The top level: `donations` replaces, `dynamicReward` merges with its own rule.
    Milestone,
    /// Inside `dynamicReward`: `ranks` replaces.
    DynamicReward,
    /// Anywhere else: everything merges.
    Plain,
}

/// `source` merged onto `target`: the target's keys in their order, each overwritten or merged by
/// the source, then the source's new keys.
fn merge_objects(
    target: &Map<String, Value>,
    source: &Map<String, Value>,
    scope: Scope,
) -> Map<String, Value> {
    let mut merged = target.clone();
    for (key, value) in source {
        let next = match (target.get(key), value) {
            (Some(existing), Value::Object(_) | Value::Array(_)) => {
                merge_member(scope, key, existing, value)
            }
            _ => value.clone(),
        };
        merged.insert(key.clone(), next);
    }
    js::in_property_order(merged)
}

fn merge_member(scope: Scope, key: &str, target: &Value, source: &Value) -> Value {
    match (scope, key) {
        (Scope::Milestone, "donations") | (Scope::DynamicReward, "ranks") => source.clone(),
        (Scope::Milestone, "dynamicReward") => merge_values(target, source, Scope::DynamicReward),
        _ => merge_values(target, source, Scope::Plain),
    }
}

/// Two objects merge; any other pair gives the source (an array replaces, and an object replaces
/// a value that is not one).
fn merge_values(target: &Value, source: &Value, scope: Scope) -> Value {
    match (target, source) {
        (Value::Object(target), Value::Object(source)) => {
            Value::Object(merge_objects(target, source, scope))
        }
        _ => source.clone(),
    }
}

/// Run the start-up checks on the merged milestones and read their parameters.
fn check(
    merged: Vec<(u32, Map<String, Value>)>,
    network: &Network,
) -> Result<Milestones, ConfigError> {
    let Some((first_height, first)) = merged.first() else {
        return Err(ConfigError::Shape("there are no milestones"));
    };
    let first_height = u64::from(*first_height);
    if first_height != 1 {
        return Err(ConfigError::Invalid {
            height: first_height,
            key: "height".to_owned(),
            reason: "the first milestone must be at height 1",
        });
    }
    let block_time = positive_integer(first, "blockTime", first_height)?;
    let active_delegates = positive_integer(first, "activeDelegates", first_height)?;
    let epoch = epoch(first, first_height)?;

    let mut all = Vec::with_capacity(merged.len());
    for (height, milestone) in merged {
        let at = u64::from(height);
        for (key, fixed) in [
            ("blockTime", block_time),
            ("activeDelegates", active_delegates),
        ] {
            // The value is a safe integer, so the double holds it exactly.
            let unchanged = matches!(
                milestone.get(key),
                Some(Value::Number(number)) if js::number_value(number) == fixed as f64
            );
            if !unchanged {
                return Err(ConfigError::Changed {
                    height: at,
                    key,
                    first_height,
                });
            }
        }
        let (fee_basis_points, burn_tx_amount) = burn(&milestone, at)?;
        let donations = donations(&milestone, at, network.pub_key_hash)?;
        let dynamic_reward = dynamic_reward(&milestone, at, active_delegates)?;
        let (block_version, max_transactions, max_payload) = block(&milestone, at)?;
        let (transfer_minimum, transfer_maximum) = transfer(&milestone, at)?;
        let reward = reward(&milestone, at)?;
        all.push(Params {
            height,
            network: network.pub_key_hash,
            epoch: epoch.clone(),
            block_time,
            active_delegates,
            block_version,
            max_transactions,
            max_payload,
            transfer_minimum,
            transfer_maximum,
            reward,
            dynamic_reward,
            fee_basis_points,
            burn_tx_amount,
            donations,
            json: milestone,
        });
    }
    Ok(Milestones { merged: all })
}

fn missing(height: u64, key: impl Into<String>) -> ConfigError {
    ConfigError::Missing {
        height,
        key: key.into(),
    }
}

fn invalid(height: u64, key: impl Into<String>, reason: &'static str) -> ConfigError {
    ConfigError::Invalid {
        height,
        key: key.into(),
        reason,
    }
}

/// `blockTime` or `activeDelegates` of the first milestone: an integer from 1 to `2^53 − 1`.
fn positive_integer(
    milestone: &Map<String, Value>,
    key: &'static str,
    height: u64,
) -> Result<u64, ConfigError> {
    let value = milestone.get(key).ok_or_else(|| missing(height, key))?;
    js::safe_integer(value)
        .filter(|integer| *integer >= 1)
        .and_then(|integer| u64::try_from(integer).ok())
        .ok_or_else(|| invalid(height, key, "must be an integer from 1 to 2^53 - 1"))
}

/// The first milestone's `epoch`.
fn epoch(milestone: &Map<String, Value>, height: u64) -> Result<String, ConfigError> {
    match milestone.get("epoch") {
        Some(Value::String(text)) if slots::epoch_unix_ms(text).is_some() => Ok(text.clone()),
        Some(_) => Err(invalid(
            height,
            "epoch",
            "must be a date and time with a time zone, such as 2026-01-01T00:00:00.000Z",
        )),
        None => Err(missing(height, "epoch")),
    }
}

/// `burn.feeBasisPoints` and `burn.txAmount`.
fn burn(milestone: &Map<String, Value>, height: u64) -> Result<(u16, u64), ConfigError> {
    let burn = match milestone.get("burn") {
        Some(Value::Object(burn)) => burn,
        Some(_) => {
            return Err(invalid(
                height,
                "burn",
                "must be an object with feeBasisPoints (9000 = 90%)",
            ));
        }
        None => return Err(missing(height, "burn")),
    };
    if burn.contains_key("feePercent") {
        return Err(ConfigError::Unsupported {
            height,
            key: "burn.feePercent".to_owned(),
        });
    }
    let fee_basis_points = match burn.get("feeBasisPoints") {
        Some(value) => js::safe_integer(value)
            .filter(|basis_points| (0..=ALL_BASIS_POINTS).contains(basis_points))
            .and_then(|basis_points| u16::try_from(basis_points).ok())
            .ok_or_else(|| {
                invalid(
                    height,
                    "burn.feeBasisPoints",
                    "must be an integer from 0 to 10000",
                )
            })?,
        None => return Err(missing(height, "burn.feeBasisPoints")),
    };
    let tx_amount = match burn.get("txAmount") {
        Some(value) => js::safe_integer(value)
            .and_then(|amount| u64::try_from(amount).ok())
            .ok_or_else(|| {
                invalid(
                    height,
                    "burn.txAmount",
                    "the minimum burn amount in base units, must be a non-negative integer",
                )
            })?,
        None => return Err(missing(height, "burn.txAmount")),
    };
    Ok((fee_basis_points, tx_amount))
}

/// The donation recipients, when the milestone has `donations`.
fn donations(
    milestone: &Map<String, Value>,
    height: u64,
    network: u8,
) -> Result<Vec<Donation>, ConfigError> {
    let Some(value) = milestone.get("donations") else {
        return Ok(Vec::new());
    };
    let Value::Object(entries) = value else {
        return Err(invalid(
            height,
            "donations",
            "must be an object that maps each address to { basisPoints, purpose }",
        ));
    };
    let mut donations = Vec::with_capacity(entries.len());
    let mut sum: u64 = 0;
    for (text, entry) in js::property_order(entries) {
        let path = format!("donations.{text}");
        let address = Address::from_base58_for(text, network)
            .map_err(|_| invalid(height, &path, "is not a valid address of this network"))?;
        let Value::Object(fields) = entry else {
            return Err(invalid(height, path, "must be an object with basisPoints"));
        };
        if let Some(key) = fields
            .keys()
            .find(|key| key.as_str() != "basisPoints" && key.as_str() != "purpose")
        {
            return Err(invalid(
                height,
                format!("{path}.{key}"),
                "is not allowed: a share is set by basisPoints (500 = 5%)",
            ));
        }
        let basis_points = match fields.get("basisPoints") {
            Some(value) => js::safe_integer(value)
                .filter(|basis_points| (1..=ALL_BASIS_POINTS).contains(basis_points))
                .and_then(|basis_points| u16::try_from(basis_points).ok())
                .ok_or_else(|| {
                    invalid(
                        height,
                        format!("{path}.basisPoints"),
                        "must be an integer from 1 to 10000",
                    )
                })?,
            None => return Err(missing(height, format!("{path}.basisPoints"))),
        };
        let purpose = match fields.get("purpose") {
            None => None,
            Some(Value::String(purpose)) => Some(purpose.clone()),
            Some(_) => {
                return Err(invalid(
                    height,
                    format!("{path}.purpose"),
                    "must be a string",
                ));
            }
        };
        sum += u64::from(basis_points);
        donations.push(Donation {
            address,
            basis_points,
            purpose,
        });
    }
    if sum > ALL_BASIS_POINTS as u64 {
        return Err(ConfigError::DonationSum { height, sum });
    }
    Ok(donations)
}

/// A block reward: a non-negative safe integer, or a canonical decimal string that fits 64 bits.
fn reward_amount(value: &Value) -> Option<u64> {
    match value {
        Value::Number(_) => js::safe_integer(value).and_then(|amount| u64::try_from(amount).ok()),
        Value::String(text) => {
            let canonical = text == "0"
                || (text.bytes().next().is_some_and(|first| first != b'0')
                    && text.bytes().all(|byte| byte.is_ascii_digit()));
            if canonical { text.parse().ok() } else { None }
        }
        Value::Null | Value::Bool(_) | Value::Array(_) | Value::Object(_) => None,
    }
}

/// The rank table, when `dynamicReward` is an object.
fn dynamic_reward(
    milestone: &Map<String, Value>,
    height: u64,
    active_delegates: u64,
) -> Result<Option<DynamicReward>, ConfigError> {
    let Some(Value::Object(table)) = milestone.get("dynamicReward") else {
        return Ok(None);
    };
    let enabled = table.get("enabled").is_some_and(js::is_truthy);
    let ranks = match table.get("ranks") {
        Some(Value::Object(ranks)) => Some(ranks),
        _ => None,
    };
    if !enabled {
        // A disabled table is never read; keep what it holds that is usable.
        let ranks = ranks
            .map(|ranks| {
                js::property_order(ranks)
                    .into_iter()
                    .filter_map(|(key, value)| Some((key.clone(), reward_amount(value)?)))
                    .collect()
            })
            .unwrap_or_default();
        let secondary_reward = table
            .get("secondaryReward")
            .and_then(reward_amount)
            .unwrap_or(0);
        return Ok(Some(DynamicReward {
            enabled,
            ranks,
            secondary_reward,
        }));
    }

    let Some(ranks) = ranks else {
        return Err(invalid(
            height,
            "dynamicReward.ranks",
            "must be an object with a reward for every rank from 1 to activeDelegates",
        ));
    };
    // The reference's check: every active rank is paid. It stops at the first missing rank, so
    // it never runs past the size of the table.
    for rank in 1..=active_delegates {
        let key = format!("dynamicReward.ranks.{rank}");
        let value = ranks
            .get(&rank.to_string())
            .ok_or_else(|| missing(height, &key))?;
        if reward_amount(value).is_none() {
            return Err(invalid(height, key, REWARD_AMOUNT));
        }
    }
    let secondary_reward = match table.get("secondaryReward") {
        Some(value) => reward_amount(value)
            .ok_or_else(|| invalid(height, "dynamicReward.secondaryReward", REWARD_AMOUNT))?,
        None => return Err(missing(height, "dynamicReward.secondaryReward")),
    };
    let mut table_ranks = IndexMap::with_capacity(ranks.len());
    for (key, value) in js::property_order(ranks) {
        let reward = reward_amount(value)
            .ok_or_else(|| invalid(height, format!("dynamicReward.ranks.{key}"), REWARD_AMOUNT))?;
        table_ranks.insert(key.clone(), reward);
    }
    Ok(Some(DynamicReward {
        enabled,
        ranks: table_ranks,
        secondary_reward,
    }))
}

/// `block.version`, `block.maxTransactions` and `block.maxPayload`.
fn block(
    milestone: &Map<String, Value>,
    height: u64,
) -> Result<(u32, Option<u32>, Option<u32>), ConfigError> {
    let block = match milestone.get("block") {
        Some(Value::Object(block)) => block,
        Some(_) => return Err(invalid(height, "block", "must be an object")),
        None => return Err(missing(height, "block")),
    };
    let version = match block.get("version") {
        Some(value) => js::safe_integer(value)
            .and_then(|version| u32::try_from(version).ok())
            .ok_or_else(|| {
                invalid(
                    height,
                    "block.version",
                    "must be an integer from 0 to 4294967295",
                )
            })?,
        None => return Err(missing(height, "block.version")),
    };
    let limit = |key: &str| -> Result<Option<u32>, ConfigError> {
        block
            .get(key)
            .map(|value| {
                js::safe_integer(value)
                    .and_then(|limit| u32::try_from(limit).ok())
                    .filter(|limit| *limit >= 1)
                    .ok_or_else(|| {
                        invalid(
                            height,
                            format!("block.{key}"),
                            "must be an integer from 1 to 4294967295",
                        )
                    })
            })
            .transpose()
    };
    Ok((version, limit("maxTransactions")?, limit("maxPayload")?))
}

/// `transfer.minimum` and `transfer.maximum`, with the reference's defaults.
fn transfer(milestone: &Map<String, Value>, height: u64) -> Result<(u32, u32), ConfigError> {
    let transfer = match milestone.get("transfer") {
        Some(Value::Object(transfer)) => transfer,
        Some(_) => return Err(invalid(height, "transfer", "must be an object")),
        None => return Err(missing(height, "transfer")),
    };
    let limit = |key: &str, default: u32| -> Result<u32, ConfigError> {
        match transfer.get(key) {
            // `transfer.maximum || 256`: a falsy value means the default.
            Some(value) if js::is_truthy(value) => js::safe_integer(value)
                .and_then(|limit| u32::try_from(limit).ok())
                .filter(|limit| *limit >= 1)
                .ok_or_else(|| {
                    invalid(
                        height,
                        format!("transfer.{key}"),
                        "must be an integer from 1 to 4294967295",
                    )
                }),
            _ => Ok(default),
        }
    };
    Ok((
        limit("minimum", DEFAULT_TRANSFER_MINIMUM)?,
        limit("maximum", DEFAULT_TRANSFER_MAXIMUM)?,
    ))
}

/// `reward`: 0 when missing or `null`, as `reward ?? 0` gives.
fn reward(milestone: &Map<String, Value>, height: u64) -> Result<u64, ConfigError> {
    match milestone.get("reward") {
        None | Some(Value::Null) => Ok(0),
        Some(value) => reward_amount(value).ok_or_else(|| invalid(height, "reward", REWARD_AMOUNT)),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn network() -> Network {
        Network {
            name: "devnet".into(),
            message_prefix: "devnet message:\n".into(),
            address_character: Some("d".into()),
            bip32_public: 70_617_039,
            bip32_private: 70_615_956,
            pub_key_hash: 90,
            nethash: "00".repeat(32),
            wif: 252,
            slip44: 1,
            token: "dROOT".into(),
            symbol: "dRT".into(),
            explorer: String::new(),
        }
    }

    /// A first milestone with every key the loader needs, with `changes` applied (a `null`
    /// change removes the key).
    fn first(changes: Value) -> Value {
        let mut milestone = json!({
            "height": 1,
            "activeDelegates": 3,
            "blockTime": 8,
            "block": { "version": 0, "maxTransactions": 150, "maxPayload": 2_097_152 },
            "epoch": "2026-01-01T00:00:00.000Z",
            "reward": 0,
            "burn": { "feeBasisPoints": 9000, "txAmount": 2_000_000 },
            "donations": {},
            "dynamicReward": { "enabled": false },
            "transfer": { "maximum": 256, "minimum": 1 },
        });
        if let (Value::Object(target), Value::Object(changes)) = (&mut milestone, changes) {
            for (key, value) in changes {
                if value.is_null() {
                    target.remove(&key);
                } else {
                    target.insert(key, value);
                }
            }
        }
        milestone
    }

    fn load(milestones: Value) -> Result<Milestones, ConfigError> {
        Milestones::load(&milestones.to_string(), &network())
    }

    #[test]
    fn a_single_milestone() {
        let milestones = load(json!([first(json!({}))])).unwrap();
        let params = milestones.at(1);
        assert_eq!(params.height(), 1);
        assert_eq!(params.network(), 90);
        assert_eq!(params.block_time(), 8);
        assert_eq!(params.active_delegates(), 3);
        assert_eq!(params.block_version(), 0);
        assert_eq!(params.max_transactions(), Some(150));
        assert_eq!(params.max_payload(), Some(2_097_152));
        assert_eq!(
            (params.transfer_minimum(), params.transfer_maximum()),
            (1, 256)
        );
        assert_eq!(params.reward(), 0);
        assert_eq!(params.fee_basis_points(), 9000);
        assert_eq!(params.burn_tx_amount(), 2_000_000);
        assert!(params.donations().is_empty());
        assert_eq!(
            params.dynamic_reward().map(DynamicReward::enabled),
            Some(false)
        );
        assert_eq!(params.epoch(), "2026-01-01T00:00:00.000Z");
        assert!(std::ptr::eq(milestones.at(0), milestones.at(u32::MAX)));
    }

    #[test]
    fn lookup_by_height() {
        let milestones = load(json!([
            json!({ "height": 10, "reward": 5 }),
            first(json!({})),
            json!({ "height": 20, "reward": 7 }),
        ]))
        .unwrap();
        let rewards: Vec<(u32, u64)> = [0, 1, 9, 10, 19, 20, u32::MAX]
            .into_iter()
            .map(|height| (height, milestones.at(height).reward()))
            .collect();
        assert_eq!(
            rewards,
            [
                (0, 0),
                (1, 0),
                (9, 0),
                (10, 5),
                (19, 5),
                (20, 7),
                (u32::MAX, 7)
            ]
        );
        assert_eq!(milestones.all().len(), 3);
    }

    #[test]
    fn equal_heights_keep_file_order() {
        let milestones = load(json!([
            first(json!({})),
            json!({ "height": 5, "reward": 1 }),
            json!({ "height": 5, "reward": 2 }),
        ]))
        .unwrap();
        assert_eq!(milestones.at(5).reward(), 2);
        assert_eq!(milestones.all()[1].reward(), 1);
    }

    #[test]
    fn objects_merge_and_arrays_replace() {
        let milestones = load(json!([
            first(json!({ "p2p": { "minimumVersions": [">=4.3.0", ">=4.2.0"], "x": 1 } })),
            json!({ "height": 2, "p2p": { "minimumVersions": [">=4.4.0"] }, "block": { "maxTransactions": 2 } }),
        ]))
        .unwrap();
        let merged = milestones.at(2).as_json();
        assert_eq!(
            merged["p2p"],
            json!({ "minimumVersions": [">=4.4.0"], "x": 1 })
        );
        assert_eq!(
            merged["block"],
            json!({ "version": 0, "maxTransactions": 2, "maxPayload": 2_097_152 })
        );
        assert_eq!(milestones.at(2).max_transactions(), Some(2));
    }

    #[test]
    fn ranks_and_top_level_donations_are_replaced() {
        let milestones = load(json!([
            first(json!({
                "dynamicReward": { "enabled": true, "secondaryReward": 1, "ranks": { "1": 10, "2": 20, "3": 30, "4": 40 } },
                "other": { "ranks": { "1": 1, "2": 2 }, "donations": { "a": 1 } },
            })),
            json!({
                "height": 2,
                "dynamicReward": { "ranks": { "1": 11, "2": 21, "3": 31 } },
                "other": { "ranks": { "1": 5 }, "donations": { "b": 2 } },
            }),
        ]))
        .unwrap();
        let later = milestones.at(2);
        let table = later.dynamic_reward().unwrap();
        assert!(table.enabled());
        assert_eq!(table.secondary_reward(), 1);
        assert_eq!(
            (table.rank(1), table.rank(3), table.rank(4)),
            (Some(11), Some(31), None)
        );
        // Below the top level the same names merge.
        assert_eq!(
            later.as_json()["other"],
            json!({ "ranks": { "1": 5, "2": 2 }, "donations": { "a": 1, "b": 2 } })
        );
    }

    #[test]
    fn keys_are_enumerated_as_javascript_does() {
        let text = r#"[{"height":1,"activeDelegates":1,"blockTime":8,"block":{"version":0},
            "epoch":"2026-01-01T00:00:00.000Z","burn":{"feeBasisPoints":0,"txAmount":0},
            "transfer":{},"z":{"b":1,"10":2,"2":3}}]"#;
        let milestones = Milestones::load(text, &network()).unwrap();
        let keys: Vec<&str> = milestones.at(1).as_json()["z"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(keys, ["2", "10", "b"]);
    }

    #[test]
    fn reward_zero_and_dynamic_reward_null_apply_later() {
        let milestones = load(json!([
            first(json!({
                "reward": 5,
                "dynamicReward": { "enabled": true, "secondaryReward": 1, "ranks": { "1": 10, "2": 20, "3": 30 } },
            })),
            json!({ "height": 10, "dynamicReward": null }),
            json!({ "height": 20, "reward": 0 }),
        ]))
        .unwrap();
        assert!(milestones.at(9).dynamic_reward().unwrap().enabled());
        assert_eq!(milestones.at(10).dynamic_reward(), None);
        assert_eq!(milestones.at(10).reward(), 5);
        assert_eq!(milestones.at(20).reward(), 0);
    }

    #[test]
    fn transfer_limits_default_when_falsy() {
        let milestones = load(json!([first(
            json!({ "transfer": { "maximum": 0, "minimum": null } })
        )]))
        .unwrap();
        let params = milestones.at(1);
        assert_eq!(
            (params.transfer_minimum(), params.transfer_maximum()),
            (1, 256)
        );
    }

    #[test]
    fn reward_amounts() {
        assert_eq!(reward_amount(&json!(0)), Some(0));
        assert_eq!(reward_amount(&json!(-0.0)), Some(0));
        assert_eq!(reward_amount(&json!(1.8e8)), Some(180_000_000));
        assert_eq!(
            reward_amount(&json!("18446744073709551615")),
            Some(u64::MAX)
        );
        assert_eq!(reward_amount(&json!("0")), Some(0));
        for refused in [
            json!(-1),
            json!(1.5),
            json!(9_007_199_254_740_992_u64),
            json!("18446744073709551616"),
            json!("01"),
            json!(""),
            json!(" 1"),
            json!("1e3"),
            json!(null),
            json!(true),
            json!([]),
        ] {
            assert_eq!(reward_amount(&refused), None, "{refused}");
        }
    }

    #[test]
    fn prototype_names_are_refused_at_any_depth() {
        for (milestone, path) in [
            (first(json!({ "constructor": 1 })), "constructor"),
            (
                first(json!({ "fees": { "staticFees": { "toString": 1 } } })),
                "fees.staticFees.toString",
            ),
            (first(json!({ "p2p": [{ "valueOf": 1 }] })), "p2p.0.valueOf"),
        ] {
            assert_eq!(
                load(json!([milestone])),
                Err(ConfigError::ReservedKey {
                    index: 0,
                    path: path.to_owned()
                })
            );
        }
        let text = r#"[{"height":1,"__proto__":{"blockTime":8}}]"#;
        assert_eq!(
            Milestones::load(text, &network()),
            Err(ConfigError::ReservedKey {
                index: 0,
                path: "__proto__".to_owned()
            })
        );
        assert_eq!(
            load(
                json!([first(json!({})), { "height": 2, "dynamicReward": { "ranks": { "hasOwnProperty": 1 } } }])
            ),
            Err(ConfigError::ReservedKey {
                index: 1,
                path: "dynamicReward.ranks.hasOwnProperty".to_owned()
            })
        );
    }

    #[test]
    fn shape_and_height_refusals() {
        assert!(matches!(
            Milestones::load("{", &network()),
            Err(ConfigError::Json(_))
        ));
        assert!(matches!(load(json!({})), Err(ConfigError::Shape(_))));
        assert!(matches!(load(json!([])), Err(ConfigError::Shape(_))));
        assert!(matches!(load(json!([1])), Err(ConfigError::Shape(_))));
        for height in [
            json!(0),
            json!(-1),
            json!(1.5),
            json!("1"),
            json!(4_294_967_296_u64),
            json!(null),
        ] {
            assert_eq!(
                load(json!([first(json!({})), { "height": height }])),
                Err(ConfigError::Height { index: 1 }),
                "{height}"
            );
        }
        assert!(matches!(
            load(json!([first(json!({ "height": 2 }))])),
            Err(ConfigError::Invalid { height: 2, .. })
        ));
    }

    #[test]
    fn fixed_block_time_and_delegate_count() {
        assert!(
            load(
                json!([first(json!({})), { "height": 7, "blockTime": 8.0, "activeDelegates": 3e0 }])
            )
            .is_ok()
        );
        assert_eq!(
            load(json!([first(json!({})), { "height": 7, "blockTime": 6 }])),
            Err(ConfigError::Changed {
                height: 7,
                key: "blockTime",
                first_height: 1
            })
        );
        assert_eq!(
            load(json!([first(json!({})), { "height": 7, "activeDelegates": "3" }])),
            Err(ConfigError::Changed {
                height: 7,
                key: "activeDelegates",
                first_height: 1
            })
        );
        assert_eq!(
            load(json!([first(json!({ "blockTime": null }))])),
            Err(ConfigError::Missing {
                height: 1,
                key: "blockTime".to_owned()
            })
        );
        assert!(matches!(
            load(json!([first(
                json!({ "activeDelegates": 9_007_199_254_740_992_u64 })
            )])),
            Err(ConfigError::Invalid { height: 1, .. })
        ));
    }

    #[test]
    fn configuration_only_refusals() {
        for changes in [
            json!({ "epoch": "2026-01-01" }),
            json!({ "epoch": null }),
            json!({ "block": null }),
            json!({ "block": { "maxTransactions": 150 } }),
            json!({ "block": { "version": 0, "maxTransactions": 0 } }),
            json!({ "block": { "version": 0, "maxPayload": 1.5 } }),
            json!({ "block": { "version": -1 } }),
            json!({ "transfer": null }),
            json!({ "transfer": { "maximum": "2" } }),
            json!({ "reward": "1.5" }),
            json!({ "reward": true }),
            json!({ "dynamicReward": { "enabled": true, "secondaryReward": 0, "ranks": { "1": 1, "2": 1, "3": 1, "9": -1 } } }),
        ] {
            let result = load(json!([first(changes.clone())]));
            assert!(
                matches!(
                    result,
                    Err(ConfigError::Missing { height: 1, .. }
                        | ConfigError::Invalid { height: 1, .. })
                ),
                "{changes}: {result:?}"
            );
        }
        let absent_limits = load(json!([first(json!({ "block": { "version": 0 } }))])).unwrap();
        assert_eq!(absent_limits.at(1).max_transactions(), None);
        assert_eq!(absent_limits.at(1).max_payload(), None);
    }

    #[test]
    fn a_disabled_table_keeps_its_usable_entries() {
        let milestones = load(json!([first(json!({
            "dynamicReward": { "enabled": 0, "ranks": { "1": 5, "2": "x" }, "secondaryReward": -1 },
        }))]))
        .unwrap();
        let table = milestones.at(1).dynamic_reward().unwrap();
        assert!(!table.enabled());
        assert_eq!(
            (table.rank(1), table.rank(2), table.secondary_reward()),
            (Some(5), None, 0)
        );
    }

    #[test]
    fn an_enabled_table_needs_every_active_rank() {
        let ranks = |count: u32| -> Value {
            Value::Object(
                (1..=count)
                    .map(|rank| (rank.to_string(), json!(rank)))
                    .collect(),
            )
        };
        assert!(load(json!([first(json!({ "dynamicReward": { "enabled": true, "secondaryReward": 0, "ranks": ranks(3) } }))])).is_ok());
        assert_eq!(
            load(json!([first(
                json!({ "dynamicReward": { "enabled": "yes", "secondaryReward": 0, "ranks": ranks(2) } })
            )])),
            Err(ConfigError::Missing {
                height: 1,
                key: "dynamicReward.ranks.3".to_owned()
            })
        );
        assert!(matches!(
            load(json!([first(
                json!({ "dynamicReward": { "enabled": true, "ranks": ranks(3) } })
            )])),
            Err(ConfigError::Missing { height: 1, .. })
        ));
        assert!(matches!(
            load(json!([first(
                json!({ "dynamicReward": { "enabled": true, "secondaryReward": 0, "ranks": [] } })
            )])),
            Err(ConfigError::Invalid { height: 1, .. })
        ));
    }

    #[test]
    fn burn_refusals() {
        for (burn, expected) in [
            (json!(null), "burn"),
            (json!({ "feeBasisPoints": 9000 }), "burn.txAmount"),
            (json!({ "txAmount": 0 }), "burn.feeBasisPoints"),
        ] {
            let milestone = if burn.is_null() {
                first(json!({ "burn": null }))
            } else {
                first(json!({ "burn": burn }))
            };
            assert_eq!(
                load(json!([milestone])),
                Err(ConfigError::Missing {
                    height: 1,
                    key: expected.to_owned()
                })
            );
        }
        assert_eq!(
            load(json!([first(
                json!({ "burn": { "feeBasisPoints": 9000, "txAmount": 0, "feePercent": 90 } })
            )])),
            Err(ConfigError::Unsupported {
                height: 1,
                key: "burn.feePercent".to_owned()
            })
        );
        let zero = load(json!([first(
            json!({ "burn": { "feeBasisPoints": -0.0, "txAmount": -0.0 } })
        )]))
        .unwrap();
        assert_eq!(
            (zero.at(1).fee_basis_points(), zero.at(1).burn_tx_amount()),
            (0, 0)
        );
    }
}
