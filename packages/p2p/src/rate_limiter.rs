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
//! | postTransactions | `rateLimitPostTransactions` (25; 0 also reads as 25) | 1 s |
//!
//! A window is kept per IP and limiter. It starts at the first point consumed and ends its
//! duration later, when the reference's timer deletes it: a point consumed at or after the end
//! starts a new window. A point consumed beyond the limit is still counted.
//!
//! Time is tokio's, which the tests pause.

use std::collections::HashMap;
use std::net::IpAddr;
use std::time::Duration;

use heartwood_p2p_wire::RouteId;
use tokio::time::Instant;

/// The windows kept before expired ones are dropped; the bound doubles with the live ones.
const PRUNE_AT: usize = 4_096;

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

/// The points consumed in one window, and when it ends.
#[derive(Debug, Clone, Copy)]
struct Window {
    ends: Instant,
    consumed: u32,
}

/// The global limiter and the per-route limiters, keyed by IP.
#[derive(Debug, Clone)]
pub struct RateLimiter {
    global: u32,
    post_transactions: u32,
    /// The windows by IP and limiter (`None`: the global one).
    windows: HashMap<(IpAddr, Option<RouteId>), Window>,
    prune_at: usize,
}

impl RateLimiter {
    /// The outgoing limiter (`buildRateLimiter` with `isOutgoing`), with the configured
    /// `rate_limit` and `rate_limit_post_transactions`.
    pub fn outgoing(rate_limit: u32, rate_limit_post_transactions: u32) -> RateLimiter {
        RateLimiter {
            global: rate_limit,
            post_transactions: match rate_limit_post_transactions {
                0 => 25,
                points => points,
            },
            windows: HashMap::new(),
            prune_at: PRUNE_AT,
        }
    }

    /// The limit of `route` (`None`: the global limit), or `None` when the route has none.
    pub fn limit(&self, route: Option<RouteId>) -> Option<Limit> {
        let (points, seconds) = match route {
            None => (self.global, 1),
            Some(RouteId::PostBlock) => (2, 4),
            Some(RouteId::GetBlocks | RouteId::GetUnconfirmedTransactions) => (1, 2),
            Some(RouteId::GetPeers) => (1, 1),
            Some(RouteId::GetStatus | RouteId::GetCommonBlocks) => (9, 1),
            Some(RouteId::PostTransactions) => (self.post_transactions, 1),
            Some(_) => return None,
        };
        let duration = Duration::from_secs(seconds);
        Some(Limit { points, duration })
    }

    /// Consume a point for `route` from `ip` at `now`, in the global limiter and then in the
    /// route's; [`Exceeded`] when a limiter has no point left. When the global limiter refuses,
    /// the route's is not asked.
    pub fn consume(&mut self, ip: IpAddr, route: RouteId, now: Instant) -> Result<(), Exceeded> {
        if self.windows.len() >= self.prune_at {
            self.windows.retain(|_, window| window.ends > now);
            self.prune_at = self.windows.len().saturating_mul(2).max(PRUNE_AT);
        }
        self.take(ip, None, now)?;
        self.take(ip, Some(route), now)
    }

    /// Whether the global or the route's limiter has no point left for `ip` at `now`, without
    /// consuming (`hasExceededRateLimitNoConsume`).
    pub fn has_exceeded(&self, ip: IpAddr, route: RouteId, now: Instant) -> bool {
        [None, Some(route)].into_iter().any(|key| {
            let window = self.windows.get(&(ip, key));
            (self.limit(key).zip(window))
                .is_some_and(|(limit, window)| window.ends > now && window.consumed >= limit.points)
        })
    }

    /// Count a point in the window of `ip` and `key`, which starts now when there is none; nothing
    /// for a route without a limit.
    fn take(&mut self, ip: IpAddr, key: Option<RouteId>, now: Instant) -> Result<(), Exceeded> {
        let Some(limit) = self.limit(key) else {
            return Ok(());
        };
        let fresh = Window {
            ends: now.checked_add(limit.duration).unwrap_or(now),
            consumed: 0,
        };
        let window = self.windows.entry((ip, key)).or_insert(fresh);
        if window.ends <= now {
            *window = fresh;
        }
        window.consumed = window.consumed.saturating_add(1);
        if window.consumed > limit.points {
            return Err(Exceeded);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
