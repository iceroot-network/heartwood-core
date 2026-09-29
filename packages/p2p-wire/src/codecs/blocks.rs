//! The codecs of the block routes: getBlocks and postBlock.

use super::Amount;
use crate::errors::DecodeError;
use crate::list;
use crate::messages::Proto;
use crate::messages::blocks::{BlockHeader, GetBlocksRequest, PostBlockRequest, PostBlockResponse};

/// The field numbers of `BlockHeader` that the getBlocks response's spread sets, in its order:
/// `totalAmount`, `totalFee`, `reward` and `transactions`.
const SPREAD_BLOCK_KEYS: [u32; 4] = [9, 10, 11, 16];
/// The field number of `PostBlockRequest.block`.
const BLOCK: u32 = 1;

/// One block of a getBlocks response: `{...header, totalAmount, totalFee, reward, transactions}`.
///
/// The response is not a message but a list ([`crate::list`]) of encoded [`BlockHeader`]s. The
/// header keeps its fields' presence; its amounts are read as [`Amount`]s (an absent amount is
/// the empty text, which is 0) and its `transactions` field as a list of transactions, capped as
/// the reference caps it. In a decoded block, the header's amount and transaction fields are
/// always present and hold the text and bytes as they came (the empty text and no bytes where
/// they were absent), and the header's [`WireOrder`](crate::messages::WireOrder) lists the four
/// keys the spread sets.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ListedBlock {
    /// The header as decoded. When encoding, its amount and transaction fields are not used.
    pub header: BlockHeader,
    /// The sum of the transactions' amounts.
    pub total_amount: Amount,
    /// The sum of the transactions' fees.
    pub total_fee: Amount,
    /// The block reward.
    pub reward: Amount,
    /// The serialised transactions; empty for a block sent without them.
    pub transactions: Vec<Vec<u8>>,
}

/// What the server writes for one block of a getBlocks response: each value once, as the store
/// gives the block. The genesis block has no previous block.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BlockListing {
    /// The block id, in hex.
    pub id: String,
    /// The block version.
    pub version: u32,
    /// Seconds since the network epoch.
    pub timestamp: u32,
    /// The previous block's id, in hex; none for the genesis block.
    pub previous_block: Option<String>,
    /// The height.
    pub height: u32,
    /// The number of transactions.
    pub number_of_transactions: u32,
    /// The sum of the transactions' amounts.
    pub total_amount: Amount,
    /// The sum of the transactions' fees.
    pub total_fee: Amount,
    /// The block reward.
    pub reward: Amount,
    /// The payload length.
    pub payload_length: u32,
    /// The payload hash, in hex.
    pub payload_hash: String,
    /// The generator's public key, in hex.
    pub generator_public_key: String,
    /// The block signature, in hex.
    pub block_signature: String,
    /// The serialised transactions; empty for a block sent without them.
    pub transactions: Vec<Vec<u8>>,
}

/// The getBlocks request: the message.
pub fn get_blocks_encode_request(request: &GetBlocksRequest) -> Vec<u8> {
    request.encode()
}

/// The getBlocks request: the message as decoded.
pub fn get_blocks_decode_request(bytes: &[u8]) -> Result<GetBlocksRequest, DecodeError> {
    GetBlocksRequest::decode(bytes)
}

/// The getBlocks response: per block, the header with its amounts as decimal text and its
/// transactions as a list (a present field, empty for a block sent without them), each encoded
/// header with a `u32` big-endian length before it.
pub fn get_blocks_encode_response(blocks: &[ListedBlock]) -> Vec<u8> {
    let headers: Vec<Vec<u8>> = blocks
        .iter()
        .map(|block| {
            BlockHeader {
                total_amount: Some(block.total_amount.to_string()),
                total_fee: Some(block.total_fee.to_string()),
                reward: Some(block.reward.to_string()),
                transactions: Some(list::encode(&block.transactions)),
                ..block.header.clone()
            }
            .encode()
        })
        .collect();
    list::encode(&headers)
}

/// The getBlocks response: at most 401 blocks, each with at most 501 transactions.
///
/// The blocks are read in order, each header and then its three amounts, so the refusal is the
/// first one the reference meets. An amount beyond 128 bits, which the reference reads, is
/// refused only where the reference raises no error.
pub fn get_blocks_decode_response(bytes: &[u8]) -> Result<Vec<ListedBlock>, DecodeError> {
    let mut too_wide = false;
    let mut listed = Vec::new();
    for item in list::items(bytes, list::MAX_BLOCKS) {
        let mut header = BlockHeader::decode(item)?;
        let transactions = list::decode(
            header.transactions.get_or_insert_default(),
            list::MAX_BLOCK_TRANSACTIONS,
        );
        let mut amount =
            |text: &mut Option<String>| match Amount::parse(text.get_or_insert_default()) {
                Err(DecodeError::AmountTooWide) => {
                    too_wide = true;
                    Ok(Amount::ZERO)
                }
                read => read,
            };
        let total_amount = amount(&mut header.total_amount)?;
        let total_fee = amount(&mut header.total_fee)?;
        let reward = amount(&mut header.reward)?;
        for number in SPREAD_BLOCK_KEYS {
            header.wire_order.note(number);
        }
        listed.push(ListedBlock {
            header,
            total_amount,
            total_fee,
            reward,
            transactions,
        });
    }
    if too_wide {
        return Err(DecodeError::AmountTooWide);
    }
    Ok(listed)
}

/// The postBlock request: the message.
pub fn post_block_encode_request(request: &PostBlockRequest) -> Vec<u8> {
    request.encode()
}

/// The postBlock request: `{...decoded, block}`, where an absent block is empty. The block is
/// always `Some`, and noted in the wire order.
pub fn post_block_decode_request(bytes: &[u8]) -> Result<PostBlockRequest, DecodeError> {
    let mut request = PostBlockRequest::decode(bytes)?;
    request.block.get_or_insert_default();
    request.wire_order.note(BLOCK);
    Ok(request)
}

/// The postBlock response: the message.
pub fn post_block_encode_response(response: &PostBlockResponse) -> Vec<u8> {
    response.encode()
}

/// The postBlock response: the message as decoded.
pub fn post_block_decode_response(bytes: &[u8]) -> Result<PostBlockResponse, DecodeError> {
    PostBlockResponse::decode(bytes)
}

/// The getBlocks response as the server writes it from `blocks`: the bytes that
/// [`get_blocks_encode_response`] writes for the same blocks.
pub fn get_blocks_encode_listing(blocks: &[BlockListing]) -> Vec<u8> {
    let headers: Vec<Vec<u8>> = blocks
        .iter()
        .map(|block| {
            BlockHeader {
                id: Some(block.id.clone()),
                version: Some(block.version),
                timestamp: Some(block.timestamp),
                previous_block: block.previous_block.clone(),
                height: Some(block.height),
                number_of_transactions: Some(block.number_of_transactions),
                total_amount: Some(block.total_amount.to_string()),
                total_fee: Some(block.total_fee.to_string()),
                reward: Some(block.reward.to_string()),
                payload_length: Some(block.payload_length),
                payload_hash: Some(block.payload_hash.clone()),
                generator_public_key: Some(block.generator_public_key.clone()),
                block_signature: Some(block.block_signature.clone()),
                transactions: Some(list::encode(&block.transactions)),
                ..BlockHeader::default()
            }
            .encode()
        })
        .collect();
    list::encode(&headers)
}

#[cfg(test)]
mod listing_tests {
    //! The server's listings write the bytes of every V13 response record they can hold (class
    //! V13, read from `tests/vectors` of the crate).

    use serde_json::Value;

    use super::{BlockListing, get_blocks_encode_listing};
    use crate::codecs::Amount;
    use crate::codecs::transactions::{
        UnconfirmedListing, get_unconfirmed_transactions_encode_listing,
    };
    use heartwood_crypto::utils::hex;

    fn records() -> Vec<Value> {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/vectors/V13-p2p-wire.jsonl"
        );
        let text = std::fs::read_to_string(path).unwrap();
        let records = text.lines().map(|line| serde_json::from_str(line).unwrap());
        records.collect()
    }

    fn text(value: &Value, key: &str) -> Option<String> {
        Some(value.get(key)?.as_str()?.to_owned())
    }

    fn number(value: &Value, key: &str) -> Option<u32> {
        u32::try_from(value.get(key)?.as_u64()?).ok()
    }

    fn amount(value: &Value, key: &str) -> Option<Amount> {
        Amount::parse(value.get(key)?.as_str()?).ok()
    }

    fn transactions(value: &Value) -> Option<Vec<Vec<u8>>> {
        let hexes = value
            .as_array()?
            .iter()
            .map(|tx| hex::decode(tx.as_str()?).ok());
        hexes.collect()
    }

    /// A block of a record's value as the store gives it: every field, the previous block
    /// optional, the transactions absent for a headers-only block.
    fn listing(block: &Value) -> Option<BlockListing> {
        let keys = block
            .as_object()?
            .keys()
            .filter(|key| !["previousBlock", "transactions"].contains(&key.as_str()));
        (keys.count() == 12).then_some(())?;
        Some(BlockListing {
            id: text(block, "id")?,
            version: number(block, "version")?,
            timestamp: number(block, "timestamp")?,
            previous_block: text(block, "previousBlock"),
            height: number(block, "height")?,
            number_of_transactions: number(block, "numberOfTransactions")?,
            total_amount: amount(block, "totalAmount")?,
            total_fee: amount(block, "totalFee")?,
            reward: amount(block, "reward")?,
            payload_length: number(block, "payloadLength")?,
            payload_hash: text(block, "payloadHash")?,
            generator_public_key: text(block, "generatorPublicKey")?,
            block_signature: text(block, "blockSignature")?,
            transactions: block
                .get("transactions")
                .map_or(Some(Vec::new()), transactions)?,
        })
    }

    fn unconfirmed(value: &Value) -> Option<UnconfirmedListing> {
        (value.as_object()?.len() == 2).then_some(())?;
        Some(UnconfirmedListing {
            pool_size: number(value, "poolSize")?,
            transactions: transactions(value.get("transactions")?)?,
        })
    }

    #[test]
    fn the_listings_write_the_reference_s_response_bytes() {
        let mut checked = [0, 0];
        for record in records() {
            let input = &record["input"];
            if record["op"] != "wire.encode" || input["direction"] != "response" {
                continue;
            }
            let expected = &record["output"]["hex"];
            let value = &input["value"];
            let bytes = match input["route"].as_str() {
                Some("getBlocks") => {
                    let blocks: Option<Vec<BlockListing>> =
                        value.as_array().unwrap().iter().map(listing).collect();
                    let Some(blocks) = blocks else {
                        continue;
                    };
                    checked[0] += 1;
                    get_blocks_encode_listing(&blocks)
                }
                Some("getUnconfirmedTransactions") => {
                    let Some(listing) = unconfirmed(value) else {
                        continue;
                    };
                    checked[1] += 1;
                    get_unconfirmed_transactions_encode_listing(&listing)
                }
                _ => continue,
            };
            assert_eq!(
                &Value::from(hex::encode(&bytes)),
                expected,
                "{}",
                record["name"]
            );
        }
        assert!(checked[0] >= 3 && checked[1] >= 1, "{checked:?}");
    }
}
