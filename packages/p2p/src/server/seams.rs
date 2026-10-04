//! The seam between the server's transport and its controllers: the transport's plugin chain calls
//! [`RouteHandlers`] after the pre-handler step, and reports each socket's open and close through
//! [`SessionEvents`], which the forger's attestation session listens to. A request running on its
//! own task reads its socket's close through [`SocketClosed`].

use std::net::IpAddr;
use std::sync::OnceLock;

use futures_util::future::BoxFuture;
use heartwood_p2p_wire::RouteId;
use tokio::sync::watch;

/// A socket's session, numbered by the server from 1 in the order the sockets opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SessionId(pub u64);

/// A request that passed the plugins before the handler.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerRequest {
    /// The request's payload as it came, which the route's codec decoded and the validation
    /// accepted.
    pub payload: Vec<u8>,
    /// The caller's address, normalised (an IPv4-mapped IPv6 address as its IPv4 address).
    pub ip: IpAddr,
    /// The socket's session.
    pub session: SessionId,
    /// Whether the caller is in `remoteAccess`: the node's forger.
    pub from_forger: bool,
}

/// A refused request: the status code and the message the reference's server answers. A code
/// below 499 ends the socket and bans its address with the message as the reason (the forger's
/// addresses excepted); 499 is answered and the socket kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerRefusal {
    /// The status code.
    pub status: u16,
    /// The message.
    pub message: String,
}

impl ServerRefusal {
    /// A refusal with `status` and `message`.
    pub fn new(status: u16, message: impl Into<String>) -> ServerRefusal {
        ServerRefusal {
            status,
            message: message.into(),
        }
    }

    /// 499 "App is not ready", before the blockchain has started.
    pub fn not_ready() -> ServerRefusal {
        ServerRefusal::new(499, "App is not ready")
    }
}

/// The route handlers: the controllers implement them, the transport calls them.
pub trait RouteHandlers: Send + Sync {
    /// Handle `request` on `route`; the response's payload, encoded with the route's codec.
    fn handle(
        &self,
        route: RouteId,
        request: ServerRequest,
    ) -> BoxFuture<'_, Result<Vec<u8>, ServerRefusal>>;
}

/// A request's view of its socket's close: the session fires it as soon as it stops serving the
/// socket, whatever the reason; a request that runs on reads it (AwaitBlock's record).
#[derive(Debug, Clone)]
pub struct SocketClosed(watch::Receiver<bool>);

impl SocketClosed {
    /// A new signal: the sender fires it by sending `true`, or by being dropped.
    pub fn channel() -> (watch::Sender<bool>, SocketClosed) {
        let (sender, receiver) = watch::channel(false);
        (sender, SocketClosed(receiver))
    }

    /// A signal that never fires, for a request without a socket.
    pub fn never() -> SocketClosed {
        static NEVER: OnceLock<watch::Sender<bool>> = OnceLock::new();
        SocketClosed(NEVER.get_or_init(|| watch::channel(false).0).subscribe())
    }

    /// Resolve once the signal fires.
    pub async fn closed(&mut self) {
        let _ = self.0.wait_for(|closed| *closed).await;
    }
}

/// The sockets' opens and closes, as the transport reports them.
pub trait SessionEvents: Send + Sync {
    /// The socket of `session` from `ip` opened.
    fn opened(&self, session: SessionId, ip: IpAddr);

    /// The socket of `session` from `ip` closed.
    fn closed(&self, session: SessionId, ip: IpAddr);
}
