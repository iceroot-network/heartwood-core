//! The reference's subnet test of `maxSameSubnetPeers`.

use std::net::IpAddr;

/// Whether `a` and `b` are IPv4 addresses in the same /24 (`cidr(a/24) === cidr(b/24)`); IPv6
/// addresses never share one.
pub fn same_subnet(a: IpAddr, b: IpAddr) -> bool {
    let _ = (a, b);
    todo!("the /24 test")
}
