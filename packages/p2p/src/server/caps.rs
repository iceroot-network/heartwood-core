//! The inbound caps: at most `maxSameSubnetPeers` addresses of one /24 (IPv4) or /48 (IPv6) hold
//! sockets at once. An address that already holds one may open another (it replaces the first,
//! one socket per address), so the cap counts addresses, not sockets. The reference has no such
//! cap; a network whose honest nodes never put 256 addresses in one subnet sees no difference.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Mutex, PoisonError};

/// The subnet of an address: its /24 (IPv4, an IPv4-mapped address as IPv4) or its /48.
fn subnet(ip: IpAddr) -> IpAddr {
    match ip.to_canonical() {
        IpAddr::V4(v4) => {
            let [a, b, c, _] = v4.octets();
            IpAddr::from([a, b, c, 0])
        }
        IpAddr::V6(v6) => {
            let [a, b, c, ..] = v6.segments();
            IpAddr::from([a, b, c, 0, 0, 0, 0, 0])
        }
    }
}

/// The inbound sockets by subnet and address.
#[derive(Debug)]
pub struct InboundCaps {
    max_same_subnet: usize,
    sockets: Mutex<HashMap<IpAddr, HashMap<IpAddr, usize>>>,
}

impl InboundCaps {
    /// Caps of `max_same_subnet` addresses per subnet.
    pub fn new(max_same_subnet: usize) -> InboundCaps {
        let sockets = Mutex::default();
        InboundCaps {
            max_same_subnet,
            sockets,
        }
    }

    /// Admit a socket from `ip`; false when its subnet has the most addresses already and `ip` is
    /// not among them.
    pub fn admit(&self, ip: IpAddr) -> bool {
        let mut sockets = self.sockets.lock().unwrap_or_else(PoisonError::into_inner);
        let addresses = sockets.entry(subnet(ip)).or_default();
        if !addresses.contains_key(&ip) && addresses.len() >= self.max_same_subnet {
            return false;
        }
        let count = addresses.entry(ip).or_default();
        *count = count.saturating_add(1);
        true
    }

    /// A socket from `ip` that was admitted closed.
    pub fn release(&self, ip: IpAddr) {
        let mut sockets = self.sockets.lock().unwrap_or_else(PoisonError::into_inner);
        let key = subnet(ip);
        let Some(addresses) = sockets.get_mut(&key) else {
            return;
        };
        if let Some(count) = addresses.get_mut(&ip) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                addresses.remove(&ip);
            }
        }
        if addresses.is_empty() {
            sockets.remove(&key);
        }
    }
}
