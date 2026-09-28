//! A devnet node's addressing: in mode B every node of a local devnet takes its own loopback
//! address, so the reference's rules per address (one socket per address, bans, the forger's
//! addresses) hold between nodes on one host.
//!
//! With [`Addressing::Alias`], peer candidates are loopback addresses in 127.0.0.0/8 (the node's
//! own address and every `remoteAccess` address refused: a forger's address is never a peer),
//! outbound sockets and the port probes bind to the node's address, and the runtime guards hold
//! the node to a devnet. [`Addressing::Off`] is every other node.

use std::net::IpAddr;

use thiserror::Error;

/// How a node takes its address.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Addressing {
    /// The reference's rules: no devnet addressing.
    #[default]
    Off,
    /// Mode B: the node's own loopback address.
    Alias {
        /// The node's address.
        address: IpAddr,
    },
}

impl Addressing {
    /// The address outbound sockets bind to, in mode B.
    pub fn bind_address(&self) -> Option<IpAddr> {
        match self {
            Addressing::Off => None,
            Addressing::Alias { address } => Some(*address),
        }
    }
}

/// A devnet guard refused the node's start.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("devnet addressing: {0}")]
pub struct GuardError(pub String);

/// The `devnet` block of the p2p options: `{"addressing": "alias", "address": <IP>}`, or
/// `{"addressing": "off"}`.
#[cfg(feature = "devnet")]
pub fn from_options(value: &serde_json::Value) -> Result<Addressing, crate::errors::OptionsError> {
    let invalid = |key: &str, reason: &str| crate::errors::OptionsError {
        key: format!("devnet.{key}"),
        reason: reason.to_owned(),
    };
    let address = value.get("address").map(|address| {
        let text = address.as_str().unwrap_or_default();
        text.parse::<IpAddr>()
            .map_err(|_| invalid("address", "must be an IP address"))
    });
    match value.get("addressing").and_then(serde_json::Value::as_str) {
        Some("off") => Ok(Addressing::Off),
        Some("alias") => match address {
            Some(address) => Ok(Addressing::Alias { address: address? }),
            None => Err(invalid("address", "is required with alias addressing")),
        },
        _ => Err(invalid("addressing", "must be alias or off")),
    }
}

/// The runtime guards of mode B at a node's start: the network is a devnet (its name is
/// `devnet`), the node's address is a loopback address in 127.0.0.0/8, and the server listens on
/// that address (`host`).
pub fn guard(addressing: &Addressing, network_name: &str, host: IpAddr) -> Result<(), GuardError> {
    let _ = (addressing, network_name, host);
    todo!("the devnet guards")
}

/// Whether `candidate` may become a peer in mode B: a loopback address in 127.0.0.0/8 other than
/// the node's own and the `remoteAccess` addresses. Every candidate may without devnet
/// addressing.
pub fn is_candidate(addressing: &Addressing, candidate: IpAddr, remote_access: &[IpAddr]) -> bool {
    let _ = (addressing, candidate, remote_access);
    todo!("the devnet candidate rule")
}
