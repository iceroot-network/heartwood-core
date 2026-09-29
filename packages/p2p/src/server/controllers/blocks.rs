//! The blocks controller: getBlocks (from `lastBlockHeight + 1`, `blockLimit` or 400, headers only
//! when asked, nothing above the tip, cut when the transactions' size estimate reaches 20 MiB
//! minus 100 KiB, `<ip> has downloaded <n> blocks from height <h>`) and postBlock (the header
//! read, more transactions than the milestone allows refused; for a caller that is not the
//! forger, a pinged block answered at once and a block that does not chain to the last
//! downloaded block refused; the generator's delegate named and ranked, the reference's lines,
//! then the node's incoming block).
//!
//! As in the reference, a block that cannot be read, or that has more transactions than the
//! milestone allows, is a handler error: a 500 answer that ends the socket without a ban.

use heartwood_crypto::Block;
use heartwood_crypto::blocks::deserialiser::deserialise_header;
use heartwood_crypto::blocks::serialiser::block_id;
use heartwood_crypto::utils::bytes::Reader;
use heartwood_crypto::utils::format_satoshi;
use heartwood_kernel::{ChainLink, debug, info, is_block_chained};
use heartwood_p2p_wire::codecs::blocks::{
    get_blocks_decode_request, get_blocks_encode_listing, post_block_decode_request,
    post_block_encode_response,
};
use heartwood_p2p_wire::messages::blocks::PostBlockResponse;

use super::{ControllerParts, internal, invalid};
use crate::broadcaster::{grouped, plural};
use crate::server::seams::{ServerRefusal, ServerRequest};

/// The blocks a getBlocks answers when it names no limit.
pub const DEFAULT_BLOCK_LIMIT: u32 = 400;
/// The size estimate at which getBlocks stops adding blocks: 20 MiB, the server's largest
/// message, less a margin of 100 KiB.
pub const MAX_ESTIMATE: usize = 20 * 1024 * 1024 - 100 * 1024;

/// getBlocks.
pub async fn get_blocks(
    parts: &ControllerParts,
    request: &ServerRequest,
) -> Result<Vec<u8>, ServerRefusal> {
    let asked = get_blocks_decode_request(&request.payload).map_err(invalid)?;
    let limit = (asked.block_limit.filter(|&limit| limit != 0)).unwrap_or(DEFAULT_BLOCK_LIMIT);
    let last_height = parts.node.chain().last_block().height();
    let from = asked.last_block_height.unwrap_or(0).checked_add(1);
    let Some(from) = from.filter(|&from| from <= last_height) else {
        return Ok(get_blocks_encode_listing(&[]));
    };
    let headers_only = asked.headers_only == Some(true);
    let blocks = (parts.node.blocks_for_download(from, limit, headers_only)).map_err(internal)?;
    // The estimate counts the transactions' hex text, as the store gives them to the reference.
    let (mut estimate, mut sent) = (0_usize, Vec::new());
    for mut block in blocks {
        if estimate >= MAX_ESTIMATE {
            break;
        }
        // The reference's rows name the genesis block's previous block as an id of zeros.
        if block.height == 1 && block.previous_block.is_none() {
            block.previous_block = Some("0".repeat(64));
        }
        let hex = block
            .transactions
            .iter()
            .map(|tx| tx.len().saturating_mul(2));
        estimate = estimate.saturating_add(hex.sum());
        sent.push(block);
    }
    let (count, height) = (plural("block", sent.len()), grouped(from.into()));
    info!(
        "{} has downloaded {count} from height {height} :floppy_disk:",
        request.ip
    );
    Ok(get_blocks_encode_listing(&sent))
}

/// postBlock.
pub async fn post_block(
    parts: &ControllerParts,
    request: &ServerRequest,
) -> Result<Vec<u8>, ServerRefusal> {
    let posted = post_block_decode_request(&request.payload).map_err(invalid)?;
    let bytes = posted.block.unwrap_or_default();
    let header = deserialise_header(&mut Reader::new(&bytes)).map_err(internal)?;
    let chain = parts.node.chain();
    let (milestones, network) = (chain.milestones(), chain.network());
    let last_height = || parts.node.chain().last_block().height();
    let answer = |status: bool, height: u32| {
        post_block_encode_response(&PostBlockResponse {
            status: Some(status),
            height: Some(height),
            ..PostBlockResponse::default()
        })
    };
    let params = milestones.at(chain.last_block().height());
    if params
        .max_transactions()
        .is_some_and(|most| header.number_of_transactions > most)
    {
        return Err(internal("too many transactions"));
    }
    if !request.from_forger {
        if parts.node.ping_block(&header) {
            return Ok(answer(true, last_height()));
        }
        let next = ChainLink::of_header(&header, block_id(&header));
        let downloaded = parts.node.last_downloaded_block();
        if is_block_chained(&downloaded, &next, milestones).is_err() {
            return Ok(answer(false, last_height()));
        }
    }
    let block = Block::decode(&bytes, milestones).map_err(internal)?;
    let (height, reward) = (block.height(), block.header().reward);
    let named = (block.header().version == 0)
        .then(|| chain.delegate_by_key(&block.header().generator_public_key))
        .flatten();
    let Some(username) = named.map(|delegate| delegate.username) else {
        return Ok(answer(false, last_height()));
    };
    let Some(rank) = parts.node.delegate_rank(&username) else {
        return Ok(answer(false, last_height()));
    };
    info!(
        "Received new block forged by delegate {username} (#{rank}) at height {} with {} reward :package:",
        grouped(height.into()),
        format_satoshi(reward.into(), &network.symbol)
    );
    let dynamic = params.dynamic_reward().filter(|table| table.enabled());
    if let Some(table) = dynamic.filter(|table| reward == table.secondary_reward())
        && (parts.node.already_forged(height, &username)).map_err(internal)?
    {
        // The reference compares with the rank's reward of the table, and throws where the table
        // has none.
        let ranked = table
            .rank(rank)
            .ok_or_else(|| internal("no reward for the rank"))?;
        if ranked != reward {
            info!("The reward was reduced because {username} already forged in this round :fire:");
        }
    }
    debug!("The id of the new block is {}", block.id().to_hex());
    let count = usize::try_from(block.header().number_of_transactions).unwrap_or(usize::MAX);
    debug!(
        "It contains {} and was received from {}",
        plural("transaction", count),
        request.ip
    );
    parts
        .node
        .handle_incoming_block(block, request.from_forger, request.ip);
    Ok(answer(true, last_height()))
}
