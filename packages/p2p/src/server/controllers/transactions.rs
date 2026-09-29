//! The transactions controller: getUnconfirmedTransactions (the collator's candidates, validated
//! only for the forger, without the excluded ids; none for `countOnly`; the pool's size) and
//! postTransactions (the P2P channel's pool processor; the accepted ids).
//!
//! The pool's size is the one the collation reads on the node thread, after the collator's
//! removals; with `countOnly`, the pool's published size.

use heartwood_crypto::TransactionId;
use heartwood_p2p_wire::codecs::transactions::{
    UnconfirmedListing, get_unconfirmed_transactions_decode_request,
    get_unconfirmed_transactions_encode_listing, post_transactions_decode_request,
    post_transactions_encode_response,
};

use super::{ControllerParts, internal, invalid};
use crate::server::seams::{ServerRefusal, ServerRequest};

/// getUnconfirmedTransactions.
pub async fn get_unconfirmed(
    parts: &ControllerParts,
    request: &ServerRequest,
) -> Result<Vec<u8>, ServerRefusal> {
    let asked = get_unconfirmed_transactions_decode_request(&request.payload).map_err(invalid)?;
    let listing = if asked.request.count_only == Some(true) {
        UnconfirmedListing {
            pool_size: u32::try_from(parts.node.pool_size()).unwrap_or(u32::MAX),
            transactions: Vec::new(),
        }
    } else {
        let exclude = asked.exclude.into_iter().map(TransactionId::from_array);
        let collation = (parts
            .node
            .collate(request.from_forger, exclude.collect())
            .await)
            .map_err(internal)?;
        UnconfirmedListing {
            pool_size: u32::try_from(collation.pool_size).unwrap_or(u32::MAX),
            transactions: collation.transactions,
        }
    };
    Ok(get_unconfirmed_transactions_encode_listing(&listing))
}

/// postTransactions.
pub async fn post_transactions(
    parts: &ControllerParts,
    request: &ServerRequest,
) -> Result<Vec<u8>, ServerRefusal> {
    let posted = post_transactions_decode_request(&request.payload).map_err(invalid)?;
    let accepted = (parts.node.post_transactions(posted.transactions).await).map_err(internal)?;
    let ids: Vec<String> = accepted.iter().map(TransactionId::to_hex).collect();
    Ok(post_transactions_encode_response(&ids))
}
