//! The reference's address rules (`isValidPeer`), with ipaddr.js's parsing and range
//! classification.

use std::net::IpAddr;

/// `ipaddr.process(ip).toString()`: the address in ipaddr.js's text form, an IPv4-mapped IPv6
/// address as IPv4; `None` when ipaddr.js cannot parse `ip`.
pub fn process(ip: &str) -> Option<String> {
    let _ = ip;
    todo!("ipaddr.js's process")
}

/// `isValidPeer`: `ip` becomes its processed form when it parses, and is valid when its range is
/// `unicast` (or `private`, `linkLocal` or `uniqueLocal` with `allow_private`), its text does not
/// start with `0`, and it is none of `interfaces`, the host's own addresses.
pub fn is_valid_peer(ip: &mut String, allow_private: bool, interfaces: &[IpAddr]) -> bool {
    let _ = (ip, allow_private, interfaces);
    todo!("the address rules")
}
