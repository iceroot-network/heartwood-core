//! [`Rules`]: the wire's requests and reply checks, the outgoing rate limiter and the peer rules,
//! behind [`PeerRules`].

use std::cmp::Ordering;
use std::net::IpAddr;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use heartwood_crypto::Params;
use heartwood_p2p_wire::RouteId;
use heartwood_p2p_wire::codecs::blocks::ListedBlock;
use heartwood_p2p_wire::errors::ReplyRefused;
use heartwood_p2p_wire::messages::blocks::PostBlockResponse;
use heartwood_p2p_wire::messages::peer::{GetCommonBlocksResponse, Peer as ListedPeer};
use heartwood_p2p_wire::replies::{self, CommonBlock, PeerAddress, UnconfirmedReply};
use heartwood_p2p_wire::requests::{ClientRequest, Headers};
use tokio::time::Instant;

use super::{is_valid_peer, is_valid_version, npm_semver, subnet};
use crate::rate_limiter::RateLimiter;
use crate::seams::PeerRules;

/// How long the throttle waits before it asks the limiter again (the reference's
/// `msBeforeReCheck`).
pub const RECHECK: Duration = Duration::from_secs(1);

/// The peer rules, with the outgoing rate limiter's state.
#[derive(Debug)]
pub struct Rules {
    limiter: Mutex<RateLimiter>,
}

impl Rules {
    /// The rules with an outgoing limiter of `rate_limit` requests per second per peer and
    /// `rate_limit_post_transactions` postTransactions per second per peer.
    pub fn new(rate_limit: u32, rate_limit_post_transactions: u32) -> Rules {
        Rules {
            limiter: Mutex::new(RateLimiter::outgoing(
                rate_limit,
                rate_limit_post_transactions,
            )),
        }
    }

    fn limiter<R>(&self, use_it: impl FnOnce(&mut RateLimiter) -> R) -> R {
        let mut limiter = self.limiter.lock().unwrap_or_else(PoisonError::into_inner);
        use_it(&mut limiter)
    }
}

impl PeerRules for Rules {
    fn encode_request(&self, request: &ClientRequest, headers: &Headers) -> Vec<u8> {
        request.encode(headers)
    }

    fn check_peers(&self, peers: &[ListedPeer]) -> Result<Vec<PeerAddress>, ReplyRefused> {
        replies::check_peers(peers)
    }

    fn check_common_blocks(
        &self,
        reply: &GetCommonBlocksResponse,
    ) -> Result<Option<CommonBlock>, ReplyRefused> {
        replies::check_common_blocks(reply)
    }

    fn check_blocks(&self, blocks: &mut [ListedBlock]) -> Result<(), ReplyRefused> {
        replies::check_blocks(blocks)
    }

    fn check_post_block(&self, reply: &PostBlockResponse) -> Result<Option<u32>, ReplyRefused> {
        replies::check_post_block(reply)
    }

    fn check_unconfirmed(
        &self,
        reply: &UnconfirmedReply,
        max_transactions: usize,
    ) -> Result<(), ReplyRefused> {
        replies::check_unconfirmed(reply, max_transactions)
    }

    fn check_post_transactions(&self, accept: &[String]) -> Result<(), ReplyRefused> {
        replies::check_post_transactions(accept)
    }

    /// The reference's throttle step: when a limit would be exceeded, wait [`RECHECK`] and ask
    /// again; otherwise consume, ignoring a refusal as the reference does.
    fn throttle(&self, ip: IpAddr, route: RouteId, now: Instant) -> Option<Duration> {
        self.limiter(|limiter| {
            if limiter.has_exceeded(ip, route, now) {
                return Some(RECHECK);
            }
            let _ = limiter.consume(ip, route, now);
            None
        })
    }

    fn would_throttle(&self, ip: IpAddr, route: RouteId, now: Instant) -> bool {
        self.limiter(|limiter| limiter.has_exceeded(ip, route, now))
    }

    fn is_valid_peer(&self, ip: &mut String, allow_private: bool, interfaces: &[IpAddr]) -> bool {
        is_valid_peer::is_valid_peer(ip, allow_private, interfaces)
    }

    fn is_valid_version(
        &self,
        version: Option<&str>,
        milestone: &Params,
        configured: &[String],
        network_name: &str,
    ) -> bool {
        is_valid_version::is_valid_version(version, milestone, configured, network_name)
    }

    fn same_subnet(&self, a: IpAddr, b: IpAddr) -> bool {
        subnet::same_subnet(a, b)
    }

    fn compare_versions(&self, a: &str, b: &str) -> Option<Ordering> {
        npm_semver::compare(a, b)
    }
}

#[cfg(test)]
mod tests;
