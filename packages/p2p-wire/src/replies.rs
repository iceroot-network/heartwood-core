//! The reference's reply schemas of the six routes other than getStatus (whose check is
//! [`crate::status::check_reply`]), with the changes the schema library makes to a reply.
//!
//! | Route | Schema |
//! |---|---|
//! | getPeers | at most 2,000 entries `{ip, port}`: `ip` in the IPv4 or the IPv6 format, `port` from 0 to 65535 |
//! | getCommonBlocks | `{common}`: `null`, or a block with a height of at least 1 and a block id; no other key |
//! | getBlocks | at most 400 block headers (the block header schema, as getStatus's header); the generator's key is lowercased |
//! | postBlock | `{status?, height?}`: a boolean and an integer of at least 1; no other key |
//! | getUnconfirmedTransactions | `{poolSize?, transactions?}`: an integer of at least 0 and at most the largest `block.maxTransactions` of the milestones; no other key |
//! | postTransactions | an array |
//!
//! A refused reply names the part whose rule failed first ([`ReplyRefused`]). The communicator
//! counts a refusal as the reference's `CoreValidationError`.

use crate::codecs::blocks::ListedBlock;
use crate::codecs::transactions::WithTransactions;
use crate::errors::ReplyRefused;
use crate::messages::blocks::PostBlockResponse;
use crate::messages::peer::{GetCommonBlocksResponse, Peer};
use crate::messages::transactions::GetUnconfirmedTransactionsResponse;
use crate::routes::RouteId;

/// The six routes whose replies this module checks, in the order a selector byte picks them
/// (`byte % 6`), as the `reply_check` fuzz target and the oracle's `wire.checkReply` read a
/// selected reply.
pub const ROUTES: [RouteId; 6] = [
    RouteId::GetPeers,
    RouteId::GetCommonBlocks,
    RouteId::GetBlocks,
    RouteId::PostBlock,
    RouteId::GetUnconfirmedTransactions,
    RouteId::PostTransactions,
];

/// A getUnconfirmedTransactions reply, as its codec decodes it.
pub type UnconfirmedReply = WithTransactions<GetUnconfirmedTransactionsResponse>;

/// A getPeers entry that passed the reply schema: the address as text, in the IPv4 or the IPv6
/// format, and the port.
///
/// The text is kept as it came: a peer is accepted only after the reference's address rules have
/// processed it (an IPv4-mapped IPv6 address becomes IPv4), which rewrites [`PeerAddress::ip`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PeerAddress {
    /// The address, as text.
    pub ip: String,
    /// The peer-to-peer port.
    pub port: u16,
}

/// The common block of a getCommonBlocks reply that passed the reply schema.
///
/// The id is kept as it came: the schema accepts hex digits of either case, and the verifier
/// compares it with the ids it asked for as text, as the reference does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommonBlock {
    /// The block's height, at least 1.
    pub height: u32,
    /// The block's id: 64 hex digits of either case.
    pub id: String,
}

/// The getPeers reply schema; the entries as addresses when it passes.
pub fn check_peers(peers: &[Peer]) -> Result<Vec<PeerAddress>, ReplyRefused> {
    let _ = peers;
    todo!("the getPeers reply schema")
}

/// The getCommonBlocks reply schema; the common block, or `None` for `common: null`.
pub fn check_common_blocks(
    reply: &GetCommonBlocksResponse,
) -> Result<Option<CommonBlock>, ReplyRefused> {
    let _ = reply;
    todo!("the getCommonBlocks reply schema")
}

/// The getBlocks reply schema, with its change to each header: the generator's key lowercased.
/// A refused reply may have been changed in part.
pub fn check_blocks(blocks: &mut [ListedBlock]) -> Result<(), ReplyRefused> {
    let _ = blocks;
    todo!("the getBlocks reply schema")
}

/// The postBlock reply schema; the height, when the reply has one.
pub fn check_post_block(reply: &PostBlockResponse) -> Result<Option<u32>, ReplyRefused> {
    let _ = reply;
    todo!("the postBlock reply schema")
}

/// The getUnconfirmedTransactions reply schema. `max_transactions` is the largest
/// `block.maxTransactions` of the milestones, as the reference's schema module computes it when it
/// loads.
pub fn check_unconfirmed(
    reply: &UnconfirmedReply,
    max_transactions: usize,
) -> Result<(), ReplyRefused> {
    let _ = (reply, max_transactions);
    todo!("the getUnconfirmedTransactions reply schema")
}

/// The postTransactions reply schema, over the accepted ids the codec decoded.
pub fn check_post_transactions(accept: &[String]) -> Result<(), ReplyRefused> {
    let _ = accept;
    todo!("the postTransactions reply schema")
}
