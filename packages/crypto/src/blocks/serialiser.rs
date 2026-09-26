//! Blocks to bytes.
//!
//! Header layout (141 bytes, little-endian): version (`u32`), timestamp (`u32`), height (`u32`),
//! previous block (32 bytes), number of transactions (`u32`), total amount, total fee and reward
//! (`u64` each), payload length (`u32`), payload hash (32 bytes), generator key (33 bytes); then
//! the 64-byte signature. A full block follows the header with one `u32` length per transaction and
//! then the transactions' bytes.

use crate::blocks::{Block, BlockHeader, BlockId};
use crate::crypto::hash::sha256;
use crate::transactions::Transaction;
use crate::utils::bytes::Writer;

/// Size of the header without the signature.
pub const HEADER_SIZE: usize = 141;

/// Size of the header signature.
pub const SIGNATURE_SIZE: usize = 64;

/// The header's bytes, with or without the signature.
pub fn serialise_header(header: &BlockHeader, include_signature: bool) -> Vec<u8> {
    let mut writer = Writer::with_capacity(HEADER_SIZE + SIGNATURE_SIZE);
    write_header(&mut writer, header, include_signature);
    writer.into_vec()
}

/// The SHA-256 of the header without its signature: the hash the generator signs.
pub fn signing_hash(header: &BlockHeader) -> [u8; 32] {
    sha256(&serialise_header(header, false))
}

/// The id of a header: the SHA-256 of its bytes with the signature.
pub fn block_id(header: &BlockHeader) -> BlockId {
    BlockId::from_array(sha256(&serialise_header(header, true)))
}

/// The full block: header, signature, transaction lengths and transactions.
///
/// Each transaction is written with the bytes it was decoded from. The reference writes a block
/// by serialising every transaction again from its fields, which gives the same bytes for every
/// transaction in canonical form.
pub fn serialise(block: &Block) -> Vec<u8> {
    serialise_with_transactions(&block.header, &block.transactions)
}

/// A full block from a signed header and its transactions.
pub fn serialise_with_transactions(header: &BlockHeader, transactions: &[Transaction]) -> Vec<u8> {
    serialise_parts(header, transactions.iter().map(Transaction::bytes))
}

/// A full block from a signed header and the bytes of its transactions.
pub(crate) fn serialise_parts<'a>(
    header: &BlockHeader,
    transactions: impl Iterator<Item = &'a [u8]> + Clone,
) -> Vec<u8> {
    let body: usize = transactions.clone().map(|bytes| 4 + bytes.len()).sum();
    let mut writer = Writer::with_capacity(HEADER_SIZE + SIGNATURE_SIZE + body);
    write_header(&mut writer, header, true);
    for bytes in transactions.clone() {
        // A transaction longer than 4 GiB cannot be decoded, so it is never in a block.
        writer.u32_le(u32::try_from(bytes.len()).unwrap_or(u32::MAX));
    }
    for bytes in transactions {
        writer.bytes(bytes);
    }
    writer.into_vec()
}

/// The block's size as the payload rule counts it: header, signature, and 4 bytes plus the bytes
/// of each transaction.
pub fn size(block: &Block) -> usize {
    HEADER_SIZE
        + SIGNATURE_SIZE
        + block
            .transactions
            .iter()
            .map(|transaction| 4 + transaction.bytes().len())
            .sum::<usize>()
}

fn write_header(writer: &mut Writer, header: &BlockHeader, include_signature: bool) {
    writer.u32_le(header.version);
    writer.u32_le(header.timestamp);
    writer.u32_le(header.height);
    writer.bytes(&header.previous_block);
    writer.u32_le(header.number_of_transactions);
    writer.u64_le(header.total_amount);
    writer.u64_le(header.total_fee);
    writer.u64_le(header.reward);
    writer.u32_le(header.payload_length);
    writer.bytes(&header.payload_hash);
    writer.bytes(header.generator_public_key.as_bytes());
    if include_signature {
        writer.bytes(header.block_signature.as_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::sig::Signature;
    use crate::identities::PublicKeyBytes;
    use crate::utils::hex;

    pub(crate) fn header() -> BlockHeader {
        BlockHeader {
            version: 0,
            timestamp: 8,
            height: 2,
            previous_block: [0xab; 32],
            number_of_transactions: 0,
            total_amount: 0,
            total_fee: 0,
            reward: 180_000_000,
            payload_length: 0,
            payload_hash: hex::decode_array(
                "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            )
            .unwrap(),
            generator_public_key: PublicKeyBytes::from_hex(
                "02dcb90a589d24e2d7e836ccc299f11d3f015ef86e208418d8b3d6de614a02cccd",
            )
            .unwrap(),
            block_signature: Signature::from_hex(
                "a7d0dc715e4af1a096efa9fb568167dc17ddaed3f57353c43792ea71ae0bac78\
                 999d76a91a3ba446fc8e80a824d28d1e80ed413cf7b61ae7f100e044243512b5",
            )
            .unwrap(),
        }
    }

    #[test]
    fn header_layout() {
        let header = header();
        let unsigned = serialise_header(&header, false);
        assert_eq!(unsigned.len(), HEADER_SIZE);
        let signed = serialise_header(&header, true);
        assert_eq!(signed.len(), HEADER_SIZE + SIGNATURE_SIZE);
        // Version, timestamp and height; previous block; transaction count, total amount, total
        // fee, reward and payload length; payload hash; generator key; signature.
        let expected = format!(
            "{}{}{}{}{}{}{}",
            "000000000800000002000000",
            "ab".repeat(32),
            "00000000000000000000000000000000000000000095ba0a0000000000000000",
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            "02dcb90a589d24e2d7e836ccc299f11d3f015ef86e208418d8b3d6de614a02cccd",
            "a7d0dc715e4af1a096efa9fb568167dc17ddaed3f57353c43792ea71ae0bac78",
            "999d76a91a3ba446fc8e80a824d28d1e80ed413cf7b61ae7f100e044243512b5",
        );
        assert_eq!(hex::encode(&signed), expected);
        assert_eq!(
            block_id(&header).to_hex(),
            "3053595187232667395a599f9fb9348df86369246c5c5b99a3b40ee062525c1e"
        );
        assert_eq!(signing_hash(&header), sha256(&unsigned));
    }

    #[test]
    fn transactions_follow_their_lengths() {
        let header = header();
        let parts: [&[u8]; 2] = [&[1, 2, 3], &[4]];
        let bytes = serialise_parts(&header, parts.into_iter());
        let body = bytes.get(HEADER_SIZE + SIGNATURE_SIZE..).unwrap();
        assert_eq!(body, [3, 0, 0, 0, 1, 0, 0, 0, 1, 2, 3, 4]);
    }
}
