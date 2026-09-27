use std::net::{IpAddr, Ipv4Addr};
use std::time::Duration;

use heartwood_p2p_wire::RouteId;
use tokio::time::Instant;

use super::RateLimiter;

const PEER: IpAddr = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 7));
const OTHER: IpAddr = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 8));

fn at(start: Instant, ms: u64) -> Instant {
    start + Duration::from_millis(ms)
}

#[test]
fn a_window_starts_at_the_first_point_and_ends_its_duration_later() {
    let mut limiter = RateLimiter::outgoing(100, 25);
    let start = Instant::now();
    assert_eq!(limiter.consume(PEER, RouteId::GetBlocks, start), Ok(()));
    assert!(limiter.has_exceeded(PEER, RouteId::GetBlocks, at(start, 1_999)));
    assert!(!limiter.has_exceeded(OTHER, RouteId::GetBlocks, at(start, 1)));
    assert!(!limiter.has_exceeded(PEER, RouteId::GetPeers, at(start, 1)));
    // The window ends exactly two seconds after its first point.
    assert!(!limiter.has_exceeded(PEER, RouteId::GetBlocks, at(start, 2_000)));
    assert_eq!(
        limiter.consume(PEER, RouteId::GetBlocks, at(start, 2_000)),
        Ok(())
    );
    assert!(limiter.has_exceeded(PEER, RouteId::GetBlocks, at(start, 3_999)));
}

#[test]
fn points_beyond_the_limit_are_refused_and_still_counted() {
    let mut limiter = RateLimiter::outgoing(100, 25);
    let start = Instant::now();
    for _ in 0..2 {
        assert_eq!(limiter.consume(PEER, RouteId::PostBlock, start), Ok(()));
    }
    assert!(
        limiter
            .consume(PEER, RouteId::PostBlock, at(start, 10))
            .is_err()
    );
    assert!(limiter.has_exceeded(PEER, RouteId::PostBlock, at(start, 3_999)));
    assert!(!limiter.has_exceeded(PEER, RouteId::PostBlock, at(start, 4_000)));
}

#[test]
fn the_global_limit_covers_every_route() {
    let mut limiter = RateLimiter::outgoing(3, 25);
    let start = Instant::now();
    for route in [RouteId::GetStatus, RouteId::GetPeers, RouteId::GetStatus] {
        assert_eq!(limiter.consume(PEER, route, start), Ok(()));
    }
    assert!(limiter.has_exceeded(PEER, RouteId::GetCommonBlocks, start));
    // A refusal by the global limiter leaves the route's window alone.
    assert!(
        limiter
            .consume(PEER, RouteId::GetCommonBlocks, start)
            .is_err()
    );
    assert!(!limiter.has_exceeded(PEER, RouteId::GetCommonBlocks, at(start, 1_000)));
}

#[test]
fn post_transactions_zero_reads_as_the_default() {
    let mut limiter = RateLimiter::outgoing(100, 0);
    let start = Instant::now();
    for _ in 0..25 {
        assert_eq!(
            limiter.consume(PEER, RouteId::PostTransactions, start),
            Ok(())
        );
    }
    assert!(limiter.has_exceeded(PEER, RouteId::PostTransactions, start));
}

#[test]
fn expired_windows_are_dropped() {
    let mut limiter = RateLimiter::outgoing(100, 25);
    let start = Instant::now();
    for i in 0..5_000u32 {
        let ip = IpAddr::V4(Ipv4Addr::from(i));
        assert_eq!(limiter.consume(ip, RouteId::GetStatus, start), Ok(()));
    }
    // The windows of the first 5,000 have ended by then, and go when the bound is reached.
    let later = at(start, 5_000);
    for i in 5_000..8_200u32 {
        let ip = IpAddr::V4(Ipv4Addr::from(i));
        assert_eq!(limiter.consume(ip, RouteId::GetStatus, later), Ok(()));
    }
    assert!(limiter.windows.len() < 10_000, "{}", limiter.windows.len());
    assert!(!limiter.has_exceeded(IpAddr::V4(Ipv4Addr::from(1)), RouteId::GetStatus, later));
}
