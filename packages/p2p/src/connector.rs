//! The connector: one nes session per peer IP, opened only when a request needs it.
//!
//! The IP is the key and the port comes from the peer, so a peer's later port does not open a
//! second session. A new session to an IP less than 10 s after the previous one was created waits
//! for the rest of the 10 s. A session that reports an error is terminated and forgotten; one
//! that closed for good stays until the peer is disconnected, and its requests fail at once. The
//! class of the last error of a request is recorded per IP.

use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::Duration;

use heartwood_p2p_wire::RouteId;
use indexmap::IndexMap;
use tokio::time::Instant;

use crate::errors::SessionError;
use crate::nes::{ConnectOptions, ErrorHook, Session};
use crate::seams::PeerTransport;
use crate::transport::{self, Dial};

/// The pause between the creation of two sessions to one IP.
pub const RECONNECT_PAUSE: Duration = Duration::from_secs(10);

/// The sessions, the last errors and the last creation times, by IP.
#[derive(Clone)]
pub struct Connector {
    options: ConnectOptions,
    dial: Dial,
    shared: Arc<Shared>,
}

#[derive(Debug, Default)]
struct Shared {
    /// Each session with the serial number it was created with.
    sessions: Mutex<IndexMap<IpAddr, (u64, Session)>>,
    errors: Mutex<IndexMap<IpAddr, &'static str>>,
    created: Mutex<IndexMap<IpAddr, Instant>>,
    serial: AtomicU64,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Default for Connector {
    fn default() -> Connector {
        Connector::with_dial(transport::tcp())
    }
}

impl std::fmt::Debug for Connector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Connector")
            .field("shared", &self.shared)
            .finish_non_exhaustive()
    }
}

impl Connector {
    /// A connector with no session, connecting over TCP with the reference's options (5 s, one
    /// reconnect).
    pub fn new() -> Connector {
        Connector::default()
    }

    /// A connector for a node with `addressing`: in mode B every session's socket binds the
    /// node's own address before it connects, so that the peers see it as the source.
    pub fn for_addressing(addressing: &crate::devnet::Addressing) -> Connector {
        match addressing.bind_address() {
            Some(local) => Connector::with_dial(transport::tcp_bound(local)),
            None => Connector::new(),
        }
    }

    /// A connector whose sessions run on streams from `dial` (a test seam: the mock peer's
    /// in-memory connections, which tokio's paused clock can drive).
    pub fn with_dial(dial: Dial) -> Connector {
        let (options, shared) = (ConnectOptions::default(), Arc::default());
        Connector {
            options,
            dial,
            shared,
        }
    }

    /// Send `payload` on `route` to `address` and wait for the response's payload, opening a
    /// session to its IP first when there is none. The session's receive limit becomes
    /// `max_payload` and its request timeout `timeout` (a zero keeps the setting).
    pub async fn emit(
        &self,
        address: SocketAddr,
        route: RouteId,
        payload: Vec<u8>,
        timeout: Duration,
        max_payload: usize,
    ) -> Result<Vec<u8>, SessionError> {
        let session = self.connect(address).await?;
        session.request(route, payload, timeout, max_payload).await
    }

    /// The IP's session, created and connected when there is none. A session being created is
    /// returned as it is, so a request made before it has connected fails at once, as the
    /// reference's does.
    async fn connect(&self, address: SocketAddr) -> Result<Session, SessionError> {
        let ip = address.ip();
        if let Some((_, session)) = lock(&self.shared.sessions).get(&ip) {
            return Ok(session.clone());
        }
        let since = lock(&self.shared.created).get(&ip).map(Instant::elapsed);
        if let Some(wait) = since.and_then(|since| RECONNECT_PAUSE.checked_sub(since)) {
            tokio::time::sleep(wait).await;
        }
        let (session, connected) = {
            let mut sessions = lock(&self.shared.sessions);
            if let Some((_, session)) = sessions.get(&ip) {
                return Ok(session.clone());
            }
            let serial = self.shared.serial.fetch_add(1, Ordering::Relaxed);
            let hook = hook(Arc::downgrade(&self.shared), ip, serial);
            let (session, connected) =
                Session::connect(address, self.options, hook, self.dial.clone());
            sessions.insert(ip, (serial, session.clone()));
            lock(&self.shared.created).insert(ip, Instant::now());
            (session, connected)
        };
        connected.await?;
        Ok(session)
    }

    /// Terminate and forget the session to `ip`.
    pub fn disconnect(&self, ip: IpAddr) {
        self.shared.remove(ip, None, None);
    }

    /// The class of the last recorded error of `ip`.
    pub fn last_error(&self, ip: IpAddr) -> Option<&'static str> {
        lock(&self.shared.errors).get(&ip).copied()
    }

    /// Record `name` as the last error of `ip`.
    pub fn set_error(&self, ip: IpAddr, name: &'static str) {
        lock(&self.shared.errors).insert(ip, name);
    }

    /// Forget the last error of `ip`.
    pub fn forget_error(&self, ip: IpAddr) {
        lock(&self.shared.errors).shift_remove(&ip);
    }
}

impl Shared {
    /// Terminate and forget the session to `ip`, only if it is the one numbered `serial` when
    /// given, failing its pending requests with `error` when given; creation times older than the
    /// pause are forgotten for IPs without a session.
    fn remove(&self, ip: IpAddr, serial: Option<u64>, error: Option<SessionError>) {
        let removed = {
            let mut sessions = lock(&self.sessions);
            let current = sessions.get(&ip).map(|(number, _)| *number);
            match current {
                Some(number) if serial.is_none_or(|serial| serial == number) => {
                    sessions.shift_remove(&ip)
                }
                _ => None,
            }
        };
        if let Some((_, session)) = removed {
            match error {
                Some(error) => session.terminate_for(error),
                None => session.terminate(),
            }
        }
        let sessions = lock(&self.sessions);
        lock(&self.created)
            .retain(|ip, at| sessions.contains_key(ip) || at.elapsed() < RECONNECT_PAUSE);
    }
}

/// The error hook of session `serial` to `ip`: the connector terminates and forgets it. After a
/// protocol error the requests pending on it fail with that error, so the communicator can tell a
/// violation from what an honest server causes (a late response, a ping flood); the reference
/// fails them as disconnected, and both are of the class `Error`.
fn hook(shared: Weak<Shared>, ip: IpAddr, serial: u64) -> ErrorHook {
    Arc::new(move |error| {
        if let Some(shared) = shared.upgrade() {
            let protocol = matches!(error, SessionError::Protocol(_)).then_some(error);
            shared.remove(ip, Some(serial), protocol);
        }
    })
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
