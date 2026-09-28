//! The peer-to-peer server, as the reference's socket server and its nes plugin serve the peer
//! routes and the forger's internal routes.
//!
//! - [`listener`]: the TCP listener, the WebSocket upgrade on `/`, the socket timeout and the
//!   message limit; [`session`]: the nes server session of one socket.
//! - [`ban_hammer`]: the bans by address and one socket per address; [`caps`]: the inbound caps
//!   per subnet.
//! - [`plugins`]: the plugin chain in the reference's order, with [`rate_limit`] (the inbound
//!   limiter), [`await_block`] (the bounded wait for the queue's drain) and [`validation`] (the
//!   request checks).
//! - [`controllers`]: the route handlers; [`seams`]: the seam between the transport and the
//!   controllers.
//!
//! The server reaches the node only through [`crate::node_services`].

pub mod await_block;
pub mod ban_hammer;
pub mod caps;
pub mod controllers;
pub mod listener;
pub mod plugins;
pub mod rate_limit;
pub mod seams;
pub mod session;
pub mod validation;

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use thiserror::Error;
use tokio::sync::watch;

use self::seams::{RouteHandlers, SessionEvents};
use crate::devnet::Addressing;
use crate::node_services::{NodeServices, PeerSide};
use crate::options::P2pOptions;

/// How long AwaitBlock waits for the queue's drain at most, before it serves from the published
/// chain.
pub const AWAIT_BLOCK_BOUND: Duration = Duration::from_secs(8);
/// How long getStatus waits for the forger's signatures of a changed status before it serves the
/// header unsigned.
pub const ATTESTATION_WAIT: Duration = Duration::from_millis(500);

/// The server's options.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerOptions {
    /// The address the listener binds (`server.hostname`).
    pub host: IpAddr,
    /// The port (`server.port`).
    pub port: u16,
    /// How long a ban lasts, in seconds (`server.banSeconds`).
    pub ban_seconds: u32,
    /// The forger's addresses (`remoteAccess`), exact and normalised; empty admits no forger.
    pub remote_access: Vec<IpAddr>,
    /// The version ranges a caller's version must satisfy when the milestone names none
    /// (`minimumVersions`).
    pub minimum_versions: Vec<String>,
    /// The requests per second from one address (`rateLimit`).
    pub rate_limit: u32,
    /// The postTransactions per second from one address (`rateLimitPostTransactions`).
    pub rate_limit_post_transactions: u32,
    /// The most inbound sockets from one /24 or /48 (`maxSameSubnetPeers`).
    pub max_same_subnet_peers: usize,
    /// The longest AwaitBlock wait.
    pub await_block_bound: Duration,
    /// The longest wait for the forger's signatures of a changed status.
    pub attestation_wait: Duration,
    /// How a devnet node takes its address.
    pub addressing: Addressing,
}

impl ServerOptions {
    /// The server's options of `options`. The exact `remoteAccess` entries are kept, normalised
    /// (an IPv4-mapped IPv6 address as its IPv4 address); an entry with a prefix length matches
    /// no source, as the reference's match of its text matches none, and is left out.
    pub fn from_p2p(options: &P2pOptions) -> ServerOptions {
        let exact = |entry: &String| entry.parse::<IpAddr>().ok();
        let remote_access = options.remote_access.iter().filter_map(exact);
        ServerOptions {
            host: options.host,
            port: options.port,
            ban_seconds: options.ban_seconds,
            remote_access: remote_access.map(|ip| ip.to_canonical()).collect(),
            minimum_versions: options.minimum_versions.clone(),
            rate_limit: options.rate_limit,
            rate_limit_post_transactions: options.rate_limit_post_transactions,
            max_same_subnet_peers: options.max_same_subnet_peers,
            await_block_bound: AWAIT_BLOCK_BOUND,
            attestation_wait: ATTESTATION_WAIT,
            addressing: options.addressing,
        }
    }
}

/// The server could not start.
#[derive(Debug, Error)]
pub enum ServerError {
    /// The listener could not bind its address.
    #[error("the P2P server cannot listen on {address}: {source}")]
    Bind {
        /// The address.
        address: SocketAddr,
        /// Why.
        source: std::io::Error,
    },
}

/// What the server is made of.
pub struct ServerParts {
    /// The options.
    pub options: ServerOptions,
    /// The node.
    pub node: Arc<dyn NodeServices>,
    /// The peer side.
    pub peers: Arc<dyn PeerSide>,
    /// The route handlers.
    pub handlers: Arc<dyn RouteHandlers>,
    /// Who hears of the sockets' opens and closes.
    pub events: Arc<dyn SessionEvents>,
}

/// The server: its listener, the sessions by address, the ban list, the limiter and the caps.
pub struct P2pServer {
    parts: ServerParts,
}

impl P2pServer {
    /// Bind the listener of `parts`; a bind failure is the error.
    pub async fn bind(parts: ServerParts) -> Result<P2pServer, ServerError> {
        let _ = parts;
        todo!("the listener's bind")
    }

    /// The address the listener is bound to.
    pub fn local_addr(&self) -> SocketAddr {
        let _ = &self.parts;
        todo!("the listener's address")
    }

    /// Accept and serve connections until `shutdown` turns true, then close every socket.
    pub async fn serve(&self, shutdown: watch::Receiver<bool>) {
        let _ = (&self.parts, shutdown);
        todo!("the accept loop")
    }

    /// Close the forger's sessions (the sockets from `remoteAccess` addresses): the first step of
    /// a graceful shutdown.
    pub fn close_forger_sessions(&self) {
        let _ = &self.parts;
        todo!("the forger's sessions closed")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_forger_addresses_are_exact_and_normalised() {
        let p2p = P2pOptions {
            remote_access: ["127.0.0.1", "::ffff:127.0.0.1", "::1", "10.0.0.0/8"]
                .map(str::to_owned)
                .to_vec(),
            ..P2pOptions::default()
        };
        let options = ServerOptions::from_p2p(&p2p);
        let expected: Vec<IpAddr> = ["127.0.0.1", "127.0.0.1", "::1"]
            .iter()
            .map(|ip| ip.parse().unwrap())
            .collect();
        assert_eq!(options.remote_access, expected);
        assert_eq!((options.port, options.ban_seconds), (4002, 10));
        assert_eq!(options.await_block_bound, Duration::from_secs(8));
    }
}
