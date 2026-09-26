//! Making blocks: signing a new header, assembling a block from a signed header, and reading the
//! reference's block JSON.
//!
//! Every path ends as the reference's `BlockFactory.fromData` does: the transactions are
//! serialised under the merged milestone at the block's height, the block is written out, and the
//! bytes are decoded again with [`Block::decode`], so a made block has passed the same checks as a
//! received one.

use serde_json::{Map, Value};

use crate::blocks::{Block, BlockHeader, serialiser};
use crate::crypto::sig::{self, Aux, SchemeId, Signature, SigningDomain};
use crate::errors::BlockError;
use crate::identities::{KeyPair, PublicKeyBytes};
use crate::managers::Milestones;
use crate::transactions::Transaction;
use crate::transactions::serialiser::{SerialiseOptions, serialise as serialise_transaction};
use crate::utils::{hex, js};
use crate::validation::Mode;

/// The header fields of a block to sign; the generator key comes from the signing keys.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockDraft {
    /// The block version.
    pub version: u32,
    /// Seconds since the network epoch.
    pub timestamp: u32,
    /// The height.
    pub height: u32,
    /// The previous block's id (32 zero bytes before the first block).
    pub previous_block: [u8; 32],
    /// The number of transactions, as the header states it.
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
}

/// Sign `draft` with `keys` (BIP340, `aux`) and make the block with `transactions`, as the
/// reference's `BlockFactory.make` does: the header is signed as given, and the result is decoded
/// again under the merged milestone at its height.
///
/// Blocks are always signed with BIP340, whatever the milestone's `bip340` says.
pub fn make(
    draft: BlockDraft,
    transactions: Vec<Transaction>,
    keys: &KeyPair,
    aux: Aux,
    milestones: &Milestones,
) -> Result<Block, BlockError> {
    let mut header = BlockHeader {
        version: draft.version,
        timestamp: draft.timestamp,
        height: draft.height,
        previous_block: draft.previous_block,
        number_of_transactions: draft.number_of_transactions,
        total_amount: draft.total_amount,
        total_fee: draft.total_fee,
        reward: draft.reward,
        payload_length: draft.payload_length,
        payload_hash: draft.payload_hash,
        generator_public_key: keys.public_key().to_compressed(),
        block_signature: Signature::from_array([0; 64]),
    };
    header.block_signature = sig::sign(
        SchemeId::Secp256k1Bip340,
        SigningDomain::Block,
        &serialiser::signing_hash(&header),
        keys.secret_key(),
        aux,
    )?;
    from_data(header, &transactions, milestones)
}

/// The block with the signed `header` and `transactions`, as the reference's
/// `BlockFactory.fromData` makes it.
///
/// A header that counts no transactions is written with the number of `transactions`, as the
/// reference writes `numberOfTransactions || transactions.length`; the signature then no longer
/// matches unless the header was signed that way.
pub fn from_data(
    header: BlockHeader,
    transactions: &[Transaction],
    milestones: &Milestones,
) -> Result<Block, BlockError> {
    let params = milestones.at(header.height);
    let mut header = header;
    if header.number_of_transactions == 0 {
        header.number_of_transactions = u32::try_from(transactions.len()).unwrap_or(u32::MAX);
    }
    let serialised = transactions
        .iter()
        .map(|transaction| {
            serialise_transaction(transaction.data(), SerialiseOptions::FULL, params)
        })
        .collect::<Result<Vec<Vec<u8>>, _>>()?;
    let bytes = serialiser::serialise_parts(&header, serialised.iter().map(Vec::as_slice));
    Ok(Block::decode(&bytes, milestones)?)
}

/// The block in the reference's JSON form, as `BlockFactory.fromJson` reads it.
///
/// `id` must be present and well formed, but the id is computed again from the header.
/// Amounts are decimal strings of digits, or JSON numbers with an integer value; hex fields have
/// their exact lengths; every header field is required except `numberOfTransactions`, which
/// defaults to the number of transactions. Each transaction is read with
/// [`Transaction::from_json`] under the merged milestone at the block's height, and at height 1,
/// the genesis block, under the rules for genesis transactions (see [`Block::decode`]).
pub fn from_json(json: &Value, milestones: &Milestones) -> Result<Block, BlockError> {
    let Value::Object(fields) = json else {
        return Err(BlockError::Json("a block is a JSON object".into()));
    };
    hex::decode_array::<32>(text(fields, "id")?)?;
    let height = integer(fields, "height")?;
    let params = milestones.at(height);
    let mode = if height == 1 {
        Mode::GENESIS
    } else {
        Mode::STRICT
    };
    let transactions = match fields.get("transactions") {
        None => Vec::new(),
        Some(Value::Array(items)) => items
            .iter()
            .enumerate()
            .map(|(index, item)| {
                Transaction::from_json_with(item, params, mode)
                    .map_err(|source| BlockError::Transaction { index, source })
            })
            .collect::<Result<Vec<Transaction>, BlockError>>()?,
        Some(_) => return Err(BlockError::Json("transactions must be an array".into())),
    };
    let number_of_transactions = match fields.get("numberOfTransactions") {
        None => u32::try_from(transactions.len()).unwrap_or(u32::MAX),
        Some(_) => integer(fields, "numberOfTransactions")?,
    };
    if fields.contains_key("transactions") && number_of_transactions as usize != transactions.len()
    {
        return Err(BlockError::Json(
            "numberOfTransactions must equal the number of transactions".into(),
        ));
    }
    let header = BlockHeader {
        version: integer(fields, "version")?,
        timestamp: integer(fields, "timestamp")?,
        height,
        previous_block: hex::decode_array(text(fields, "previousBlock")?)?,
        number_of_transactions,
        total_amount: amount(fields, "totalAmount")?,
        total_fee: amount(fields, "totalFee")?,
        reward: amount(fields, "reward")?,
        payload_length: integer(fields, "payloadLength")?,
        payload_hash: hex::decode_array(text(fields, "payloadHash")?)?,
        generator_public_key: PublicKeyBytes::from_hex(text(fields, "generatorPublicKey")?)?,
        block_signature: Signature::from_hex(text(fields, "blockSignature")?)?,
    };
    from_data(header, &transactions, milestones)
}

fn field<'a>(fields: &'a Map<String, Value>, key: &str) -> Result<&'a Value, BlockError> {
    fields
        .get(key)
        .ok_or_else(|| BlockError::Json(format!("{key} is missing")))
}

fn text<'a>(fields: &'a Map<String, Value>, key: &str) -> Result<&'a str, BlockError> {
    field(fields, key)?
        .as_str()
        .ok_or_else(|| BlockError::Json(format!("{key} must be a string")))
}

/// A header field of 32 bits: a JSON number with an integer value that fits.
fn integer(fields: &Map<String, Value>, key: &str) -> Result<u32, BlockError> {
    js::safe_integer(field(fields, key)?)
        .and_then(|value| u32::try_from(value).ok())
        .ok_or_else(|| BlockError::Json(format!("{key} must be an integer from 0 to 4294967295")))
}

/// An amount: a string of decimal digits, or a JSON number with an integer value, that fits 64
/// bits.
fn amount(fields: &Map<String, Value>, key: &str) -> Result<u64, BlockError> {
    let refused = || BlockError::Json(format!("{key} must be an amount that fits 64 bits"));
    match field(fields, key)? {
        Value::String(digits)
            if !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit()) =>
        {
            digits.parse().map_err(|_| refused())
        }
        Value::Number(number) => {
            let value = js::number_value(number);
            // Every double from 0 to 2^64 with no fraction converts exactly.
            if value.trunc() == value && (0.0..18_446_744_073_709_551_616.0).contains(&value) {
                Ok(value as u64)
            } else {
                Err(refused())
            }
        }
        _ => Err(refused()),
    }
}
