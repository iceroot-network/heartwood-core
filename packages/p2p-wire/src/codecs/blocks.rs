//! The codecs of the block routes: getBlocks and postBlock.

use super::Amount;
use crate::errors::DecodeError;
use crate::messages::Proto;
use crate::messages::blocks::{BlockHeader, GetBlocksRequest, PostBlockRequest, PostBlockResponse};

/// One block of a getBlocks response: `{...header, totalAmount, totalFee, reward, transactions}`.
///
/// The response is not a message but a list ([`crate::list`]) of encoded [`BlockHeader`]s. The
/// header keeps its fields' presence; its amounts are read as [`Amount`]s (an absent amount is
/// the empty text, which is 0) and its `transactions` field as a list of transactions, capped as
/// the reference caps it. The header's [`WireOrder`](crate::messages::WireOrder) lists the four
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
    let _ = blocks;
    todo!()
}

/// The getBlocks response: at most 401 blocks, each with at most 501 transactions.
pub fn get_blocks_decode_response(bytes: &[u8]) -> Result<Vec<ListedBlock>, DecodeError> {
    let _ = bytes;
    todo!()
}

/// The postBlock request: the message.
pub fn post_block_encode_request(request: &PostBlockRequest) -> Vec<u8> {
    request.encode()
}

/// The postBlock request: `{...decoded, block}`, where an absent block is empty. The block is
/// always `Some`, and noted in the wire order.
pub fn post_block_decode_request(bytes: &[u8]) -> Result<PostBlockRequest, DecodeError> {
    let _ = bytes;
    todo!()
}

/// The postBlock response: the message.
pub fn post_block_encode_response(response: &PostBlockResponse) -> Vec<u8> {
    response.encode()
}

/// The postBlock response: the message as decoded.
pub fn post_block_decode_response(bytes: &[u8]) -> Result<PostBlockResponse, DecodeError> {
    PostBlockResponse::decode(bytes)
}
