//! The reference's rate limiter: fixed windows per key, as `rate-limiter-flexible`'s memory
//! limiter keeps them (a window starts at the first point consumed and lasts its duration), one
//! global limiter and one per rate-limited route.
//!
//! The outgoing table, which the client applies to itself per peer IP so that it never exceeds a
//! reference server's limits:
//!
//! | Route | Points | Window |
//! |---|---|---|
//! | every request (global) | `rateLimit` (100) | 1 s |
//! | postBlock | 2 | 4 s |
//! | getBlocks | 1 | 2 s |
//! | getPeers | 1 | 1 s |
//! | getStatus, getCommonBlocks | 9 | 1 s |
//! | getUnconfirmedTransactions | 1 | 2 s |
//! | postTransactions | `rateLimitPostTransactions` (25) | 1 s |
//!
//! Time is tokio's, which the tests pause.

use std::net::IpAddr;
use std::time::Duration;

use heartwood_p2p_wire::RouteId;
use tokio::time::Instant;

/// One limit: `points` per window of `duration`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limit {
    /// The points a window allows.
    pub points: u32,
    /// The window's length.
    pub duration: Duration,
}

/// A request that would exceed a limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Exceeded;

/// The global limiter and the per-route limiters, keyed by IP.
#[derive(Debug, Clone)]
pub struct RateLimiter {
    #[expect(dead_code, reason = "the limiter's methods read it")]
    global: Limit,
    #[expect(dead_code, reason = "the limiter's methods read it")]
    routes: Vec<(RouteId, Limit)>,
}

impl RateLimiter {
    /// The outgoing limiter (`buildRateLimiter` with `isOutgoing`), with the configured
    /// `rate_limit` and `rate_limit_post_transactions`.
    pub fn outgoing(rate_limit: u32, rate_limit_post_transactions: u32) -> RateLimiter {
        let _ = (rate_limit, rate_limit_post_transactions);
        todo!("the outgoing table")
    }

    /// Consume a point for `route` from `ip` at `now`, in the global limiter and then in the
    /// route's; [`Exceeded`] when a limiter has no point left.
    pub fn consume(&mut self, ip: IpAddr, route: RouteId, now: Instant) -> Result<(), Exceeded> {
        let _ = (ip, route, now);
        todo!("consume a point")
    }

    /// Whether the global or the route's limiter has no point left for `ip` at `now`, without
    /// consuming (`hasExceededRateLimitNoConsume`).
    pub fn has_exceeded(&self, ip: IpAddr, route: RouteId, now: Instant) -> bool {
        let _ = (ip, route, now);
        todo!("check without consuming")
    }
}
