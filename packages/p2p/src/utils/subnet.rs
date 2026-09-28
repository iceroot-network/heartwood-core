//! The reference's subnet test of `maxSameSubnetPeers`.

use std::net::IpAddr;

/// Whether `a` and `b` are IPv4 addresses in the same /24 (`cidr(a/24) === cidr(b/24)`); IPv6
/// addresses, IPv4-mapped ones included, never share one.
pub fn same_subnet(a: IpAddr, b: IpAddr) -> bool {
    match (a, b) {
        (IpAddr::V4(a), IpAddr::V4(b)) => {
            let ([a1, a2, a3, _], [b1, b2, b3, _]) = (a.octets(), b.octets());
            (a1, a2, a3) == (b1, b2, b3)
        }
        _ => false,
    }
}
