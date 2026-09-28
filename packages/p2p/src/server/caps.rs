//! The inbound caps: at most `maxSameSubnetPeers` sockets from one /24 (IPv4) or /48 (IPv6).

use std::net::IpAddr;

/// The inbound sockets by subnet.
pub struct InboundCaps {}

impl InboundCaps {
    /// Caps of `max_same_subnet` sockets per subnet.
    pub fn new(max_same_subnet: usize) -> InboundCaps {
        let _ = max_same_subnet;
        todo!("the inbound caps")
    }

    /// Admit a socket from `ip`; false when its subnet is at the cap.
    pub fn admit(&self, ip: IpAddr) -> bool {
        let _ = ip;
        todo!("a socket admitted")
    }

    /// A socket from `ip` closed.
    pub fn release(&self, ip: IpAddr) {
        let _ = ip;
        todo!("a socket released")
    }
}
