use std::net::{IpAddr, Ipv4Addr};
use std::time::Duration;

use heartwood_p2p_wire::RouteId;
use tokio::time::Instant;

use super::{RECHECK, Rules};
use crate::seams::PeerRules;

const PEER: IpAddr = IpAddr::V4(Ipv4Addr::new(198, 51, 100, 4));

#[test]
fn the_throttle_waits_while_a_limit_is_spent_and_consumes_otherwise() {
    let rules = Rules::new(100, 25);
    let start = Instant::now();
    assert_eq!(rules.throttle(PEER, RouteId::GetPeers, start), None);
    assert!(rules.would_throttle(PEER, RouteId::GetPeers, start));
    assert_eq!(
        rules.throttle(PEER, RouteId::GetPeers, start + Duration::from_millis(999)),
        Some(RECHECK)
    );
    let later = start + Duration::from_secs(1);
    assert!(!rules.would_throttle(PEER, RouteId::GetPeers, later));
    assert_eq!(rules.throttle(PEER, RouteId::GetPeers, later), None);
}

#[test]
fn the_rules_are_the_wire_s_and_the_utilities() {
    let rules = Rules::new(100, 25);
    let mut ip = "::ffff:8.8.8.8".to_owned();
    assert!(rules.is_valid_peer(&mut ip, false, &[]));
    assert_eq!(ip, "8.8.8.8");
    assert!(rules.same_subnet(
        IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1)),
        IpAddr::V4(Ipv4Addr::new(192, 0, 2, 200))
    ));
    assert_eq!(
        rules.compare_versions("4.3.1", "4.3.1-heartwood.0.1.0"),
        Some(std::cmp::Ordering::Greater)
    );
    assert_eq!(rules.check_post_transactions(&[]), Ok(()));
}
