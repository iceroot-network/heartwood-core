//! The nes server session of one socket: the hello (protocol "2"), heartbeats every 15 s with 5 s
//! to answer, the server's pings, requests multiplexed by id and each handled on its own task, so
//! a pending long poll or AwaitBlock wait never delays another request on the socket; 499 errors
//! answered and the socket kept, every other error ending the socket with the reference's ban
//! reason; binary frames only, and a WebSocket ping or pong frame ending the socket ("Malicious
//! ping frame received", "Malicious pong frame received").

use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::net::TcpStream;
use tokio_tungstenite::WebSocketStream;

use super::plugins::Plugins;
use super::seams::SessionId;

/// The nes protocol version of the hello.
pub const PROTOCOL: &str = "2";
/// How often the server sends a heartbeat.
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(15);
/// How long the client has to answer a heartbeat.
pub const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(5);

/// How a session ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionEnd {
    /// The socket closed; whether it made an endpoint request first.
    Closed {
        /// A request reached a route.
        endpoint_request: bool,
    },
    /// The server ended the socket, with the reason the ban hammer bans with.
    Terminated {
        /// The reason.
        reason: String,
    },
}

/// What a session is made of.
pub struct SessionParts {
    /// The session's number.
    pub id: SessionId,
    /// The caller's address, normalised.
    pub ip: IpAddr,
    /// The plugin chain and the handlers.
    pub plugins: Arc<Plugins>,
}

/// The nes server session of one socket.
pub struct ServerSession {
    parts: SessionParts,
}

impl ServerSession {
    /// A session of `parts`.
    pub fn new(parts: SessionParts) -> ServerSession {
        ServerSession { parts }
    }

    /// Serve `socket` until it closes or the session ends it.
    pub async fn run(self, socket: WebSocketStream<TcpStream>) -> SessionEnd {
        let _ = (&self.parts, socket);
        todo!("the nes server session")
    }
}
