//! The chain-definition builder for devnets (feature `genesis`).
//!
//! [`build`] makes, from its inputs alone, the files of a devnet chain definition: `network.json`,
//! `milestones.json`, `genesisBlock.json` and `exceptions.json`, byte for byte as the reference's
//! devnet generator writes them, plus the derived key material and a summary. It does no I/O and
//! reads no clock: the seed, the epoch, the rules, the roles and the allocations are all inputs.
//!
//! Every key is derived from the seed and a role name ([`mod@derive`]). The genesis block holds, in
//! order:
//!
//! 1. the issuance record: transfers (1/6) from the generator key, fee 0, with one item per
//!    allocation, as many items per transfer as the milestone's `transfer.maximum` allows, nonces
//!    counting from 1;
//! 2. a delegate registration (1/2) for each delegate, nonce 1;
//! 3. a vote (2/2) for each delegate, 100 % for itself, nonce 2.
//!
//! Every transaction and the block are signed with BIP340 and a fixed aux derived from the seed
//! ([`derive::aux`]): the label of a transaction's aux is its 0-based index in the block, and the
//! block's is `block`. The block is version `block.version` of the first milestone, height 1,
//! timestamp 0, with 32 bytes of payload per transaction; its payload hash, the SHA-256 of the
//! concatenated transaction ids, is also the network hash.
//!
//! The transactions are made as the generator makes them, signed and serialised directly, and each
//! is then decoded under the transaction rules for a genesis block ([`validation::Mode::GENESIS`],
//! which allows the zero fee, as the reference does for its genesis block's transactions), with
//! its signature verified. The builder also refuses early, with a clearer error, what those rules
//! and the later steps would refuse: invalid delegate names, allocations of 0, and transfers
//! outside the milestone's `transfer.minimum` and `transfer.maximum`.
//!
//! `milestones.json` holds the first milestone, built from the input rules, and, when there are
//! donations, a second milestone that sets them. Before the block is built the milestones pass the
//! same start-up checks as a node's ([`Milestones::load`]), and the finished block passes the block
//! rules ([`Block::verify`]).
//!
//! JSON is written as `JSON.stringify(value, null, 4)` followed by a newline writes it (see
//! [`crate::utils::js::stringify`]), which also puts the keys of every object in the order a
//! JavaScript object enumerates them. The genesis block and its transactions have their keys in
//! alphabetical order, as the reference's genesis files do.

use std::collections::HashSet;
use std::fmt;

use serde_json::{Map, Value, json};
use zeroize::Zeroizing;

use crate::blocks::serialiser as serialiser_block;
use crate::blocks::{Block, BlockHeader, BlockId, VerifyFailure};
use crate::crypto::hash::sha256_concat;
use crate::crypto::sig::{self, Aux, SchemeId, Signature, SigningDomain};
use crate::errors::{BlockError, BuildError, GenesisError};
use crate::identities::{Address, KeyPair};
use crate::managers::{Milestones, Network, Params};
use crate::transactions::serialiser::{self, SerialiseOptions};
use crate::transactions::types::{TransferItem, Votes};
use crate::transactions::{Asset, Transaction, TransactionData, TransactionType, signer};
use crate::utils::{hex, js};
use crate::validation;

pub mod bip39;
pub mod derive;

/// Everything a chain definition is built from.
#[derive(Clone, PartialEq)]
pub struct GenesisInput {
    /// The seed text every key and aux is derived from. `Debug` does not show it.
    pub seed: String,
    /// The derivation tag, the first part of every derivation input.
    pub derivation_tag: String,
    /// The network epoch, a whole second as `Date.prototype.toISOString` writes it, for example
    /// `2026-01-01T00:00:00.000Z`. It is written into the first milestone.
    pub epoch: String,
    /// The network description. The builder sets its `addressCharacter` from the network byte and
    /// its `nethash` to the genesis payload hash; the values given for them are ignored.
    pub network: Network,
    /// The rules the first milestone starts from, keys in file order (for example, a network's
    /// merged milestone at some height). A `height` key is ignored.
    pub rules: Map<String, Value>,
    /// Values set on the first milestone after the epoch and the empty donations, in order.
    pub overrides: Vec<RuleOverride>,
    /// The role of the key that signs the genesis block and the issuance record.
    pub generator_role: String,
    /// The genesis allocations, in the order of the issuance record's items.
    pub allocations: Vec<Allocation>,
    /// The genesis delegates, in registration order.
    pub delegates: Vec<GenesisDelegate>,
    /// The donation recipients, in file order.
    pub donations: Vec<DonationShare>,
    /// The height of the milestone that sets the donations: 2 or more.
    pub donations_from_height: u32,
    /// Roles of extra wallets whose key material is derived but not funded at genesis.
    pub wallet_roles: Vec<String>,
}

impl fmt::Debug for GenesisInput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GenesisInput")
            .field("derivation_tag", &self.derivation_tag)
            .field("epoch", &self.epoch)
            .field("network", &self.network)
            .field("rules", &self.rules)
            .field("overrides", &self.overrides)
            .field("generator_role", &self.generator_role)
            .field("allocations", &self.allocations)
            .field("delegates", &self.delegates)
            .field("donations", &self.donations)
            .field("donations_from_height", &self.donations_from_height)
            .field("wallet_roles", &self.wallet_roles)
            .finish_non_exhaustive()
    }
}

/// A value set at a dotted path of the first milestone, such as `dynamicFees.minFee`.
///
/// The path is split at every `.`. An object on the way that is missing, or is not an object, is
/// replaced by an empty object; the last key then takes the value, in place if it exists.
#[derive(Debug, Clone, PartialEq)]
pub struct RuleOverride {
    /// The dotted path.
    pub path: String,
    /// The value.
    pub value: Value,
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
    /// The genesis block that `genesis_block_json` describes.
    pub block: Block,
}

/// A vote for one delegate with all of the voter's weight: 10,000 basis points, 100 %.
const WHOLE_VOTE: u16 = 10_000;

/// The version of every genesis transaction.
const TRANSACTION_VERSION: u8 = 3;

/// Build the chain definition of `input`.
pub fn build(input: &GenesisInput) -> Result<ChainDefinition, GenesisError> {
    let allocated = check_input(input)?;
    let network_byte = input.network.pub_key_hash;
    let key =
        |role: &String| derive::role_key(&input.derivation_tag, role, &input.seed, network_byte);
    let generator = key(&input.generator_role)?;
    let allocations = derive_all(input.allocations.iter().map(|a| &a.role), key)?;
    let delegates = derive_all(input.delegates.iter().map(|d| &d.role), key)?;
    let donations = derive_all(input.donations.iter().map(|d| &d.role), key)?;
    let wallets = derive_all(input.wallet_roles.iter(), key)?;
    let aux = |label: &str| Aux::derived(derive::aux(&input.derivation_tag, label, &input.seed));

    let mut network = input.network.clone();
    network.address_character = Some(address_character(network_byte)?.to_string());
    network.nethash = String::new();

    let milestones_json = write_json(&milestones(input, &donations));
    let milestones = Milestones::load(&milestones_json, &network)?;
    let params = milestones.at(1);

    // The transactions in block order, each with its asset as the genesis file writes it.
    let mut transactions: Vec<(Transaction, Value)> = Vec::new();
    let mut add = |keys: &KeyPair, nonce: u64, asset: Asset, json: Value| {
        let label = transactions.len().to_string();
        let tx = genesis_transaction(keys, nonce, asset, aux(&label), params)?;
        transactions.push((tx, json));
        Ok::<(), GenesisError>(())
    };
    let items: Vec<TransferItem> = input
        .allocations
        .iter()
        .zip(&allocations)
        .map(|(allocation, recipient)| TransferItem {
            amount: allocation.amount,
            recipient: recipient.address,
        })
        .collect();
    for (nonce, chunk) in (1..).zip(transfer_chunks(&items, params)?) {
        let json = json!({
            "transfers": chunk
                .iter()
                .map(|item| json!({
                    "amount": item.amount.to_string(),
                    "recipientId": item.recipient.to_base58(),
                }))
                .collect::<Vec<_>>(),
        });
        let asset = Asset::Transfer {
            transfers: chunk.to_vec(),
        };
        add(&generator.keys, nonce, asset, json)?;
    }
    for (delegate, keys) in input.delegates.iter().zip(&delegates) {
        validation::validate_username(&delegate.username).map_err(BuildError::from)?;
        let json = json!({ "delegate": { "username": delegate.username } });
        let asset = Asset::DelegateRegistration {
            username: delegate.username.clone(),
        };
        add(&keys.keys, 1, asset, json)?;
    }
    for (delegate, keys) in input.delegates.iter().zip(&delegates) {
        let json = json!({ "votes": { delegate.username.as_str(): WHOLE_VOTE / 100 } });
        let votes: Votes = [(delegate.username.clone(), WHOLE_VOTE)]
            .into_iter()
            .collect();
        add(&keys.keys, 2, Asset::Vote { votes }, json)?;
    }

    let (transactions, assets): (Vec<Transaction>, Vec<Value>) = transactions.into_iter().unzip();
    let block = genesis_block(transactions, &generator.keys, aux("block"), params)?;
    let verification = block.verify(&milestones, 0);
    if !verification.is_verified() {
        return Err(GenesisError::Verification(verification.failures));
    }
    let issued = issuance(&block)?;
    if issued != allocated {
        return Err(GenesisError::Issuance { issued, allocated });
    }

    let header = block.header();
    network.nethash = hex::encode(&header.payload_hash);
    Ok(ChainDefinition {
        network_json: write_json(&network.to_json()),
        milestones_json,
        genesis_block_json: write_json(&block_json(&block, assets)),
        exceptions_json: write_json(&Value::Object(Map::new())),
        summary: GenesisSummary {
            issued,
            nethash: header.payload_hash,
            genesis_id: *block.id(),
            transactions: header.number_of_transactions,
        },
        generator,
        allocations,
        delegates,
        donations,
        wallets,
        block,
    })
}

/// The allocations split into transfers of at most `transfer.maximum` items. Every transfer must
/// have at least `transfer.minimum` items.
fn transfer_chunks<'a>(
    items: &'a [TransferItem],
    params: &Params,
) -> Result<std::slice::Chunks<'a, TransferItem>, GenesisError> {
    let maximum = usize::try_from(params.transfer_maximum())
        .ok()
        .filter(|&n| n > 0)
        .ok_or_else(|| GenesisError::Input("transfer.maximum is 0".into()))?;
    let minimum = usize::try_from(params.transfer_minimum()).unwrap_or(usize::MAX);
    let chunks = items.chunks(maximum);
    if chunks.clone().any(|chunk| chunk.len() < minimum) {
        return Err(GenesisError::Input(format!(
            "the allocations do not split into transfers of {minimum} to {maximum} items"
        )));
    }
    Ok(chunks)
}

/// A genesis transaction, made as the reference's devnet generator makes it: version 3, fee 0,
/// signed over its unsigned bytes. It is then decoded under the rules for genesis transactions,
/// which allow the zero fee, and its signature is checked before it is used.
fn genesis_transaction(
    keys: &KeyPair,
    nonce: u64,
    asset: Asset,
    aux: Aux,
    params: &Params,
) -> Result<Transaction, GenesisError> {
    let kind = match &asset {
        Asset::Transfer { .. } => TransactionType::Transfer,
        Asset::SecondSignature { .. } => TransactionType::SecondSignature,
        Asset::DelegateRegistration { .. } => TransactionType::DelegateRegistration,
        Asset::DelegateResignation { .. } => TransactionType::DelegateResignation,
        Asset::Burn { .. } => TransactionType::Burn,
        Asset::Vote { .. } => TransactionType::Vote,
    };
    let mut data = TransactionData {
        version: TRANSACTION_VERSION,
        network: params.network(),
        kind,
        nonce,
        sender_public_key: keys.public_key().clone(),
        fee: 0,
        memo: None,
        asset,
        signature: None,
        second_signature: None,
    };
    signer::sign(&mut data, keys, aux, params)?;
    let bytes =
        serialiser::serialise(&data, SerialiseOptions::FULL, params).map_err(BuildError::from)?;
    let tx = Transaction::decode_with(&bytes, params, validation::Mode::GENESIS)
        .map_err(BuildError::from)?;
    if !tx.is_verified() {
        return Err(GenesisError::Verification(vec![
            VerifyFailure::UnverifiedTransactions(vec![*tx.id()]),
        ]));
    }
    Ok(tx)
}

/// The genesis block of `transactions`: version `block.version`, height 1, timestamp 0, no
/// previous block, no reward, 32 bytes of payload per transaction, the header signed with BIP340
/// by `keys`, as the reference's devnet generator makes it.
fn genesis_block(
    transactions: Vec<Transaction>,
    keys: &KeyPair,
    aux: Aux,
    params: &Params,
) -> Result<Block, GenesisError> {
    let too_many = || GenesisError::Input("too many genesis transactions".into());
    let count = u32::try_from(transactions.len()).map_err(|_| too_many())?;
    let mut header = BlockHeader {
        version: params.block_version(),
        timestamp: 0,
        height: 1,
        previous_block: [0; 32],
        number_of_transactions: count,
        total_amount: checked_sum(transactions.iter().map(Transaction::amount))?,
        total_fee: checked_sum(transactions.iter().map(Transaction::fee))?,
        reward: 0,
        payload_length: count.checked_mul(32).ok_or_else(too_many)?,
        payload_hash: sha256_concat(transactions.iter().map(|tx| tx.id().as_bytes().as_slice())),
        generator_public_key: keys.public_key().to_compressed(),
        block_signature: Signature::from_array([0; 64]),
    };
    header.block_signature = sig::sign(
        SchemeId::Secp256k1Bip340,
        SigningDomain::Block,
        &serialiser_block::signing_hash(&header),
        keys.secret_key(),
        aux,
    )
    .map_err(BlockError::from)?;
    Ok(Block {
        id: serialiser_block::block_id(&header),
        header,
        transactions,
    })
}

/// Check the inputs that the later steps do not check, and return the sum of the allocations.
fn check_input(input: &GenesisInput) -> Result<u64, GenesisError> {
    let refuse = |message: &str| Err(GenesisError::Input(message.to_owned()));
    if input.seed.is_empty() {
        return refuse("the seed is empty");
    }
    check_epoch(&input.epoch)?;
    let mut roles = std::iter::once(&input.generator_role)
        .chain(input.allocations.iter().map(|a| &a.role))
        .chain(input.delegates.iter().map(|d| &d.role))
        .chain(input.donations.iter().map(|d| &d.role))
        .chain(&input.wallet_roles);
    if roles.any(|role| role.is_empty()) {
        return refuse("a role name is empty");
    }
    if input.delegates.is_empty() {
        return refuse("there are no delegates");
    }
    // A transfer item of the reference is at least 1 base unit, in the genesis block too.
    if input.allocations.iter().any(|a| a.amount == 0) {
        return refuse("an allocation is 0");
    }
    // Every signing key must be a different key: each sender's nonces start at 1.
    let mut signers = HashSet::from([input.generator_role.as_str()]);
    let mut usernames = HashSet::new();
    for delegate in &input.delegates {
        if !signers.insert(delegate.role.as_str()) {
            return Err(GenesisError::Input(format!(
                "role {:?} signs more than once",
                delegate.role
            )));
        }
        if !usernames.insert(delegate.username.as_str()) {
            return Err(GenesisError::Input(format!(
                "delegate name {:?} is registered twice",
                delegate.username
            )));
        }
    }
    // The reference cannot build its state with donations at height 1, whose block has no
    // generating delegate; the genesis reward is 0 anyway.
    if !input.donations.is_empty() && input.donations_from_height < 2 {
        return refuse("donations can start at height 2 at the earliest");
    }
    checked_sum(input.allocations.iter().map(|a| a.amount))
}

/// Refuse an epoch that is not a whole second in `Date.prototype.toISOString` form.
fn check_epoch(epoch: &str) -> Result<(), GenesisError> {
    let bad = || GenesisError::Epoch(epoch.to_owned());
    let bytes = epoch.as_bytes();
    let shape = bytes.len() == 24
        && bytes.iter().enumerate().all(|(i, &c)| match i {
            4 | 7 => c == b'-',
            10 => c == b'T',
            13 | 16 => c == b':',
            19 => c == b'.',
            23 => c == b'Z',
            _ => c.is_ascii_digit(),
        });
    if !shape || !epoch.ends_with(".000Z") {
        return Err(bad());
    }
    let field = |range: std::ops::Range<usize>| {
        epoch
            .get(range)
            .and_then(|digits| digits.parse::<u32>().ok())
            .ok_or_else(bad)
    };
    let (year, month, day) = (field(0..4)?, field(5..7)?, field(8..10)?);
    let (hour, minute, second) = (field(11..13)?, field(14..16)?, field(17..19)?);
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return Err(bad()),
    };
    if (1..=days).contains(&day) && hour < 24 && minute < 60 && second < 60 {
        Ok(())
    } else {
        Err(bad())
    }
}

fn derive_all<'a>(
    roles: impl Iterator<Item = &'a String>,
    key: impl Fn(&String) -> Result<DerivedKey, GenesisError>,
) -> Result<Vec<DerivedKey>, GenesisError> {
    roles.map(key).collect()
}

fn checked_sum(values: impl IntoIterator<Item = u64>) -> Result<u64, GenesisError> {
    values
        .into_iter()
        .try_fold(0u64, u64::checked_add)
        .ok_or_else(|| GenesisError::Input("an amount adds up to more than 2^64 - 1".into()))
}

/// The first character of the network's addresses, which must not depend on the key hash.
fn address_character(network: u8) -> Result<char, GenesisError> {
    let first = |fill: u8| {
        let bytes = std::array::from_fn(|i| if i == 0 { network } else { fill });
        Address::from_bytes(bytes).to_base58().chars().next()
    };
    match (first(0x00), first(0xff)) {
        (Some(low), Some(high)) if low == high => Ok(low),
        (low, high) => Err(GenesisError::AddressCharacter {
            network,
            low: low.unwrap_or_default(),
            high: high.unwrap_or_default(),
        }),
    }
}

/// `milestones.json`: the first milestone and, with donations, the milestone that sets them.
fn milestones(input: &GenesisInput, donations: &[DerivedKey]) -> Value {
    let mut first = Map::new();
    first.insert("height".into(), Value::from(1));
    for (key, value) in &input.rules {
        if key != "height" {
            first.insert(key.clone(), value.clone());
        }
    }
    first.insert("epoch".into(), Value::from(input.epoch.as_str()));
    first.insert("donations".into(), Value::Object(Map::new()));
    for rule in &input.overrides {
        set_leaf(&mut first, &rule.path, rule.value.clone());
    }
    let mut list = vec![Value::Object(first)];
    if !input.donations.is_empty() {
        let mut shares = Map::new();
        for (share, recipient) in input.donations.iter().zip(donations) {
            let mut entry = Map::new();
            entry.insert("basisPoints".into(), Value::from(share.basis_points));
            if let Some(purpose) = &share.purpose {
                entry.insert("purpose".into(), Value::from(purpose.as_str()));
            }
            shares.insert(recipient.address.to_base58(), Value::Object(entry));
        }
        list.push(json!({ "height": input.donations_from_height, "donations": shares }));
    }
    Value::Array(list)
}

/// Set `value` at the dotted `path` below `object` (see [`RuleOverride`]).
fn set_leaf(object: &mut Map<String, Value>, path: &str, value: Value) {
    match path.split_once('.') {
        None => {
            object.insert(path.to_owned(), value);
        }
        Some((key, rest)) => {
            let child = object.entry(key).or_insert(Value::Null);
            if !child.is_object() {
                *child = Value::Object(Map::new());
            }
            if let Value::Object(child) = child {
                set_leaf(child, rest, value);
            }
        }
    }
}

/// The issuance counter: the items of every transfer in `block` signed by the block's generator.
fn issuance(block: &Block) -> Result<u64, GenesisError> {
    let generator = block.header().generator_public_key.as_bytes().as_slice();
    let items = block.transactions().iter().flat_map(|tx| {
        let data = tx.data();
        let transfers = match &data.asset {
            Asset::Transfer { transfers }
                if data.kind == TransactionType::Transfer
                    && data.sender_public_key.as_bytes() == generator =>
            {
                transfers.as_slice()
            }
            _ => &[],
        };
        transfers.iter().map(|item| item.amount)
    });
    checked_sum(items)
}

/// `genesisBlock.json`: the block and its transactions, keys in alphabetical order.
fn block_json(block: &Block, assets: Vec<Value>) -> Value {
    let header = block.header();
    let transactions: Vec<Value> = block
        .transactions()
        .iter()
        .zip(assets)
        .map(|(tx, asset)| transaction_json(tx, asset))
        .collect();
    json!({
        "blockSignature": header.block_signature.to_hex(),
        "generatorPublicKey": header.generator_public_key.to_hex(),
        "height": header.height,
        "id": block.id().to_hex(),
        "numberOfTransactions": header.number_of_transactions,
        "payloadHash": hex::encode(&header.payload_hash),
        "payloadLength": header.payload_length,
        "previousBlock": hex::encode(&header.previous_block),
        "reward": header.reward.to_string(),
        "timestamp": header.timestamp,
        "totalAmount": header.total_amount.to_string(),
        "totalFee": header.total_fee.to_string(),
        "transactions": transactions,
        "version": header.version,
    })
}

/// A genesis transaction as the genesis file writes it, keys in alphabetical order.
fn transaction_json(tx: &Transaction, asset: Value) -> Value {
    let data = tx.data();
    let mut json = Map::new();
    json.insert("asset".into(), asset);
    json.insert("fee".into(), Value::from(data.fee.to_string()));
    json.insert("id".into(), Value::from(tx.id().to_hex()));
    json.insert("network".into(), Value::from(data.network));
    json.insert("nonce".into(), Value::from(data.nonce.to_string()));
    json.insert(
        "senderPublicKey".into(),
        Value::from(data.sender_public_key.to_hex()),
    );
    if let Some(signature) = &data.signature {
        json.insert("signature".into(), Value::from(signature.to_hex()));
    }
    json.insert("type".into(), Value::from(data.kind.type_id()));
    json.insert("typeGroup".into(), Value::from(data.kind.type_group()));
    json.insert("version".into(), Value::from(data.version));
    Value::Object(json)
}

/// `value` as `JSON.stringify(value, null, 4) + "\n"` writes the JavaScript object it describes.
fn write_json(value: &Value) -> String {
    let mut text = js::stringify(value, 4);
    text.push('\n');
    text
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn object(value: Value) -> Map<String, Value> {
        match value {
            Value::Object(map) => map,
            other => panic!("not an object: {other}"),
        }
    }

    #[test]
    fn set_leaf_follows_the_javascript_rule() {
        let mut m = object(json!({ "a": { "b": 1, "c": 2 }, "d": [1], "e": null, "f": 3 }));
        set_leaf(&mut m, "a.b", json!(10));
        set_leaf(&mut m, "a.x", json!(11));
        set_leaf(&mut m, "d.y", json!(12));
        set_leaf(&mut m, "e.z", json!(13));
        set_leaf(&mut m, "f", json!({ "g": 14 }));
        set_leaf(&mut m, "n.o.p", json!(15));
        assert_eq!(
            serde_json::to_string(&m).unwrap(),
            r#"{"a":{"b":10,"c":2,"x":11},"d":{"y":12},"e":{"z":13},"f":{"g":14},"n":{"o":{"p":15}}}"#
        );
    }

    #[test]
    fn epochs_must_be_whole_seconds_in_iso_form() {
        for good in [
            "2026-01-01T00:00:00.000Z",
            "2024-02-29T23:59:59.000Z",
            "2000-02-29T12:00:00.000Z",
            "0000-01-01T00:00:00.000Z",
        ] {
            assert_eq!(check_epoch(good), Ok(()), "{good}");
        }
        for bad in [
            "",
            "2026-01-01T00:00:00Z",
            "2026-01-01T00:00:00.001Z",
            "2026-01-01 00:00:00.000Z",
            "2026-01-01T00:00:00.000+00:00",
            "2026-13-01T00:00:00.000Z",
            "2026-00-01T00:00:00.000Z",
            "2026-02-29T00:00:00.000Z",
            "1900-02-29T00:00:00.000Z",
            "2026-04-31T00:00:00.000Z",
            "2026-01-00T00:00:00.000Z",
            "2026-01-01T24:00:00.000Z",
            "2026-01-01T00:60:00.000Z",
            "2026-01-01T00:00:60.000Z",
            "+026-01-01T00:00:00.000Z",
            "2026-01-01T00:00:00.000\u{17d}",
        ] {
            assert_eq!(
                check_epoch(bad),
                Err(GenesisError::Epoch(bad.into())),
                "{bad}"
            );
        }
    }

    fn key() -> impl Strategy<Value = String> {
        prop::sample::select(vec!["a", "b", "0", "1", "10", "01", "x.y"]).prop_map(str::to_owned)
    }

    fn tree() -> impl Strategy<Value = Value> {
        let leaf = prop_oneof![Just(Value::Null), any::<u32>().prop_map(Value::from)];
        leaf.prop_recursive(3, 16, 4, |inner| {
            prop_oneof![
                prop::collection::vec(inner.clone(), 0..3).prop_map(Value::Array),
                prop::collection::vec((key(), inner), 0..4)
                    .prop_map(|entries| Value::Object(entries.into_iter().collect())),
            ]
        })
    }

    proptest! {
        #[test]
        fn set_leaf_sets_the_value_and_keeps_the_other_keys_in_place(
            root in prop::collection::vec((key(), tree()), 0..5),
            path in prop::collection::vec(prop::sample::select(vec!["a", "b", "1"]), 1..4),
            value in any::<u32>(),
        ) {
            let mut map: Map<String, Value> = root.into_iter().collect();
            let before: Vec<String> = map.keys().cloned().collect();
            set_leaf(&mut map, &path.join("."), Value::from(value));
            let whole = Value::Object(map.clone());
            let found = path.iter().try_fold(&whole, |v, k| v.get(k));
            prop_assert_eq!(found, Some(&Value::from(value)));
            let after: Vec<&String> = map.keys().collect();
            prop_assert_eq!(&after[..before.len()], &before.iter().collect::<Vec<_>>()[..]);
        }

        #[test]
        fn every_whole_second_is_an_epoch(
            year in 0u32..10_000, month in 1u32..=12, day in 1u32..=28,
            hour in 0u32..24, minute in 0u32..60, second in 0u32..60,
        ) {
            let epoch = format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.000Z");
            prop_assert_eq!(check_epoch(&epoch), Ok(()));
        }
    }
}
