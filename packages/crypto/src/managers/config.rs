//! The milestones, `milestones.json`: loading, merging and the start-up checks.
//!
//! Loading sorts the milestones by height and merges each one onto the merged milestone before it:
//! objects merge key by key and arrays are replaced, except `dynamicReward.ranks` and `donations`,
//! which a milestone that states them replaces whole. Every merged milestone then passes the
//! start-up checks, and a refusal names the height and the key:
//!
//! - `blockTime` and `activeDelegates`: stated by the first milestone, each a JSON number whose
//!   double value is an integer from 1 to `2^53 − 1`, and unchanged at every later milestone
//!   (compared as doubles);
//! - `burn.feeBasisPoints`: an integer from 0 to 10,000; `burn.feePercent` is refused;
//! - `burn.txAmount`: a JSON number whose double value is an integer from 0 to `2^53 − 1`;
//! - `donations`: an object of addresses of this network, each with `basisPoints` (an integer from
//!   1 to 10,000) and an optional string `purpose`, nothing else, adding up to at most 10,000;
//! - with `dynamicReward.enabled`, a reward for every rank from 1 to `activeDelegates`, and a
//!   `secondaryReward`, each a non-negative safe integer or a decimal string that fits 64 bits.
//!
//! JSON numbers are read as `JSON.parse` reads them (see [`crate::utils::js`]), so `8.0` and `8e0`
//! are 8, and `9007199254740993` is `2^53`.

#![expect(
    unused_variables,
    reason = "the bodies are not written yet; remove this once they are"
)]

use indexmap::IndexMap;
use serde_json::{Map, Value};

use crate::errors::ConfigError;
use crate::identities::Address;
use crate::managers::Network;

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

    /// The secondary reward, `dynamicReward.secondaryReward`.
    pub fn secondary_reward(&self) -> u64 {
        self.secondary_reward
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

    /// The network epoch, as the file states it.
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

    /// The merged milestone as JSON, keys in merge order.
    pub fn as_json(&self) -> &Map<String, Value> {
        &self.json
    }
}

/// The merged milestones of a network, in height order.
#[derive(Debug, Clone, PartialEq)]
pub struct Milestones {
    merged: Vec<Params>,
}

impl Milestones {
    /// Read `milestones.json` for `network`: sort, merge, and run every start-up check.
    pub fn load(json: &str, network: &Network) -> Result<Milestones, ConfigError> {
        todo!()
    }

    /// The merged milestone in force at `height`: the last one that starts at or below it (the
    /// first one for heights below the first milestone).
    pub fn at(&self, height: u32) -> &Params {
        todo!()
    }

    /// Every merged milestone, in height order.
    pub fn all(&self) -> &[Params] {
        &self.merged
    }
}
