//! The inbound rate limiter, over the client's limiter with the reference's inbound table: every
//! route `rateLimit` per second (100); postBlock 2 per 2 s; getBlocks 2 per second; getPeers 2 per
//! second; getStatus and getCommonBlocks 9 per second; getUnconfirmedTransactions 2 per second;
//! postTransactions `rateLimitPostTransactions` per second (25). The forger's addresses are exempt:
//! the plugin chain does not ask for them. A request over a limit is answered 429 "Rate limit
//! exceeded".

use std::net::IpAddr;
use std::sync::{Mutex, PoisonError};

use heartwood_p2p_wire::RouteId;
use tokio::time::Instant;

use crate::rate_limiter::RateLimiter;

/// The inbound limits by address.
#[derive(Debug)]
pub struct InboundLimiter {
    limiter: Mutex<RateLimiter>,
}

impl InboundLimiter {
    /// A limiter of `rate_limit` requests and `post_transactions` postTransactions per second.
    pub fn new(rate_limit: u32, post_transactions: u32) -> InboundLimiter {
        let limiter = Mutex::new(RateLimiter::inbound(rate_limit, post_transactions));
        InboundLimiter { limiter }
    }

    /// Take a point for a request on `route` from `ip` at `now`; false when it exceeds a limit.
    pub fn consume(&self, ip: IpAddr, route: RouteId, now: Instant) -> bool {
        let mut limiter = self.limiter.lock().unwrap_or_else(PoisonError::into_inner);
        limiter.consume(ip, route, now).is_ok()
    }
}
