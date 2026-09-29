//! The relay's API, read on a loopback address only.
//!
//! The address rule: the configured API host when it is a loopback address (a devnet node's API
//! listens on its own `127.0.0.x`), `127.0.0.1` or `::1` when the API binds a wildcard address,
//! and no read at all when the API is disabled or bound to a specific address that is not
//! loopback. At most [`MAX_REQUESTS`] requests a run ([`Budget`]), each step within
//! [`STEP_TIMEOUT`], no retry; an answer of 429 (the API's rate limit) gives `unknown` with that
//! reason.

use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use heartwood_kernel::Clock;

use crate::doctor::seams::{DoctorApi, RelayFacts};
use crate::doctor::sources::ParseError;
use crate::doctor::view::{Budget, Reason, Source, Unknown};

/// The most requests a run sends.
pub const MAX_REQUESTS: u32 = 12;
/// The time each step of a request may take.
pub const STEP_TIMEOUT: Duration = Duration::from_secs(1);
/// The peers read per page, and the most pages read.
pub const PEERS_PAGE: u32 = 100;
/// The most pages of peers read.
pub const PEERS_PAGES: u32 = 2;

/// `GET /api/node/status`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ApiStatus {
    /// Whether the relay considers itself synced.
    pub synced: bool,
    /// Its height.
    pub now: u32,
    /// How many blocks it is behind the network.
    pub blocks_count: i64,
}

/// `GET /api/node/syncing`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ApiSyncing {
    /// Whether it is syncing.
    pub syncing: bool,
    /// The blocks left.
    pub blocks: i64,
    /// Its height.
    pub height: u32,
}

/// `GET /api/node/configuration`: the node's version is `data.core.version` (the top-level
/// `data.version` is the network's address byte).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiConfiguration {
    /// `core.version`, as the API gives it; checks keep it only as a version id.
    pub core_version: Option<String>,
    /// `nethash`.
    pub nethash: Option<String>,
    /// The block time, in seconds, at the tip.
    pub block_time_s: Option<u32>,
    /// The active delegates at the tip.
    pub active_delegates: Option<u32>,
}

/// One peer of `GET /api/peers`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiPeer {
    /// Its address.
    pub ip: IpAddr,
    /// Its P2P port.
    pub port: u16,
    /// Its height, when it reported one.
    pub height: Option<u32>,
    /// Its version, as the peer reported it: untrusted text.
    pub version: Option<String>,
}

/// `GET /api/peers`, at most [`PEERS_PAGES`] pages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiPeers {
    /// `meta.totalCount`.
    pub total_count: u32,
    /// The peers read.
    pub peers: Vec<ApiPeer>,
}

/// A block of `GET /api/blocks/last` or `GET /api/blocks/{height}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiBlock {
    /// Its id.
    pub id: String,
    /// Its height.
    pub height: u32,
    /// Its slot's first second, in seconds since the network epoch.
    pub timestamp: Option<u32>,
    /// Its generator's public key.
    pub generator_public_key: Option<String>,
    /// Its generator's username.
    pub username: Option<String>,
}

/// A wallet of `GET /api/wallets/{publicKey}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiWallet {
    /// Its address.
    pub address: String,
    /// Its public key.
    pub public_key: Option<String>,
    /// Its delegate username, when it is a delegate.
    pub username: Option<String>,
}

/// What a read of the API needs.
pub struct ApiContext<'a> {
    /// The HTTP seam.
    pub api: &'a dyn DoctorApi,
    /// Where the API is reached ([`api_address`]).
    pub address: SocketAddr,
    /// The run's budget.
    pub budget: &'a Budget,
    /// The clock, for the run's deadline.
    pub clock: &'a dyn Clock,
}

/// Where the doctor reaches the relay's API, by the address rule.
pub fn api_address(relay: &RelayFacts) -> Result<SocketAddr, Unknown> {
    let _ = relay;
    Err(Unknown::new(Source::Api, Reason::NotBuilt))
}

/// The body of `GET path`, taking one request of the budget; 429 gives [`Reason::RateLimited`],
/// any other status but 200 [`Reason::Unreadable`] (404 is the caller's to read as "none").
pub fn fetch(context: &ApiContext<'_>, path: &str) -> Result<(u16, Vec<u8>), Unknown> {
    let _ = (context.address, path);
    Err(Unknown::new(Source::Api, Reason::NotBuilt))
}

/// `GET /api/node/status`.
pub fn read_status(context: &ApiContext<'_>) -> Result<ApiStatus, Unknown> {
    let _ = context;
    Err(Unknown::new(Source::Api, Reason::NotBuilt))
}

/// `GET /api/node/syncing`.
pub fn read_syncing(context: &ApiContext<'_>) -> Result<ApiSyncing, Unknown> {
    let _ = context;
    Err(Unknown::new(Source::Api, Reason::NotBuilt))
}

/// `GET /api/node/configuration`.
pub fn read_configuration(context: &ApiContext<'_>) -> Result<ApiConfiguration, Unknown> {
    let _ = context;
    Err(Unknown::new(Source::Api, Reason::NotBuilt))
}

/// `GET /api/peers`, at most [`PEERS_PAGES`] pages of [`PEERS_PAGE`].
pub fn read_peers(context: &ApiContext<'_>) -> Result<ApiPeers, Unknown> {
    let _ = context;
    Err(Unknown::new(Source::Api, Reason::NotBuilt))
}

/// `GET /api/blocks/last`.
pub fn read_last_block(context: &ApiContext<'_>) -> Result<ApiBlock, Unknown> {
    let _ = context;
    Err(Unknown::new(Source::Api, Reason::NotBuilt))
}

/// `GET /api/blocks/{height}`; `None` when the relay has no block there.
pub fn read_block(context: &ApiContext<'_>, height: u32) -> Result<Option<ApiBlock>, Unknown> {
    let _ = (context, height);
    Err(Unknown::new(Source::Api, Reason::NotBuilt))
}

/// `GET /api/wallets/{publicKey}`; `None` when there is no such wallet.
pub fn read_wallet(
    context: &ApiContext<'_>,
    public_key: &str,
) -> Result<Option<ApiWallet>, Unknown> {
    let _ = (context, public_key);
    Err(Unknown::new(Source::Api, Reason::NotBuilt))
}

/// The `data` of a JSON body, for the parsers.
pub fn parse_data(body: &[u8]) -> Result<serde_json::Value, ParseError> {
    let _ = body;
    Err(ParseError::NotBuilt)
}
