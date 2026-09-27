//! The relay client: the reference's forger client over one nes session per configured host.
//!
//! - Hosts are tried in list order. [`Client::select_host`] makes up to 10 sweeps, 250 ms apart:
//!   each goes through the hosts in order and takes the first whose session is ready; every host it
//!   passes whose session is missing or not ready gets a new session in place of the old one; the
//!   hosts after the chosen one are left as they are. No ready host after the sweeps is
//!   [`RelayError::HostNoResponse`].
//! - `getCurrentRound`, `getSlotNumber` and `syncBlockchain` select a host first; the other calls
//!   reuse the host last selected, the attestation routes included, so the attester follows the
//!   selection.
//! - A session is `ws://<ip>:<port>/`, with a 20 MiB receive limit, reconnects without limit with
//!   the reference's back-off, and its sockets bound to the `localAddress` option when set. Its
//!   errors are logged as the reference's `onError` logs them; a replaced session is silenced
//!   first.
//! - Each call encodes its request with the route's codec (protobuf for the three public routes,
//!   with the headers `{port: 0, version}`; JSON for the internal ones), has its deadline
//!   ([`crate::deadlines`]), and fails as [`RelayError::Communication`] with the endpoint
//!   `<hostname>:<port><<route id>>` and the nes error's message.
//! - `emitEvent` is sent only when some host's address text contains `127.0.0.1` or `::1` (so
//!   `127.0.0.11` does, `127.1.0.11` does not), and to the selected host; otherwise it logs
//!   "emitEvent: unable to find any local hosts" when it is called.

use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use heartwood_kernel::{debug, error, warning};
use heartwood_p2p::errors::SessionError;
use heartwood_p2p::nes::{ConnectOptions, ErrorHook, Session};
use heartwood_p2p::transport::Dial;
use heartwood_p2p_wire::codecs::{blocks, transactions};
use heartwood_p2p_wire::internal::{
    self, CurrentRound, EmitEvent, InternalBody, NetworkState, NetworkStateRequest, SlotNumber,
    SlotNumberRequest, StatusSignatures, StatusToSignRequest, StatusToSignResponse,
};
use heartwood_p2p_wire::requests::{ClientRequest, Headers};
use heartwood_p2p_wire::status::{self, ReceivedStatus};
use heartwood_p2p_wire::{RequestError, RouteId};

use crate::deadlines;
use crate::errors::RelayError;
use crate::options::{DEFAULT_PORT, RelayHost};
use crate::seams::{AttestLink, Deadline, ForgerEvent, ForgingTransactions, RelayLink};

/// The receive limit of a session to a relay: 20 MiB, the reference client's `maxPayload`.
pub const MAX_PAYLOAD: usize = 20 * 1024 * 1024;
/// How many sweeps a host selection makes.
pub const SELECTION_SWEEPS: u32 = 10;
/// The pause between two sweeps.
pub const SWEEP_PAUSE: Duration = Duration::from_millis(250);
/// How long a session's WebSocket may take to open; a hung attempt is replaced by the next sweep.
const OPEN_TIMEOUT: Duration = Duration::from_secs(5);
/// The session error the reference's forger does not log.
const QUIET_ERROR: &str = "Connection terminated while waiting to connect";

/// What a client connects to, and how it introduces itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientOptions {
    /// The relays, in selection order.
    pub hosts: Vec<RelayHost>,
    /// The address the sessions' sockets are bound to.
    pub local_address: Option<IpAddr>,
    /// The node's version, which the forger's public calls carry in their headers.
    pub version: String,
}

/// The relay client.
pub struct Client {
    options: ClientOptions,
    dial: Dial,
    /// `{port: 0, version}`; an empty version fails every public call.
    headers: Result<Headers, RequestError>,
    /// One session per host, in the hosts' order, once a selection has passed the host.
    sessions: Mutex<Vec<Option<Link>>>,
    selected: AtomicUsize,
    disposed: AtomicBool,
}

/// A host's session, and the switch that silences its errors once it is replaced.
struct Link {
    session: Session,
    quiet: Arc<AtomicBool>,
}

impl Client {
    /// A client whose sessions connect over TCP, from the `localAddress` option when it is set.
    pub fn new(options: ClientOptions) -> Client {
        let dial = match options.local_address {
            Some(local) => heartwood_p2p::transport::tcp_bound(local),
            None => heartwood_p2p::transport::tcp(),
        };
        Client::with_dial(options, dial)
    }

    /// A client whose sessions open their streams with `dial` (the mock relay's in tests).
    pub fn with_dial(options: ClientOptions, dial: Dial) -> Client {
        let sessions = Mutex::new(options.hosts.iter().map(|_| None).collect());
        Client {
            headers: Headers::new(options.version.clone(), 0),
            options,
            dial,
            sessions,
            selected: AtomicUsize::new(0),
            disposed: AtomicBool::new(false),
        }
    }

    /// Select a host, as the reference's `selectHost` does, and give it.
    pub async fn select_host(&self) -> Result<RelayHost, RelayError> {
        let hosts = &self.options.hosts;
        for _ in 0..SELECTION_SWEEPS {
            if self.disposed.load(Ordering::Acquire) {
                break;
            }
            for (index, host) in hosts.iter().enumerate() {
                let ready = self
                    .session(index)
                    .is_some_and(|session| session.is_ready());
                if ready {
                    self.selected.store(index, Ordering::Release);
                    return Ok(*host);
                }
                let link = self.connect(host);
                if let Some(old) = self.sessions().get_mut(index).and_then(|s| s.replace(link)) {
                    old.quiet.store(true, Ordering::Release);
                    old.session.terminate();
                }
            }
            tokio::time::sleep(SWEEP_PAUSE).await;
        }
        let listed: Vec<String> = (hosts.iter())
            .map(|host| format!("{}:{}", host.hostname, host.port))
            .collect();
        let listed = serde_json::to_string(&listed).unwrap_or_default();
        debug!("No open socket connection to any host: {listed} :bangbang:");
        Err(RelayError::host_no_response(hosts))
    }

    /// The host last selected (the first host before any selection).
    pub fn selected(&self) -> RelayHost {
        let index = self.selected.load(Ordering::Acquire);
        (self.options.hosts.get(index).copied()).unwrap_or(RelayHost {
            hostname: IpAddr::from([127, 0, 0, 1]),
            port: DEFAULT_PORT,
        })
    }

    fn sessions(&self) -> MutexGuard<'_, Vec<Option<Link>>> {
        self.sessions.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn session(&self, index: usize) -> Option<Session> {
        let sessions = self.sessions();
        sessions
            .get(index)?
            .as_ref()
            .map(|link| link.session.clone())
    }

    /// A new session to `host`, connecting in the background.
    fn connect(&self, host: &RelayHost) -> Link {
        let quiet = Arc::new(AtomicBool::new(false));
        let silenced = quiet.clone();
        let on_error: ErrorHook = Arc::new(move |failure: SessionError| {
            let message = message(&failure);
            if !silenced.load(Ordering::Acquire) && message != QUIET_ERROR {
                error!("{message} :bangbang:");
            }
        });
        let options = ConnectOptions {
            timeout: OPEN_TIMEOUT,
            retries: u32::MAX,
            max_payload: MAX_PAYLOAD,
        };
        let address = SocketAddr::new(host.hostname, host.port);
        let (session, _) = Session::connect(address, options, on_error, self.dial.clone());
        Link { session, quiet }
    }

    /// A request on the selected host's session, answered before `deadline`.
    async fn emit(
        &self,
        route: RouteId,
        payload: Vec<u8>,
        deadline: Deadline,
    ) -> Result<Vec<u8>, RelayError> {
        let index = self.selected.load(Ordering::Acquire);
        let host = self.selected();
        let failed = |message: &str| RelayError::communication(&host, route, message);
        let Some(session) = self.session(index) else {
            return Err(failed(
                r#"Expected value which is "non-null and non-undefined""#,
            ));
        };
        if !session.is_ready() {
            return Err(failed("Failed to send message - server disconnected"));
        }
        let timeout = deadline.remaining();
        if timeout.is_zero() {
            return Err(failed("Request timed out"));
        }
        let request = session.request(route, payload, timeout, MAX_PAYLOAD);
        match tokio::time::timeout_at(deadline.0, request).await {
            Ok(Ok(answer)) => Ok(answer),
            Ok(Err(failure)) => Err(failed(&message(&failure))),
            Err(_) => Err(failed("Request timed out")),
        }
    }

    /// An internal route's request, read with the answer's JSON body.
    async fn call<T: InternalBody>(
        &self,
        route: RouteId,
        request: Vec<u8>,
        deadline: Deadline,
    ) -> Result<T, RelayError> {
        let answer = self.emit(route, request, deadline).await?;
        internal::decode(&answer).map_err(|refused| self.refused(route, refused))
    }

    /// A public route's request, with the forger's headers.
    async fn public(
        &self,
        request: ClientRequest,
        deadline: Deadline,
    ) -> Result<Vec<u8>, RelayError> {
        let route = request.route();
        let headers = (self.headers.as_ref()).map_err(|refused| self.refused(route, refused))?;
        self.emit(route, request.encode(headers), deadline).await
    }

    fn refused(&self, route: RouteId, error: impl std::fmt::Display) -> RelayError {
        RelayError::communication(&self.selected(), route, error.to_string())
    }

    /// A call whose answer is read and ignored, as the reference's codec parses it.
    async fn ignored(&self, route: RouteId, request: Vec<u8>) -> Result<(), RelayError> {
        let deadline = deadlines::deadline(route, Duration::ZERO);
        let answer = self.emit(route, request, deadline).await?;
        internal::parse(&answer)
            .map(drop)
            .map_err(|refused| self.refused(route, refused))
    }
}

/// A nes error's message, without the JavaScript class that its text starts with.
fn message(error: &SessionError) -> String {
    let text = error.to_string();
    let prefix = format!("{}: ", error.js_class());
    text.strip_prefix(&prefix)
        .map_or_else(|| text.clone(), str::to_owned)
}

impl RelayLink for Client {
    async fn get_round(&self) -> Result<CurrentRound, RelayError> {
        self.select_host().await?;
        let deadline = deadlines::deadline(RouteId::GetCurrentRound, Duration::ZERO);
        self.call(RouteId::GetCurrentRound, b"{}".to_vec(), deadline)
            .await
    }

    async fn get_slot_number(
        &self,
        timestamp: Option<i64>,
        deadline: Deadline,
    ) -> Result<i64, RelayError> {
        self.select_host().await?;
        let request = internal::encode(&SlotNumberRequest { timestamp });
        let slot: SlotNumber = self.call(RouteId::GetSlotNumber, request, deadline).await?;
        Ok(slot.0)
    }

    async fn get_network_state(
        &self,
        log: bool,
        deadline: Deadline,
    ) -> Result<NetworkState, RelayError> {
        let request = internal::encode(&NetworkStateRequest { log });
        self.call(RouteId::GetNetworkState, request, deadline).await
    }

    async fn get_status(&self, deadline: Deadline) -> Result<ReceivedStatus, RelayError> {
        let answer = self.public(ClientRequest::GetStatus, deadline).await?;
        status::decode_response(&answer)
            .map_err(|refused| self.refused(RouteId::GetStatus, refused))
    }

    async fn get_transactions(
        &self,
        deadline: Deadline,
    ) -> Result<Option<ForgingTransactions>, RelayError> {
        let request = ClientRequest::GetUnconfirmedTransactions {
            exclude: Vec::new(),
        };
        let answer = self.public(request, deadline).await?;
        let route = RouteId::GetUnconfirmedTransactions;
        let listed = transactions::get_unconfirmed_transactions_decode_response(&answer)
            .map_err(|refused| self.refused(route, refused))?;
        // The reference's answer always has its list, so it is never empty; a pool size the relay
        // left out is read as the default count of the reference's pluraliser, 1.
        Ok(Some(ForgingTransactions {
            pool_size: listed.message.pool_size.unwrap_or(1),
            transactions: listed.transactions,
        }))
    }

    async fn broadcast_block(&self, bytes: &[u8], deadline: Deadline) {
        let request = ClientRequest::PostBlock {
            block: bytes.to_vec(),
        };
        let posted = match self.public(request, deadline).await {
            Ok(answer) => blocks::post_block_decode_response(&answer)
                .map(drop)
                .map_err(|refused| self.refused(RouteId::PostBlock, refused)),
            Err(failure) => Err(failure),
        };
        if let Err(failure) = posted {
            error!("Broadcast block failed: {failure} :bangbang:");
        }
    }

    async fn sync_with_network(&self) {
        // The reference's selection error leaves the slot check, whose error log shows it.
        if let Err(failure) = self.select_host().await {
            return warning!("{failure}");
        }
        debug!(
            "Sending wake-up check to relay node {}",
            self.selected().hostname
        );
        if let Err(failure) = self.ignored(RouteId::SyncBlockchain, b"{}".to_vec()).await {
            error!("Could not sync check: {failure} :bangbang:");
        }
    }

    fn emit_event(&self, event: ForgerEvent) -> impl Future<Output = ()> + Send {
        let local = (self.options.hosts.iter()).find(|host| {
            let text = host.hostname.to_string();
            text.contains("127.0.0.1") || text.contains("::1")
        });
        if local.is_none() {
            error!("emitEvent: unable to find any local hosts :bangbang:");
        }
        async move {
            let Some(local) = local else {
                return;
            };
            let name = event.name();
            let body = EmitEvent {
                event: name.to_owned(),
                body: event.body(),
            };
            if self
                .ignored(RouteId::EmitEvent, internal::encode(&body))
                .await
                .is_err()
            {
                let (hostname, port) = (local.hostname, local.port);
                error!("Failed to emit \"{name}\" to \"{hostname}:{port}\" :bangbang:");
            }
        }
    }

    fn dispose(&self) {
        self.disposed.store(true, Ordering::Release);
        for link in self.sessions().iter().flatten() {
            link.quiet.store(true, Ordering::Release);
            link.session.terminate();
        }
    }
}

impl AttestLink for Client {
    async fn status_to_sign(
        &self,
        public_keys: &[String],
    ) -> Result<StatusToSignResponse, RelayError> {
        let route = RouteId::GetStatusToSign;
        let request = internal::encode(&StatusToSignRequest {
            public_keys: public_keys.to_vec(),
        });
        let deadline = deadlines::deadline(route, Duration::ZERO);
        self.call(route, request, deadline).await
    }

    async fn post_signatures(&self, signatures: StatusSignatures) -> Result<(), RelayError> {
        self.ignored(RouteId::PostStatusSignatures, internal::encode(&signatures))
            .await
    }
}
