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

/// The runtime guards of mode B at a node's start, with the node's `options` on the network named
/// `network_name`: the network is a devnet (named `devnet` or `devnet-...`), the node's address is
/// a loopback address in 127.0.0.0/8, the server listens on that address (`server.hostname`),
/// and `remoteAccess` is not empty and holds only loopback addresses. Without devnet addressing
/// there is nothing to guard. A node that passes logs a warning naming the mode.
pub fn guard(options: &crate::options::P2pOptions, network_name: &str) -> Result<(), GuardError> {
    let Addressing::Alias { address } = options.addressing else {
        return Ok(());
    };
    let refuse = |reason: String| Err(GuardError(reason));
    let devnet = network_name == "devnet" || network_name.starts_with("devnet-");
    if !devnet {
        return refuse(format!("the network {network_name} is not a devnet"));
    }
    if !is_loopback_v4(address) {
        return refuse(format!(
            "the node's address {address} is not in 127.0.0.0/8"
        ));
    }
    if options.host != address {
        let host = options.host;
        return refuse(format!(
            "the server listens on {host}, not on the node's address {address}"
        ));
    }
    let loopback = |entry: &String| {
        entry
            .parse::<IpAddr>()
            .is_ok_and(|ip| ip.to_canonical().is_loopback())
    };
    if options.remote_access.is_empty() || !options.remote_access.iter().all(loopback) {
        return refuse(
            "remoteAccess must list loopback addresses only, and at least one".to_owned(),
        );
    }
    heartwood_kernel::warning!(
        "Devnet addressing: this node is {address} and accepts peers in 127.0.0.0/8 only"
    );
    Ok(())
}

/// The runtime guard of a listener other than the peer server (`what`, on `host`) in mode B: it
/// must listen on a loopback address in 127.0.0.0/8, never on a wildcard or a routable address.
/// Without devnet addressing there is nothing to guard.
pub fn guard_listener(addressing: &Addressing, what: &str, host: IpAddr) -> Result<(), GuardError> {
    match addressing {
        Addressing::Alias { .. } if !is_loopback_v4(host) => Err(GuardError(format!(
            "{what} listens on {host}, which is not in 127.0.0.0/8"
        ))),
        _ => Ok(()),
    }
}

fn is_loopback_v4(ip: IpAddr) -> bool {
    matches!(ip.to_canonical(), IpAddr::V4(v4) if v4.is_loopback())
}

/// Whether `candidate` may become a peer in mode B: a loopback address in 127.0.0.0/8 other than
/// the node's own and the `remoteAccess` addresses. Every candidate may without devnet
/// addressing.
pub fn is_candidate(addressing: &Addressing, candidate: IpAddr, remote_access: &[IpAddr]) -> bool {
    let candidate = candidate.to_canonical();
    match addressing {
        Addressing::Off => true,
        Addressing::Alias { address } => {
            is_loopback_v4(candidate)
                && candidate != *address
                && !remote_access.contains(&candidate)
        }
    }
}
