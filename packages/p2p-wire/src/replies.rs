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
use crate::messages::peer::{GetCommonBlocksResponse, Peer, StatusHeader};
use crate::messages::transactions::GetUnconfirmedTransactionsResponse;
use crate::routes::RouteId;
use crate::status::{header_passes, is_hex};

#[cfg(test)]
mod tests;

/// The most entries a getPeers reply may list.
pub const MAX_PEERS: usize = 2_000;
/// The most blocks a getBlocks reply may hold.
pub const MAX_BLOCKS: usize = 400;

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
///
/// A decoded entry is an object of the protobuf library, so an absent field reads as its default:
/// an entry without an address has the empty text, which neither format accepts, and one without
/// a port has port 0.
pub fn check_peers(peers: &[Peer]) -> Result<Vec<PeerAddress>, ReplyRefused> {
    if peers.len() > MAX_PEERS {
        return Err(ReplyRefused::Root);
    }
    peers
        .iter()
        .map(|peer| {
            let ip = peer.ip.clone().unwrap_or_default();
            let port = u16::try_from(peer.port.unwrap_or(0)).map_err(|_| ReplyRefused::Peer)?;
            if is_ipv4_format(&ip) || is_ipv6_format(&ip) {
                Ok(PeerAddress { ip, port })
            } else {
                Err(ReplyRefused::Peer)
            }
        })
        .collect()
}

/// The getCommonBlocks reply schema; the common block, or `None` for `common: null`.
///
/// The decoded reply is an object of the protobuf library: an absent `common` reads as `null`,
/// and an absent field of a present one as its default (height 0, the empty text), which the
/// schema refuses.
pub fn check_common_blocks(
    reply: &GetCommonBlocksResponse,
) -> Result<Option<CommonBlock>, ReplyRefused> {
    let Some(common) = &reply.common else {
        return Ok(None);
    };
    let height = common.height.unwrap_or(0);
    let id = common.id.clone().unwrap_or_default();
    if height == 0 || !is_hex(&id, Some(64)) {
        return Err(ReplyRefused::Common);
    }
    Ok(Some(CommonBlock { height, id }))
}

/// The getBlocks reply schema, with its change to each header: the generator's key lowercased.
/// The blocks are checked in order and each is changed once it has passed, so a refused reply may
/// have been changed in part.
pub fn check_blocks(blocks: &mut [ListedBlock]) -> Result<(), ReplyRefused> {
    if blocks.len() > MAX_BLOCKS {
        return Err(ReplyRefused::Root);
    }
    for block in blocks {
        let listed = &block.header;
        let header = StatusHeader {
            id: listed.id.clone(),
            timestamp: listed.timestamp,
            previous_block: listed.previous_block.clone(),
            height: listed.height,
            payload_hash: listed.payload_hash.clone(),
            generator_public_key: listed.generator_public_key.clone(),
            block_signature: listed.block_signature.clone(),
            ..StatusHeader::default()
        };
        if !header_passes(&header, [block.total_amount, block.total_fee, block.reward]) {
            return Err(ReplyRefused::Header);
        }
        if let Some(key) = block.header.generator_public_key.as_mut() {
            key.make_ascii_lowercase();
        }
    }
    Ok(())
}

/// The postBlock reply schema; the height. The decoded reply is an object of the protobuf
/// library, so an absent height reads as 0, which the schema refuses: an accepted reply always
/// has a height.
pub fn check_post_block(reply: &PostBlockResponse) -> Result<Option<u32>, ReplyRefused> {
    match reply.height {
        Some(height) if height >= 1 => Ok(Some(height)),
        _ => Err(ReplyRefused::PostBlock),
    }
}

/// The getUnconfirmedTransactions reply schema. `max_transactions` is the largest
/// `block.maxTransactions` of the milestones, as the reference's schema module computes it when it
/// loads. The pool size is an unsigned integer whenever it is present, and the schema deletes the
/// `headers` a server may add, which a client does not read.
pub fn check_unconfirmed(
    reply: &UnconfirmedReply,
    max_transactions: usize,
) -> Result<(), ReplyRefused> {
    if reply.transactions.len() > max_transactions {
        return Err(ReplyRefused::Unconfirmed);
    }
    Ok(())
}

/// The postTransactions reply schema, over the accepted ids the codec decoded: an array, which a
/// decoded reply always is.
pub fn check_post_transactions(accept: &[String]) -> Result<(), ReplyRefused> {
    let _ = accept;
    Ok(())
}

/// The schema library's `ipv4` format: four dotted numbers from 0 to 255 of one to three digits,
/// leading zeros allowed.
fn is_ipv4_format(text: &str) -> bool {
    let octets: Vec<&str> = text.split('.').collect();
    octets.len() == 4 && octets.iter().all(|octet| is_octet(octet, true))
}

/// One to three decimal digits of a value up to 255; without `leading_zeros`, no leading `0`
/// except in `0` itself.
fn is_octet(text: &str, leading_zeros: bool) -> bool {
    (1..=3).contains(&text.len())
        && text.bytes().all(|b| b.is_ascii_digit())
        && text.parse::<u16>().is_ok_and(|value| value <= 255)
        && (leading_zeros || text == "0" || !text.starts_with('0'))
}

/// The schema library's `ipv6` format: white space around the address; an optional zone, `%` and
/// at least one character, with line breaks only among its trailing white space; eight groups of
/// one to four hex digits, the last two of which may be a dotted IPv4 address without leading
/// zeros, or at most seven groups around one `::`.
fn is_ipv6_format(text: &str) -> bool {
    let text = text.trim_start_matches(is_js_space);
    let body = match text.split_once('%') {
        Some((body, zone)) => {
            let line_break = |c: char| matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}');
            let tail = zone.find(line_break).and_then(|at| zone.get(at..));
            if zone.is_empty()
                || zone.starts_with(line_break)
                || !tail.is_none_or(|tail| tail.chars().all(is_js_space))
            {
                return false;
            }
            body
        }
        None => text.trim_end_matches(is_js_space),
    };
    // The groups of a part, a dotted IPv4 address at its end counting two.
    let groups = |part: &str| -> Option<usize> {
        if part.is_empty() {
            return Some(0);
        }
        let pieces: Vec<&str> = part.split(':').collect();
        let last = pieces.len().saturating_sub(1);
        pieces
            .iter()
            .enumerate()
            .try_fold(0usize, |count, (i, piece)| {
                let quad: Vec<&str> = piece.split('.').collect();
                let size =
                    if i == last && quad.len() == 4 && quad.iter().all(|o| is_octet(o, false)) {
                        2
                    } else if (1..=4).contains(&piece.len())
                        && piece.bytes().all(|b| b.is_ascii_hexdigit())
                    {
                        1
                    } else {
                        return None;
                    };
                count.checked_add(size)
            })
    };
    match body.split_once("::") {
        None => groups(body) == Some(8),
        Some((head, tail)) => {
            !head.contains('.')
                && groups(head)
                    .zip(groups(tail))
                    .is_some_and(|(before, after)| before.saturating_add(after) <= 7)
        }
    }
}

/// JavaScript's white space and line terminators, which `\s` matches: Unicode's white space
/// without U+0085, and U+FEFF.
fn is_js_space(c: char) -> bool {
    (c.is_whitespace() && c != '\u{85}') || c == '\u{FEFF}'
}
