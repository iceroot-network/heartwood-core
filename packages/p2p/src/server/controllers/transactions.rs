//! The transactions controller: getUnconfirmedTransactions (the collator's candidates, validated
//! only for the forger, without the excluded ids; none for `countOnly`; the pool's size) and
//! postTransactions (the P2P channel's pool processor; the accepted ids).

use super::ControllerParts;
use crate::server::seams::{ServerRefusal, ServerRequest};

/// getUnconfirmedTransactions.
pub async fn get_unconfirmed(
    parts: &ControllerParts,
    request: &ServerRequest,
) -> Result<Vec<u8>, ServerRefusal> {
    let _ = (parts, request);
    todo!("getUnconfirmedTransactions")
}

/// postTransactions.
pub async fn post_transactions(
    parts: &ControllerParts,
    request: &ServerRequest,
) -> Result<Vec<u8>, ServerRefusal> {
    let _ = (parts, request);
    todo!("postTransactions")
}
