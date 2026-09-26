//! The chain-definition builder for devnets (feature `genesis`).
//!
//! [`build`] makes, from its inputs alone, the files of a devnet chain definition: `network.json`,
//! `milestones.json`, `genesisBlock.json` and `exceptions.json`, byte for byte as the reference's
//! devnet generator writes them, plus the derived key material and a summary. It does no I/O and
//! reads no clock: the seed, the epoch, the rules, the roles and the allocations are all inputs.
//!
//! The genesis block holds, in order: one transfer (1/6) from the generator key, nonce 1, fee 0,
//! with one item per allocation (the issuance record); a delegate registration (1/2) for each
//! delegate, nonce 1; and a vote (2/2) for each delegate, 100 % for itself, nonce 2. Every
//! transaction and the block are signed with BIP340 and a fixed aux derived from the seed
//! ([`derive::aux`]). The network hash is the block's payload hash.
//!
//! JSON is written as `JSON.stringify(value, null, 4)` followed by a newline writes it (see
//! [`crate::utils::js::stringify`]).

#![expect(
    unused_variables,
    reason = "the bodies are not written yet; remove this once they are"
)]

use std::fmt;

use serde_json::{Map, Value};
use zeroize::Zeroizing;

use crate::blocks::BlockId;
use crate::errors::GenesisError;
use crate::identities::{Address, KeyPair};
use crate::managers::Network;

pub mod bip39;
pub mod derive;

/// Everything a chain definition is built from.
#[derive(Debug, Clone, PartialEq)]
pub struct GenesisInput {
    /// The seed text every key and aux is derived from.
    pub seed: String,
    /// The derivation tag, the first part of every derivation input.
    pub derivation_tag: String,
    /// The network epoch, as ISO 8601 text; written into the first milestone.
    pub epoch: String,
    /// The network description. Its `nethash` is replaced by the genesis payload hash.
    pub network: Network,
    /// The first milestone's keys in file order. The builder sets `height`, `epoch` and an empty
    /// `donations`.
    pub milestone: Map<String, Value>,
    /// The role of the key that signs the genesis block and the issuance record.
    pub generator_role: String,
    /// The genesis allocations, in the order of the issuance record's items.
    pub allocations: Vec<Allocation>,
    /// The genesis delegates, in registration order.
    pub delegates: Vec<GenesisDelegate>,
    /// The donation recipients, in file order.
    pub donations: Vec<DonationShare>,
    /// The height of the milestone that sets the donations.
    pub donations_from_height: u32,
    /// Roles of extra wallets whose key material is derived but not funded at genesis.
    pub wallet_roles: Vec<String>,
}

/// An amount issued at genesis to the key of a role.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Allocation {
    /// The role whose key receives the amount.
    pub role: String,
    /// Amount in base units.
    pub amount: u64,
}

/// A delegate registered at genesis.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenesisDelegate {
    /// The role its key is derived from.
    pub role: String,
    /// The delegate name.
    pub username: String,
}

/// A donation recipient whose address is derived from a role.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DonationShare {
    /// The role its key is derived from.
    pub role: String,
    /// The share of the block reward, in basis points.
    pub basis_points: u16,
    /// The purpose written next to the share, if any.
    pub purpose: Option<String>,
}

/// Key material derived for a role.
pub struct DerivedKey {
    /// The role.
    pub role: String,
    /// The 12-word passphrase. It is wiped when it is dropped.
    pub passphrase: Zeroizing<String>,
    /// The passphrase's key pair.
    pub keys: KeyPair,
    /// The key's address on the devnet.
    pub address: Address,
}

impl fmt::Debug for DerivedKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DerivedKey")
            .field("role", &self.role)
            .field("address", &self.address)
            .finish_non_exhaustive()
    }
}

/// The figures a caller checks a chain definition by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GenesisSummary {
    /// The issuance counter: the sum of the issuance record's items.
    pub issued: u64,
    /// The network hash (the genesis payload hash).
    pub nethash: [u8; 32],
    /// The genesis block id.
    pub genesis_id: BlockId,
    /// The number of genesis transactions.
    pub transactions: u32,
}

/// A built chain definition.
#[derive(Debug)]
pub struct ChainDefinition {
    /// `network.json`.
    pub network_json: String,
    /// `milestones.json`.
    pub milestones_json: String,
    /// `genesisBlock.json`.
    pub genesis_block_json: String,
    /// `exceptions.json`.
    pub exceptions_json: String,
    /// The generator key.
    pub generator: DerivedKey,
    /// The allocation keys, in input order.
    pub allocations: Vec<DerivedKey>,
    /// The delegate keys, in input order.
    pub delegates: Vec<DerivedKey>,
    /// The donation recipient keys, in input order.
    pub donations: Vec<DerivedKey>,
    /// The extra wallet keys, in input order.
    pub wallets: Vec<DerivedKey>,
    /// The summary.
    pub summary: GenesisSummary,
}

/// Build the chain definition of `input`.
pub fn build(input: &GenesisInput) -> Result<ChainDefinition, GenesisError> {
    todo!()
}
