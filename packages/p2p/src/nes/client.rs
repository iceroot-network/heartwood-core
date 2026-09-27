//! The nes client session.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use heartwood_p2p_wire::RouteId;

use crate::errors::SessionError;

/// The client's receive limit when no request raises it: 100 KiB.
pub const DEFAULT_MAX_PAYLOAD: usize = 100 * 1024;

/// How a session connects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConnectOptions {
    /// How long the connection and its hello may take.
    pub timeout: Duration,
    /// How many automatic reconnects follow a close.
    pub retries: u32,
}

impl Default for ConnectOptions {
    /// The connector's options: 5 s and one reconnect.
    fn default() -> ConnectOptions {
        ConnectOptions {
            timeout: Duration::from_secs(5),
            retries: 1,
        }
    }
}

/// Called with the error that ends a session: a protocol error, a heartbeat timeout, a failed
/// WebSocket (the reference's `onError`). The connector then terminates and forgets the session.
pub type ErrorHook = Arc<dyn Fn(SessionError) + Send + Sync>;

/// One nes session over one WebSocket connection: the id counter, the pending requests, the
/// connection-wide timeout and receive limit, and the heartbeat.
#[derive(Debug)]
pub struct Session {
    #[expect(dead_code, reason = "the session's methods read it")]
    address: SocketAddr,
}

impl Session {
    /// Open a session to `address`: the WebSocket, then the hello. `on_error` is told of the error
    /// that ends the session later.
    pub async fn connect(
        address: SocketAddr,
        options: ConnectOptions,
        on_error: ErrorHook,
    ) -> Result<Session, SessionError> {
        let _ = (address, options, on_error);
        todo!("open the WebSocket and send the hello")
    }

    /// Send a request on `route` with `payload` and wait for its response's payload. The
    /// connection's timeout becomes `timeout` and its receive limit `max_payload` before the
    /// request is sent.
    pub async fn request(
        &self,
        route: RouteId,
        payload: Vec<u8>,
        timeout: Duration,
        max_payload: usize,
    ) -> Result<Vec<u8>, SessionError> {
        let _ = (route, payload, timeout, max_payload);
        todo!("send the request and wait for its response")
    }

    /// Close the connection at once; every pending request fails.
    pub fn terminate(&self) {
        todo!("terminate the connection")
    }
}
