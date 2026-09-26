//! Making blocks: signing a new header, assembling a block from a signed header, and reading the
//! reference's block JSON.
//!
//! Every path ends as the reference's `BlockFactory.fromData` does: the transactions are
//! serialised under the merged milestone at the block's height, the block is written out, and the
//! bytes are decoded again with [`Block::decode`]. [`Block::receive`] ends the same way, so a made
//! block has passed the same checks as a received one.

use serde_json::{Map, Value};

use crate::blocks::deserialiser::{self, Rules};
use crate::blocks::{Block, BlockHeader, serialiser};
use crate::crypto::sig::{self, Aux, SchemeId, Signature, SigningDomain};
use crate::errors::{BlockDecodeError, BlockError, JsonError};
use crate::identities::{KeyPair, PublicKeyBytes};
use crate::managers::Milestones;
use crate::transactions::serialiser::{SerialiseOptions, serialise as serialise_transaction};
use crate::transactions::{Transaction, TransactionData};
use crate::utils::{hex, js};
use crate::validation::{Mode, validate_transaction};

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
/// `BlockFactory.fromData` makes it: the reference's block schema requires the header to count
/// exactly the transactions given and applies the transaction rules to each transaction's fields
/// (removing vote names that are not delegate names), every transaction is serialised again from
/// its fields, and the result is decoded with [`Block::decode`].
pub fn from_data(
    header: BlockHeader,
    transactions: &[Transaction],
    milestones: &Milestones,
) -> Result<Block, BlockError> {
    assemble(header, Some(transactions), milestones, Rules::Decode)
}

/// `BlockFactory.fromData` on a header and, when the data has them, its transactions, with the
/// transactions of the result decoded under `rules`.
///
/// Without transactions the header's count is not checked and the block is written without
/// transactions, so it decodes to a block with none, whatever the header counts.
pub(crate) fn assemble(
    header: BlockHeader,
    transactions: Option<&[Transaction]>,
    milestones: &Milestones,
    rules: Rules,
) -> Result<Block, BlockError> {
    let params = milestones.at(header.height);
    if let Some(transactions) = transactions
        && header.number_of_transactions as usize != transactions.len()
    {
        return Err(count_refused(&header).into());
    }
    // The block schema applies the transaction rules to every transaction's fields, which also
    // removes the vote names that are not delegate names, before the block is written.
    let mode = match rules {
        Rules::Decode if header.height == 1 => Mode::Genesis,
        Rules::Decode | Rules::Received | Rules::Trusted => Mode::Strict,
    };
    let mut fields: Vec<TransactionData> = transactions
        .unwrap_or_default()
        .iter()
        .map(|transaction| transaction.data().clone())
        .collect();
    for (index, data) in fields.iter_mut().enumerate() {
        validate_transaction(data, params, mode)
            .map_err(|source| BlockError::TransactionSchema { index, source })?;
    }
    let serialised = fields
        .iter()
        .map(|data| serialise_transaction(data, SerialiseOptions::FULL, params))
        .collect::<Result<Vec<Vec<u8>>, _>>()?;
    let bytes = serialiser::serialise_parts(&header, serialised.iter().map(Vec::as_slice));
    Ok(deserialiser::decode_with(&bytes, milestones, rules)?)
}

/// The block schema's refusal of a header that does not count the transactions of the block.
fn count_refused(header: &BlockHeader) -> BlockDecodeError {
    BlockDecodeError::Schema {
        height: header.height,
        reason: "numberOfTransactions must equal the number of transactions",
    }
}

/// The block in the reference's JSON form, as `BlockFactory.fromJson` reads it.
///
/// `id` must be present and well formed, but the id is computed again from the header.
/// Amounts are decimal strings of digits, or JSON numbers with an integer value; hex fields have
/// their exact lengths; every header field is required except `numberOfTransactions`, which
/// defaults to the number of transactions, and `transactions`. Each transaction is read with
/// [`Transaction::from_json`] under the merged milestone at the block's height, and at height 1
/// under the rules for the configured genesis block's transactions (see [`Block::decode`]): this
/// reader is for the node's own chain definition.
pub fn from_json(json: &Value, milestones: &Milestones) -> Result<Block, BlockError> {
    let Value::Object(fields) = json else {
        return Err(JsonError::NotAnObject.into());
    };
    hex::decode_array::<32>(text(fields, "id")?)?;
    let height = integer(fields, "height")?;
    let params = milestones.at(height);
    let mode = if height == 1 {
        Mode::Genesis
    } else {
        Mode::Strict
    };
    let transactions = match fields.get("transactions") {
        None => None,
        Some(Value::Array(items)) => Some(
            items
                .iter()
                .enumerate()
                .map(|(index, item)| {
                    Transaction::from_json_with(item, params, mode)
                        .map_err(|source| BlockError::Transaction { index, source })
                })
                .collect::<Result<Vec<Transaction>, BlockError>>()?,
        ),
        Some(_) => {
            return Err(JsonError::Invalid {
                field: "transactions",
                expected: "an array",
            }
            .into());
        }
    };
    let number_of_transactions = match fields.get("numberOfTransactions") {
        None => u32::try_from(transactions.as_ref().map_or(0, Vec::len)).unwrap_or(u32::MAX),
        Some(_) => integer(fields, "numberOfTransactions")?,
    };
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
    assemble(header, transactions.as_deref(), milestones, Rules::Decode)
}

fn field<'a>(fields: &'a Map<String, Value>, key: &'static str) -> Result<&'a Value, BlockError> {
    fields
        .get(key)
        .ok_or(BlockError::Json(JsonError::Missing { field: key }))
}

fn text<'a>(fields: &'a Map<String, Value>, key: &'static str) -> Result<&'a str, BlockError> {
    field(fields, key)?
        .as_str()
        .ok_or(BlockError::Json(JsonError::Invalid {
            field: key,
            expected: "a string",
        }))
}

/// A header field of 32 bits: a JSON number with an integer value that fits.
fn integer(fields: &Map<String, Value>, key: &'static str) -> Result<u32, BlockError> {
    js::safe_integer(field(fields, key)?)
        .and_then(|value| u32::try_from(value).ok())
        .ok_or(BlockError::Json(JsonError::Invalid {
            field: key,
            expected: "an integer from 0 to 4294967295",
        }))
}

/// An amount: a string of decimal digits, or a JSON number with an integer value, that fits 64
/// bits.
fn amount(fields: &Map<String, Value>, key: &'static str) -> Result<u64, BlockError> {
    let refused = || {
        BlockError::Json(JsonError::Invalid {
            field: key,
            expected: "an amount that fits 64 bits",
        })
    };
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
