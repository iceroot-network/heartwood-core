//! The relay client: the reference's forger client over one nes session per configured host.
//!
//! - Hosts are tried in list order. [`Client::select_host`] makes up to 10 sweeps, 250 ms apart:
//!   each goes through the hosts in order and takes the first whose session is ready; every host it
//!   passes whose session is missing or not ready gets a new session in place of the old one; the
//!   hosts after the chosen one are left as they are. No ready host after the sweeps is
//!   [`RelayError::HostNoResponse`].
//! - `getCurrentRound`, `getSlotNumber` and `syncBlockchain` select a host first; the other calls
//!   reuse the host last selected, the attestation routes included.
//! - A session is `ws://<ip>:<port>/`, with a 20 MiB receive limit, reconnects without limit with
//!   the reference's back-off, and its sockets bound to the `localAddress` option when set.
//! - Each call encodes its request with the route's codec (protobuf for the three public routes,
//!   with the headers `{port: 0, version}`; JSON for the internal ones), has its deadline
//!   ([`crate::deadlines`]), and fails as [`RelayError::Communication`] with the endpoint
//!   `<hostname>:<port><<route id>>`.
//! - `emitEvent` is sent only when some host's address text contains `127.0.0.1` or `::1` (so
//!   `127.0.0.11` does, `127.1.0.11` does not), and to the selected host; otherwise it logs
//!   "emitEvent: unable to find any local hosts".

use std::net::IpAddr;

use heartwood_p2p::transport::Dial;
use heartwood_p2p_wire::internal::{
    CurrentRound, NetworkState, StatusSignatures, StatusToSignResponse,
};
use heartwood_p2p_wire::status::ReceivedStatus;

use crate::errors::RelayError;
use crate::options::RelayHost;
use crate::seams::{AttestLink, Deadline, ForgerEvent, ForgingTransactions, RelayLink};

/// The receive limit of a session to a relay: 20 MiB, the reference client's `maxPayload`.
pub const MAX_PAYLOAD: usize = 20 * 1024 * 1024;
/// How many sweeps a host selection makes.
pub const SELECTION_SWEEPS: u32 = 10;
/// The pause between two sweeps.
pub const SWEEP_PAUSE: std::time::Duration = std::time::Duration::from_millis(250);

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
    #[expect(dead_code, reason = "the client's methods read it")]
    options: ClientOptions,
    #[expect(dead_code, reason = "the client's methods read it")]
    dial: Dial,
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
        Client { options, dial }
    }

    /// Select a host, as the reference's `selectHost` does, and give it.
    pub async fn select_host(&self) -> Result<RelayHost, RelayError> {
        todo!("selectHost")
    }

    /// The host last selected (the first host before any selection).
    pub fn selected(&self) -> RelayHost {
        todo!("the selected host")
    }
}

impl RelayLink for Client {
    async fn get_round(&self) -> Result<CurrentRound, RelayError> {
        todo!("getCurrentRound")
    }

    async fn get_slot_number(
        &self,
        timestamp: Option<i64>,
        deadline: Deadline,
    ) -> Result<i64, RelayError> {
        let _ = (timestamp, deadline);
        todo!("getSlotNumber")
    }

    async fn get_network_state(
        &self,
        log: bool,
        deadline: Deadline,
    ) -> Result<NetworkState, RelayError> {
        let _ = (log, deadline);
        todo!("getNetworkState")
    }

    async fn get_status(&self, deadline: Deadline) -> Result<ReceivedStatus, RelayError> {
        let _ = deadline;
        todo!("getStatus")
    }

    async fn get_transactions(
        &self,
        deadline: Deadline,
    ) -> Result<Option<ForgingTransactions>, RelayError> {
        let _ = deadline;
        todo!("getUnconfirmedTransactions")
    }

    async fn broadcast_block(&self, bytes: &[u8], deadline: Deadline) {
        let _ = (bytes, deadline);
        todo!("postBlock")
    }

    async fn sync_with_network(&self) {
        todo!("syncBlockchain")
    }

    async fn emit_event(&self, event: ForgerEvent) {
        let _ = event;
        todo!("emitEvent")
    }

    fn dispose(&self) {
        todo!("the client's dispose")
    }
}

impl AttestLink for Client {
    async fn status_to_sign(
        &self,
        public_keys: &[String],
    ) -> Result<StatusToSignResponse, RelayError> {
        let _ = public_keys;
        todo!("getStatusToSign")
    }

    async fn post_signatures(&self, signatures: StatusSignatures) -> Result<(), RelayError> {
        let _ = signatures;
        todo!("postStatusSignatures")
    }
}
