//! The peer controller: getPeers (the repository's peers but the caller and those with port -1,
//! by latency, at most 2,000, as `{ip, port}`), getCommonBlocks (the highest stored or in-memory
//! block among the ids, and the tip's height) and getStatus (the served status).

use super::ControllerParts;
use crate::server::seams::{ServerRefusal, ServerRequest};

/// getPeers.
pub async fn get_peers(
    parts: &ControllerParts,
    request: &ServerRequest,
) -> Result<Vec<u8>, ServerRefusal> {
    let _ = (parts, request);
    todo!("getPeers")
}

/// getCommonBlocks.
pub async fn get_common_blocks(
    parts: &ControllerParts,
    request: &ServerRequest,
) -> Result<Vec<u8>, ServerRefusal> {
    let _ = (parts, request);
    todo!("getCommonBlocks")
}

/// getStatus.
pub async fn get_status(
    parts: &ControllerParts,
    request: &ServerRequest,
) -> Result<Vec<u8>, ServerRefusal> {
    let _ = (parts, request);
    todo!("getStatus")
}
