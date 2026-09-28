//! The blocks controller: getBlocks (from `lastBlockHeight + 1`, `blockLimit` or 400, headers only
//! when asked, nothing above the tip, cut when the transactions' size estimate reaches 20 MiB
//! minus 100 KiB, `<ip> has downloaded <n> blocks from height <h>`) and postBlock (the header
//! read, more transactions than the milestone allows refused; for a caller that is not the
//! forger, a pinged block answered at once and a block that does not chain to the last
//! downloaded block refused; the generator's delegate named and ranked, the reference's lines,
//! then the node's incoming block).

use super::ControllerParts;
use crate::server::seams::{ServerRefusal, ServerRequest};

/// getBlocks.
pub async fn get_blocks(
    parts: &ControllerParts,
    request: &ServerRequest,
) -> Result<Vec<u8>, ServerRefusal> {
    let _ = (parts, request);
    todo!("getBlocks")
}

/// postBlock.
pub async fn post_block(
    parts: &ControllerParts,
    request: &ServerRequest,
) -> Result<Vec<u8>, ServerRefusal> {
    let _ = (parts, request);
    todo!("postBlock")
}
