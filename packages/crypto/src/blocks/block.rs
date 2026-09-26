//! A block, its id and its verification rules.

use std::collections::HashSet;
use std::fmt;

use serde_json::{Map, Value, json};

use crate::blocks::serialiser;
use crate::crypto::hash::sha256_concat;
use crate::crypto::sig::{self, SchemeId, Signature, SigningDomain};
use crate::errors::HexError;
use crate::identities::{Address, PublicKeyBytes};
use crate::managers::Milestones;
use crate::transactions::{Transaction, TransactionId};
use crate::utils::{burn, donations, hex};

/// A block id: the SHA-256 of the serialised header, signature included.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BlockId([u8; 32]);

impl BlockId {
    /// The id with these 32 bytes.
    pub const fn from_array(bytes: [u8; 32]) -> BlockId {
        BlockId(bytes)
    }

    /// The id in the hex `text` (64 hex digits).
    pub fn from_hex(text: &str) -> Result<BlockId, HexError> {
        hex::decode_array(text).map(BlockId)
    }

    /// The 32 bytes.
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Lowercase hex, as the reference writes ids.
    pub fn to_hex(&self) -> String {
        hex::encode(&self.0)
    }
}

impl fmt::Display for BlockId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

/// A block header, as serialised: 141 bytes, then the 64-byte signature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockHeader {
    /// The block version.
    pub version: u32,
    /// Seconds since the network epoch.
    pub timestamp: u32,
    /// The height.
    pub height: u32,
    /// The previous block's id (32 zero bytes before the first block).
    pub previous_block: [u8; 32],
    /// The number of transactions.
    pub number_of_transactions: u32,
    /// The sum of the transactions' top-level amounts.
    pub total_amount: u64,
    /// The sum of the transactions' fees.
    pub total_fee: u64,
    /// The block reward.
    pub reward: u64,
    /// The payload length: 32 bytes per transaction.
    pub payload_length: u32,
    /// The SHA-256 of the concatenated transaction ids.
    pub payload_hash: [u8; 32],
    /// The generator's public key, as the header holds it.
    pub generator_public_key: PublicKeyBytes,
    /// The generator's BIP340 signature of the header.
    pub block_signature: Signature,
}

/// A block: header, id and transactions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    pub(crate) header: BlockHeader,
    pub(crate) id: BlockId,
    pub(crate) transactions: Vec<Transaction>,
}

impl Block {
    /// The header.
    pub fn header(&self) -> &BlockHeader {
        &self.header
    }

    /// The id.
    pub fn id(&self) -> &BlockId {
        &self.id
    }

    /// The height.
    pub fn height(&self) -> u32 {
        self.header.height
    }

    /// The transactions, in block order.
    pub fn transactions(&self) -> &[Transaction] {
        &self.transactions
    }

    /// The ids of the transactions, in block order.
    pub fn transaction_ids(&self) -> impl Iterator<Item = &TransactionId> {
        self.transactions.iter().map(Transaction::id)
    }

    /// The sum of the burned part of every fee, under the merged milestone at the block's height.
    ///
    /// The sum is exact whatever the fees; for a block that verifies it is at most the header's
    /// total fee, so it fits 64 bits.
    pub fn burned_fee(&self, milestones: &Milestones) -> u128 {
        let params = milestones.at(self.header.height);
        self.transactions
            .iter()
            .map(|transaction| u128::from(burn::burned_fee(transaction.fee(), params)))
            .sum()
    }

    /// The donations taken from the block reward, under the merged milestone at the block's height,
    /// in file order.
    pub fn donations(&self, milestones: &Milestones) -> Vec<(Address, u64)> {
        donations::calculate_donations(self.header.reward, milestones.at(self.header.height))
    }

    /// Check the block as the reference's `Block.verify` does, under the merged milestone at the
    /// block's height. `current_time` is the time in seconds since the network epoch, which a block
    /// timestamp may not exceed by more than one block time.
    ///
    /// Every rule is checked, and every failure is listed in the reference's order. The
    /// reference's check for expired transactions has no counterpart: none of the transaction
    /// types this crate carries has an expiration.
    pub fn verify(&self, milestones: &Milestones, current_time: i64) -> Verification {
        let header = &self.header;
        let params = milestones.at(header.height);
        let mut failures = Vec::new();

        if !self.verify_signature() {
            failures.push(VerifyFailure::Signature);
        }
        if header.version != params.block_version() {
            failures.push(VerifyFailure::Version);
        }
        if i128::from(header.timestamp) > i128::from(current_time) + i128::from(params.block_time())
        {
            failures.push(VerifyFailure::Timestamp);
        }
        let size = serialiser::size(self);
        if let Some(max) = params.max_payload()
            && size > max as usize
        {
            failures.push(VerifyFailure::PayloadTooLarge { size, max });
        }
        let unverified: Vec<TransactionId> = self
            .transactions
            .iter()
            .filter(|transaction| !transaction.is_verified())
            .map(|transaction| *transaction.id())
            .collect();
        if !unverified.is_empty() {
            failures.push(VerifyFailure::UnverifiedTransactions(unverified));
        }
        if self.transactions.len() != header.number_of_transactions as usize {
            failures.push(VerifyFailure::TransactionCount);
        }
        if u64::from(header.payload_length) != 32 * u64::from(header.number_of_transactions) {
            failures.push(VerifyFailure::PayloadLength);
        }
        if let Some(max) = params.max_transactions()
            && self.transactions.len() > max as usize
            && header.height > 1
        {
            failures.push(VerifyFailure::TooManyTransactions);
        }

        let mut seen = HashSet::with_capacity(self.transactions.len());
        let mut total_amount: u128 = 0;
        let mut total_fee: u128 = 0;
        for transaction in &self.transactions {
            if !seen.insert(*transaction.id()) {
                failures.push(VerifyFailure::DuplicateTransaction(*transaction.id()));
            }
            total_amount += u128::from(transaction.amount());
            total_fee += u128::from(transaction.fee());
        }
        if total_amount != u128::from(header.total_amount) {
            failures.push(VerifyFailure::TotalAmount);
        }
        if total_fee != u128::from(header.total_fee) {
            failures.push(VerifyFailure::TotalFee);
        }
        let payload_hash = sha256_concat(self.transaction_ids().map(|id| id.as_bytes().as_slice()));
        if payload_hash != header.payload_hash {
            failures.push(VerifyFailure::PayloadHash);
        }

        Verification { failures }
    }

    /// Whether the header signature is a valid BIP340 signature by the generator key.
    ///
    /// The 33-byte generator key is read as its x coordinate, whatever its prefix byte, as in the
    /// reference.
    pub fn verify_signature(&self) -> bool {
        sig::verify(
            SchemeId::Secp256k1Bip340,
            SigningDomain::Block,
            &serialiser::signing_hash(&self.header),
            &self.header.block_signature,
            self.header.generator_public_key.as_bytes(),
        )
    }

    /// The block as the reference's `toJson` writes it, transactions included.
    ///
    /// Each transaction carries, after its own keys, the block id, the height, its position in
    /// the block and its burned fee, as the reference adds them.
    pub fn to_json(&self, milestones: &Milestones) -> Value {
        let header = &self.header;
        let params = milestones.at(header.height);
        let donations: Map<String, Value> = self
            .donations(milestones)
            .into_iter()
            .map(|(address, amount)| (address.to_base58(), json!(amount.to_string())))
            .collect();
        let transactions: Vec<Value> = self
            .transactions
            .iter()
            .enumerate()
            .map(|(sequence, transaction)| {
                let mut json = transaction.to_json(params);
                if let Value::Object(map) = &mut json {
                    map.insert("blockId".into(), json!(self.id.to_hex()));
                    map.insert("blockHeight".into(), json!(header.height));
                    map.insert("sequence".into(), json!(sequence));
                    map.insert(
                        "burnedFee".into(),
                        json!(burn::burned_fee(transaction.fee(), params).to_string()),
                    );
                }
                json
            })
            .collect();

        let mut map = Map::new();
        map.insert("version".into(), json!(header.version));
        map.insert("timestamp".into(), json!(header.timestamp));
        map.insert("height".into(), json!(header.height));
        map.insert(
            "previousBlock".into(),
            json!(hex::encode(&header.previous_block)),
        );
        map.insert(
            "numberOfTransactions".into(),
            json!(header.number_of_transactions),
        );
        map.insert("totalAmount".into(), json!(header.total_amount.to_string()));
        map.insert("totalFee".into(), json!(header.total_fee.to_string()));
        map.insert("reward".into(), json!(header.reward.to_string()));
        map.insert("payloadLength".into(), json!(header.payload_length));
        map.insert(
            "payloadHash".into(),
            json!(hex::encode(&header.payload_hash)),
        );
        map.insert(
            "generatorPublicKey".into(),
            json!(header.generator_public_key.to_hex()),
        );
        map.insert(
            "blockSignature".into(),
            json!(header.block_signature.to_hex()),
        );
        map.insert("id".into(), json!(self.id.to_hex()));
        map.insert(
            "burnedFee".into(),
            json!(self.burned_fee(milestones).to_string()),
        );
        map.insert("donations".into(), Value::Object(donations));
        map.insert("transactions".into(), Value::Array(transactions));
        Value::Object(map)
    }
}

/// One failed block rule, in the order the reference checks them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifyFailure {
    /// The header signature does not verify.
    Signature,
    /// The version is not the milestone's block version.
    Version,
    /// The timestamp is more than one block time in the future.
    Timestamp,
    /// The serialised block is larger than the milestone's maximum payload.
    PayloadTooLarge {
        /// The block's size in bytes.
        size: usize,
        /// The maximum.
        max: u32,
    },
    /// Transactions whose signature did not verify.
    UnverifiedTransactions(Vec<TransactionId>),
    /// The header's transaction count differs from the transactions.
    TransactionCount,
    /// The payload length is not 32 bytes per transaction.
    PayloadLength,
    /// More transactions than the milestone allows (above height 1).
    TooManyTransactions,
    /// A transaction appears twice.
    DuplicateTransaction(TransactionId),
    /// The total amount differs from the transactions' sum.
    TotalAmount,
    /// The total fee differs from the transactions' sum.
    TotalFee,
    /// The payload hash differs from the hash of the transaction ids.
    PayloadHash,
}

impl fmt::Display for VerifyFailure {
    /// The reference's text for the failure; the unverified transactions are named by id.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VerifyFailure::Signature => f.write_str("Failed to verify block signature"),
            VerifyFailure::Version => f.write_str("Invalid block version"),
            VerifyFailure::Timestamp => f.write_str("Invalid block timestamp"),
            VerifyFailure::PayloadTooLarge { size, max } => {
                write!(f, "Payload is too large: {size} > {max}")
            }
            VerifyFailure::UnverifiedTransactions(ids) => {
                f.write_str("One or more transactions are not verified:")?;
                for id in ids {
                    write!(f, " {id}")?;
                }
                Ok(())
            }
            VerifyFailure::TransactionCount => f.write_str("Invalid number of transactions"),
            VerifyFailure::PayloadLength => f.write_str("Invalid payload length"),
            VerifyFailure::TooManyTransactions => f.write_str("Transactions length is too high"),
            VerifyFailure::DuplicateTransaction(id) => {
                write!(f, "Encountered duplicate transaction: {id}")
            }
            VerifyFailure::TotalAmount => f.write_str("Invalid total amount"),
            VerifyFailure::TotalFee => f.write_str("Invalid total fee"),
            VerifyFailure::PayloadHash => f.write_str("Invalid payload hash"),
        }
    }
}

/// The outcome of [`Block::verify`]: every failed rule.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Verification {
    /// The failed rules, in check order.
    pub failures: Vec<VerifyFailure>,
}

impl Verification {
    /// Whether every rule passed.
    pub fn is_verified(&self) -> bool {
        self.failures.is_empty()
    }
}
