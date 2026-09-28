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

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use heartwood_kernel::{Clock, SystemClock};
use thiserror::Error;
use tokio::net::TcpListener;
use tokio::sync::{mpsc, watch};
use tokio::time::Instant;

use self::ban_hammer::BanHammer;
use self::caps::InboundCaps;
use self::listener::Upgrade;
use self::plugins::{PluginParts, Plugins};
use self::seams::{RouteHandlers, SessionEvents, SessionId};
use self::session::{Control, ServerSession, SessionEnd, SessionParts};
use crate::devnet::Addressing;
use crate::node_services::{NodeServices, PeerSide};
use crate::options::P2pOptions;
use crate::transport::Stream;

/// How long AwaitBlock waits for the queue's drain at most, before it serves from the published
/// chain.
pub const AWAIT_BLOCK_BOUND: Duration = Duration::from_secs(8);
/// How long getStatus waits for the forger's signatures of a changed status before it serves the
/// header unsigned.
pub const ATTESTATION_WAIT: Duration = Duration::from_millis(500);
/// How long a stopping server waits for its sockets to close.
pub const STOP_TIMEOUT: Duration = Duration::from_secs(5);

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
    /// The most transactions a postTransactions request carries (the pool's
    /// `maxTransactionsPerRequest`).
    pub max_transactions_per_request: usize,
    /// The most addresses of one /24 or /48 with inbound sockets (`maxSameSubnetPeers`).
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
            max_transactions_per_request: options.max_transactions_per_request,
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
    listener: Mutex<Option<TcpListener>>,
    address: SocketAddr,
    shared: Arc<Shared>,
}

/// What every connection's task shares.
struct Shared {
    plugins: Arc<Plugins>,
    ban_hammer: Arc<BanHammer>,
    caps: InboundCaps,
    node: Arc<dyn NodeServices>,
    peers: Arc<dyn PeerSide>,
    events: Arc<dyn SessionEvents>,
    clock: Arc<dyn Clock>,
    beats_from: Instant,
    serial: AtomicU64,
    /// Each open socket's address and its session's orders.
    sessions: Mutex<HashMap<SessionId, (IpAddr, mpsc::UnboundedSender<Control>)>>,
}

impl P2pServer {
    /// Bind the listener of `parts`; a bind failure is the error.
    pub async fn bind(parts: ServerParts) -> Result<P2pServer, ServerError> {
        P2pServer::bind_with_clock(parts, Arc::new(SystemClock)).await
    }

    /// [`P2pServer::bind`], with the bans timed by `clock`.
    pub async fn bind_with_clock(
        parts: ServerParts,
        clock: Arc<dyn Clock>,
    ) -> Result<P2pServer, ServerError> {
        let address = SocketAddr::new(parts.options.host, parts.options.port);
        let bind = |source| ServerError::Bind { address, source };
        let listener = TcpListener::bind(address).await.map_err(bind)?;
        let address = listener.local_addr().map_err(bind)?;
        let options = Arc::new(parts.options);
        let ban_hammer = Arc::new(BanHammer::new(
            options.ban_seconds,
            options.remote_access.clone(),
        ));
        let caps = InboundCaps::new(options.max_same_subnet_peers);
        let plugins = Arc::new(Plugins::new(PluginParts {
            options,
            node: Arc::clone(&parts.node),
            peers: Arc::clone(&parts.peers),
            handlers: parts.handlers,
            ban_hammer: Arc::clone(&ban_hammer),
            clock: Arc::clone(&clock),
        }));
        let shared = Arc::new(Shared {
            plugins,
            ban_hammer,
            caps,
            node: parts.node,
            peers: parts.peers,
            events: parts.events,
            clock,
            beats_from: Instant::now(),
            serial: AtomicU64::new(0),
            sessions: Mutex::default(),
        });
        let listener = Mutex::new(Some(listener));
        Ok(P2pServer {
            listener,
            address,
            shared,
        })
    }

    /// The address the listener is bound to.
    pub fn local_addr(&self) -> SocketAddr {
        self.address
    }

    /// Accept and serve connections until `shutdown` turns true, then close every socket and
    /// wait for them, at most [`STOP_TIMEOUT`]. The listener closes when this returns.
    pub async fn serve(&self, mut shutdown: watch::Receiver<bool>) {
        let listener = lock(&self.listener).take();
        if let Some(listener) = listener {
            loop {
                tokio::select! {
                    // The borrow the wait returns is dropped at once, so the future stays Send.
                    () = async { drop(shutdown.wait_for(|stop| *stop).await) } => break,
                    accepted = listener.accept() => match accepted {
                        Ok((stream, peer)) => {
                            let _ = stream.set_nodelay(true);
                            self.accept(Box::new(stream), peer.ip());
                        }
                        // Out of descriptors, say: try again shortly rather than at once.
                        Err(_) => tokio::time::sleep(Duration::from_millis(50)).await,
                    },
                }
            }
        }
        self.close_where(|_| true);
        let _ = tokio::time::timeout(STOP_TIMEOUT, async {
            while !lock(&self.shared.sessions).is_empty() {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await;
    }

    /// Serve one connection from `ip` on `stream`, as the listener serves each it accepts (tests
    /// hand it in-memory streams).
    pub fn accept(&self, stream: Box<dyn Stream>, ip: IpAddr) {
        tokio::spawn(Arc::clone(&self.shared).connection(stream, ip.to_canonical()));
    }

    /// The reason `ip` was first banned with, while its ban is listed.
    pub fn ban_reason(&self, ip: IpAddr) -> Option<String> {
        self.shared.ban_hammer.reason(ip.to_canonical())
    }

    /// Close the forger's sessions (the sockets from `remoteAccess` addresses): the first step of
    /// a graceful shutdown.
    pub fn close_forger_sessions(&self) {
        let plugins = &self.shared.plugins;
        self.close_where(|ip| plugins.is_forger(ip));
    }

    fn close_where(&self, close: impl Fn(IpAddr) -> bool) {
        for (ip, control) in lock(&self.shared.sessions).values() {
            if close(*ip) {
                let _ = control.send(Control::Close);
            }
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Shared {
    /// One connection: the bans, the caps and one socket per address, the upgrade, the session,
    /// and the bans when it closes.
    async fn connection(self: Arc<Self>, stream: Box<dyn Stream>, ip: IpAddr) {
        if self.ban_hammer.is_banned(ip, self.clock.now_ms()) {
            return;
        }
        if !self.caps.admit(ip) {
            heartwood_kernel::debug!("Refused a connection from {ip}: its subnet is at its cap");
            return;
        }
        let id = SessionId(
            self.serial
                .fetch_add(1, Ordering::Relaxed)
                .saturating_add(1),
        );
        let (orders, mut control) = mpsc::unbounded_channel();
        lock(&self.sessions).insert(id, (ip, orders));
        for other in self.ban_hammer.opened(ip, id) {
            if let Some((_, orders)) = lock(&self.sessions).get(&other) {
                let _ = orders.send(Control::Destroy);
            }
        }
        let upgraded = tokio::select! {
            upgraded = listener::upgrade(stream) => upgraded,
            _ = control.recv() => Upgrade::Failed,
        };
        let end = match upgraded {
            Upgrade::Open(reader, writer) => {
                self.events.opened(id, ip);
                let plugins = Arc::clone(&self.plugins);
                let parts = SessionParts {
                    id,
                    ip,
                    plugins,
                    beats_from: self.beats_from,
                    control,
                };
                let end = ServerSession::new(parts).run(reader, writer).await;
                self.events.closed(id, ip);
                self.peers.mark_stale(ip);
                end
            }
            Upgrade::NotWebSocket => {
                let reason = "Not a websocket connection";
                self.ban_hammer.ban(ip, reason, self.clock.now_ms());
                SessionEnd::Handshake
            }
            Upgrade::Failed => SessionEnd::Handshake,
        };
        lock(&self.sessions).remove(&id);
        self.caps.release(ip);
        let (now, synced) = (self.clock.now_ms(), self.node.first_synced_ms());
        self.ban_hammer.closed(ip, id, &end, now, synced);
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
