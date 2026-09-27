//! The connector: one nes session per peer IP, opened only when a request needs it.
//!
//! A new session to an IP less than 10 s after the previous one was created waits for the rest of
//! the 10 s. A session that fails is terminated and forgotten. The class of the last error of a
//! request is recorded per IP.

use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use heartwood_p2p_wire::RouteId;
use indexmap::IndexMap;
use tokio::time::Instant;

use crate::errors::SessionError;
use crate::nes::{ConnectOptions, Session};
use crate::seams::PeerTransport;

/// The pause between the creation of two sessions to one IP.
pub const RECONNECT_PAUSE: Duration = Duration::from_secs(10);

/// The sessions, the last errors and the last creation times, by IP.
#[derive(Debug, Default)]
pub struct Connector {
    #[expect(dead_code, reason = "the connector's methods read it")]
    options: ConnectOptions,
    #[expect(dead_code, reason = "the connector's methods read it")]
    sessions: Mutex<IndexMap<IpAddr, Arc<Session>>>,
    #[expect(dead_code, reason = "the connector's methods read it")]
    errors: Mutex<IndexMap<IpAddr, &'static str>>,
    #[expect(dead_code, reason = "the connector's methods read it")]
    created: Mutex<IndexMap<IpAddr, Instant>>,
}

impl Connector {
    /// A connector with no session, connecting with the reference's options (5 s, one reconnect).
    pub fn new() -> Connector {
        Connector::default()
    }

    /// Send `payload` on `route` to `address` and wait for the response's payload, opening a
    /// session to its IP first when there is none.
    pub async fn emit(
        &self,
        address: SocketAddr,
        route: RouteId,
        payload: Vec<u8>,
        timeout: Duration,
        max_payload: usize,
    ) -> Result<Vec<u8>, SessionError> {
        let _ = (address, route, payload, timeout, max_payload);
        todo!("emit on the IP's session")
    }

    /// Terminate and forget the session to `ip`.
    pub fn disconnect(&self, ip: IpAddr) {
        let _ = ip;
        todo!("disconnect the IP")
    }

    /// The class of the last recorded error of `ip`.
    pub fn last_error(&self, ip: IpAddr) -> Option<&'static str> {
        let _ = ip;
        todo!("the IP's last error")
    }

    /// Record `name` as the last error of `ip`.
    pub fn set_error(&self, ip: IpAddr, name: &'static str) {
        let _ = (ip, name);
        todo!("record the IP's error")
    }

    /// Forget the last error of `ip`.
    pub fn forget_error(&self, ip: IpAddr) {
        let _ = ip;
        todo!("forget the IP's error")
    }
}

impl PeerTransport for Connector {
    async fn emit(
        &self,
        address: SocketAddr,
        route: RouteId,
        payload: Vec<u8>,
        timeout: Duration,
        max_payload: usize,
    ) -> Result<Vec<u8>, SessionError> {
        Connector::emit(self, address, route, payload, timeout, max_payload).await
    }

    fn disconnect(&self, ip: IpAddr) {
        Connector::disconnect(self, ip);
    }

    fn set_error(&self, ip: IpAddr, name: &'static str) {
        Connector::set_error(self, ip, name);
    }

    fn forget_error(&self, ip: IpAddr) {
        Connector::forget_error(self, ip);
    }
}
