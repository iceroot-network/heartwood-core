//! A downloaded block, and how it becomes a [`Block`] as the reference's download path makes one.
//!
//! The communicator reads each transaction of a getBlocks reply as the reference's
//! `fromBytesUnsafe` reads it (`Transaction::decode_trusted`: the processor's block verification
//! checks it again), where the reference reads it, so a transaction it refuses fails the download
//! ([`read_listed`]). [`to_block`] then rebuilds the block from the header's fields and those
//! transactions as `BlockFactory.fromData` does (`blocks::factory::from_data`), with the id,
//! payload hash and signature of the canonical bytes.
//!
//! The header's fields are written as the reference's block serialiser writes them: an absent
//! version, transaction count or payload length is 0 (an absent count is the number of
//! transactions), hex text is read as Node reads it (pairs of digits, an odd last digit dropped),
//! and a field that does not give its exact number of bytes refuses the block, as the serialiser's
//! length assertion does.

use std::net::IpAddr;

use heartwood_crypto::blocks::{factory, serialiser};
use heartwood_crypto::{
    Block, BlockHeader, Milestones, Params, PublicKeyBytes, Signature, Transaction,
};
use heartwood_p2p_wire::codecs::Amount;
use heartwood_p2p_wire::codecs::blocks::ListedBlock;

use crate::errors::{AssembleError, DownloadError};

/// A block of a getBlocks reply that passed the reply check.
#[derive(Debug, Clone, PartialEq)]
pub struct DownloadedBlock {
    /// The header as the reply check left it (the generator's key lowercased). Its `transactions`
    /// field is not used: the transactions are in [`DownloadedBlock::transactions`].
    pub header: ListedBlock,
    /// The transactions, read as the reference's `fromBytesUnsafe` reads them; `None` for a block
    /// fetched with headers only.
    pub transactions: Option<Vec<Transaction>>,
    /// The address of the peer the block came from.
    pub ip: IpAddr,
}

impl DownloadedBlock {
    /// The height the header states (0 when absent, which the reply check never accepts).
    pub fn height(&self) -> u32 {
        self.header.header.height.unwrap_or(0)
    }
}

/// A block of a checked getBlocks reply from the peer at `ip`, with its transactions read as the
/// reference's `fromBytesUnsafe` reads them under `params` (the milestone the node is at), or
/// none when the blocks were fetched with headers only. A transaction that the reader refuses
/// fails the whole download from the peer, as it throws in the reference.
pub fn read_listed(
    mut listed: ListedBlock,
    ip: IpAddr,
    headers_only: bool,
    params: &Params,
) -> Result<DownloadedBlock, DownloadError> {
    let (raw, height) = (
        std::mem::take(&mut listed.transactions),
        listed.header.height,
    );
    let read = |bytes: &Vec<u8>| Transaction::decode_trusted(bytes, params);
    let failed = |source| DownloadError::Transaction {
        height: height.unwrap_or(0),
        source,
    };
    let transactions = match headers_only {
        true => None,
        false => Some(
            raw.iter()
                .map(read)
                .collect::<Result<_, _>>()
                .map_err(failed)?,
        ),
    };
    let header = listed;
    Ok(DownloadedBlock {
        header,
        transactions,
        ip,
    })
}

/// The block of `downloaded` as `BlockFactory.fromData` rebuilds it under `milestones`.
pub fn to_block(
    downloaded: &DownloadedBlock,
    milestones: &Milestones,
) -> Result<Block, AssembleError> {
    let listed = &downloaded.header;
    let fields = &listed.header;
    let given = downloaded.transactions.as_ref().map(Vec::len);
    // The block schema requires a stated count to equal the transactions given; the serialiser
    // writes the count, or the number of transactions when there is none.
    let number_of_transactions = match (fields.number_of_transactions, given) {
        (Some(count), Some(len)) if usize::try_from(count).ok() != Some(len) => {
            return Err(refused("numberOfTransactions"));
        }
        (Some(count), _) => count,
        (None, Some(len)) => u32::try_from(len).map_err(|_| refused("numberOfTransactions"))?,
        (None, None) => 0,
    };
    let key = bytes(fields.generator_public_key.as_deref(), "generatorPublicKey")?;
    let signature = bytes(fields.block_signature.as_deref(), "blockSignature")?;
    let header = BlockHeader {
        version: fields.version.unwrap_or(0),
        timestamp: fields.timestamp.ok_or(refused("timestamp"))?,
        height: fields.height.ok_or(refused("height"))?,
        previous_block: bytes(fields.previous_block.as_deref(), "previousBlock")?,
        number_of_transactions,
        total_amount: amount(listed.total_amount, "totalAmount")?,
        total_fee: amount(listed.total_fee, "totalFee")?,
        reward: amount(listed.reward, "reward")?,
        payload_length: fields.payload_length.unwrap_or(0),
        payload_hash: bytes(fields.payload_hash.as_deref(), "payloadHash")?,
        generator_public_key: PublicKeyBytes::from_array(key),
        block_signature: Signature::from_array(signature),
    };
    match &downloaded.transactions {
        // The genesis rules of height 1 are for the node's own chain definition: a block from a
        // peer at height 1 is received, as the reference exempts only its genesis block's ids.
        Some(transactions) if header.height == 1 => {
            let bytes = serialiser::serialise_with_transactions(&header, transactions);
            Ok(Block::receive(&bytes, milestones)?)
        }
        Some(transactions) => Ok(factory::from_data(header, transactions, milestones)?),
        // Without transactions the block is its header: written and read back.
        None => Block::decode(&serialiser::serialise_header(&header, true), milestones)
            .map_err(|error| AssembleError::Block(error.into())),
    }
}

fn refused(field: &'static str) -> AssembleError {
    AssembleError::Field { field }
}

/// An amount as the serialiser writes it: an unsigned 64-bit integer.
fn amount(value: Amount, field: &'static str) -> Result<u64, AssembleError> {
    u64::try_from(value.value()).map_err(|_| refused(field))
}

/// The `N` bytes of the hex `text` as Node reads hex; an absent field or another length refuses
/// the block.
fn bytes<const N: usize>(
    text: Option<&str>,
    field: &'static str,
) -> Result<[u8; N], AssembleError> {
    let text = text.ok_or(refused(field))?;
    node_hex(text).try_into().map_err(|_| refused(field))
}

/// The bytes of `text` as Node's `Buffer.from(text, "hex")` reads them: pairs of hex digits of
/// either case up to the first pair that is not one, an odd last digit dropped.
pub fn node_hex(text: &str) -> Vec<u8> {
    // A hex digit's value is below 16.
    let digit = |d: u8| char::from(d).to_digit(16).map(|v| v as u8);
    let (pairs, _) = text.as_bytes().as_chunks::<2>();
    pairs
        .iter()
        .map_while(|&[high, low]| Some(digit(high)?.wrapping_shl(4) | digit(low)?))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_reads_as_node_reads_it() {
        assert_eq!(node_hex("00Ff1a"), [0x00, 0xff, 0x1a]);
        assert_eq!(node_hex("abc"), [0xab]);
        assert_eq!(node_hex("ab0gff"), [0xab]);
        assert!(node_hex("").is_empty());
        assert!(node_hex("0").is_empty());
    }
}
