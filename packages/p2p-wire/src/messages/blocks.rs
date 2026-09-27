//! The messages of `blocks.proto`: getBlocks and postBlock.

use super::shared::Headers;
use crate::proto::message;

message! {
    /// The getBlocks request.
    GetBlocksRequest {
        /// The height after which blocks are wanted.
        1 last_block_height "lastBlockHeight": u32,
        /// The most blocks wanted.
        2 block_limit "blockLimit": u32,
        /// Whether only the headers are wanted.
        3 headers_only "headersOnly": bool,
        /// Whether transactions are wanted in their serialised form.
        4 serialised "serialised": bool,
        /// The client's headers.
        5 headers "headers": message(Headers),
    }
}

message! {
    /// The getBlocks response message. The route does not use it: its response is a list of
    /// [`BlockHeader`]s (see [`crate::codecs::blocks`]).
    GetBlocksResponse {
        /// The blocks.
        1 blocks "blocks": bytes,
    }
}

message! {
    /// One block of a getBlocks response (`GetBlocksResponse.BlockHeader`).
    BlockHeader {
        /// The block id, in hex.
        1 id "id": string,
        /// The block version.
        3 version "version": u32,
        /// Seconds since the network epoch.
        4 timestamp "timestamp": u32,
        /// The previous block's id, in hex.
        5 previous_block "previousBlock": string,
        /// The height.
        7 height "height": u32,
        /// The number of transactions.
        8 number_of_transactions "numberOfTransactions": u32,
        /// The sum of the transactions' amounts, as decimal text.
        9 total_amount "totalAmount": string,
        /// The sum of the transactions' fees, as decimal text.
        10 total_fee "totalFee": string,
        /// The block reward, as decimal text.
        11 reward "reward": string,
        /// The payload length.
        12 payload_length "payloadLength": u32,
        /// The payload hash, in hex.
        13 payload_hash "payloadHash": string,
        /// The generator's public key, in hex.
        14 generator_public_key "generatorPublicKey": string,
        /// The block signature, in hex.
        15 block_signature "blockSignature": string,
        /// The block's transactions as a length-prefixed list (see [`crate::list`]).
        16 transactions "transactions": bytes,
    }
}

message! {
    /// The postBlock request.
    PostBlockRequest {
        /// The serialised block.
        1 block "block": bytes,
        /// The client's headers.
        2 headers "headers": message(Headers),
    }
}

message! {
    /// The postBlock response.
    PostBlockResponse {
        /// Whether the server took the block.
        1 status "status": bool,
        /// The server's height.
        2 height "height": u32,
    }
}
