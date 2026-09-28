//! The peer controller: getPeers (the repository's peers but the caller, by latency, at most
//! 2,000, as `{ip, port}`), getCommonBlocks (the highest stored or in-memory block among the ids;
//! the reference's `lastBlockHeight` is not in the message and is not written) and getStatus
//! (the served status).
//!
//! The reference also leaves out peers with port -1, which a peer's port here cannot be.

use heartwood_crypto::BlockId;
use heartwood_crypto::blocks::serialiser::block_id;
use heartwood_p2p_wire::codecs::peer::{
    get_common_blocks_decode_request, get_common_blocks_encode_response, get_peers_encode_response,
};
use heartwood_p2p_wire::messages::peer::{Common, GetCommonBlocksResponse, Peer as Listed};

use super::{ControllerParts, internal, invalid};
use crate::server::seams::{ServerRefusal, ServerRequest};

/// The most peers a getPeers answer lists.
pub const MAX_PEERS: usize = 2_000;

/// getPeers.
pub async fn get_peers(
    parts: &ControllerParts,
    request: &ServerRequest,
) -> Result<Vec<u8>, ServerRefusal> {
    let mut peers = parts.peers.peers();
    peers.retain(|peer| peer.ip != request.ip);
    // A stable sort, as the reference's; a peer the repository holds always has a latency.
    peers.sort_by_key(|peer| peer.latency.unwrap_or(u32::MAX));
    let listed: Vec<Listed> = (peers.iter().take(MAX_PEERS))
        .map(|peer| Listed {
            ip: Some(peer.ip.to_string()),
            port: Some(u32::from(peer.port)),
            ..Listed::default()
        })
        .collect();
    Ok(get_peers_encode_response(&listed))
}

/// getCommonBlocks. The ids are matched as text, as the reference matches them: an id in
/// capitals names no block.
pub async fn get_common_blocks(
    parts: &ControllerParts,
    request: &ServerRequest,
) -> Result<Vec<u8>, ServerRefusal> {
    let asked = get_common_blocks_decode_request(&request.payload).map_err(invalid)?;
    let lowercase = |id: &&String| !id.bytes().any(|byte| byte.is_ascii_uppercase());
    let ids: Vec<BlockId> = (asked.ids.iter().filter(lowercase))
        .filter_map(|id| BlockId::from_hex(id).ok())
        .collect();
    let headers = parts.node.common_blocks(&ids).map_err(internal)?;
    let highest =
        (headers.iter().enumerate()).max_by_key(|(index, header)| (header.height, *index));
    let common = highest.map(|(_, header)| Common {
        height: Some(header.height),
        id: Some(block_id(header).to_hex()),
        ..Common::default()
    });
    Ok(get_common_blocks_encode_response(
        &GetCommonBlocksResponse {
            common,
            ..GetCommonBlocksResponse::default()
        },
    ))
}

/// getStatus.
pub async fn get_status(
    parts: &ControllerParts,
    _request: &ServerRequest,
) -> Result<Vec<u8>, ServerRefusal> {
    parts.status.get_status().await
}
