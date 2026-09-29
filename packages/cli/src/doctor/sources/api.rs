//! The relay's API, read on a loopback address only.
//!
//! The address rule: the configured API host when it is a loopback address (a devnet node's API
//! listens on its own `127.0.0.x`), `127.0.0.1` or `::1` when the API binds a wildcard address,
//! and no read at all when the API is disabled or bound to a specific address that is not
//! loopback. At most [`MAX_REQUESTS`] requests a run ([`Budget`]), each step within
//! [`STEP_TIMEOUT`], no retry; an answer of 429 (the API's rate limit) gives `unknown` with that
//! reason.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;

use heartwood_kernel::Clock;
use serde_json::Value;

use crate::doctor::seams::{DoctorApi, RelayFacts};
use crate::doctor::sources::ParseError;
use crate::doctor::view::{Budget, Reason, Source, Unknown};
use crate::errors::HttpError;

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
    let unknown = |reason| Unknown::new(Source::Api, reason);
    let bound = relay.api.ok_or(unknown(Reason::ApiOff))?;
    match bound.ip() {
        ip if ip.is_loopback() => Ok(bound),
        IpAddr::V4(ip) if ip.is_unspecified() => Ok((Ipv4Addr::LOCALHOST, bound.port()).into()),
        IpAddr::V6(ip) if ip.is_unspecified() => Ok((Ipv6Addr::LOCALHOST, bound.port()).into()),
        _ => Err(unknown(Reason::ApiNotLoopback)),
    }
}

/// The body of `GET path`, taking one request of the budget; 429 gives [`Reason::RateLimited`],
/// any other status but 200 [`Reason::Unreadable`] (404 is the caller's to read as "none").
pub fn fetch(context: &ApiContext<'_>, path: &str) -> Result<(u16, Vec<u8>), Unknown> {
    context.budget.take_request(context.clock.now_ms())?;
    let unknown = |reason| Unknown::new(Source::Api, reason);
    let answer = context.api.get(context.address, path, STEP_TIMEOUT);
    match answer {
        Ok(response) => match response.status {
            200 | 404 => Ok((response.status, response.body)),
            429 => Err(unknown(Reason::RateLimited)),
            _ => Err(unknown(Reason::Unreadable)),
        },
        Err(HttpError::Timeout { .. }) => Err(unknown(Reason::TimedOut)),
        Err(HttpError::NotLoopback { .. }) => Err(unknown(Reason::ApiNotLoopback)),
        Err(HttpError::Response { .. } | HttpError::Body { .. }) => Err(unknown(Reason::Unparsed)),
        Err(HttpError::Status { status: 429, .. }) => Err(unknown(Reason::RateLimited)),
        Err(HttpError::Status { .. }) => Err(unknown(Reason::Unreadable)),
        Err(HttpError::Connection { .. }) => Err(unknown(Reason::NotRunning)),
    }
}

/// The body of a 200 answer to `GET path`, as JSON.
fn document(context: &ApiContext<'_>, path: &str) -> Result<Value, Unknown> {
    let (status, body) = fetch(context, path)?;
    let unparsed = |error| Unknown::from_parse(Source::Api, error);
    match status {
        200 => root(&body).map_err(unparsed),
        _ => Err(Unknown::new(Source::Api, Reason::Unreadable)),
    }
}

/// The JSON document of a body.
fn root(body: &[u8]) -> Result<Value, ParseError> {
    serde_json::from_slice(body).map_err(|_| ParseError::Malformed("not JSON"))
}

/// The `data` of a JSON body, for the parsers.
pub fn parse_data(body: &[u8]) -> Result<Value, ParseError> {
    let mut root = root(body)?;
    root.get_mut("data")
        .map(Value::take)
        .ok_or(ParseError::Malformed("no data"))
}

/// The `data` of a 200 answer to `GET path`.
fn read(context: &ApiContext<'_>, path: &str) -> Result<Value, Unknown> {
    let mut root = document(context, path)?;
    let missing = Unknown::from_parse(Source::Api, ParseError::Malformed("no data"));
    root.get_mut("data").map(Value::take).ok_or(missing)
}

/// A member of `value` as an unsigned number that fits `u32`.
fn number(value: &Value, key: &str) -> Option<u32> {
    u32::try_from(value.get(key)?.as_u64()?).ok()
}

/// A member of `value` as text.
fn text(value: &Value, key: &str) -> Option<String> {
    Some(value.get(key)?.as_str()?.to_owned())
}

/// `data` as `T`, or the unknown for an answer that is not what the API writes.
fn shaped<T>(data: &Value, shape: impl Fn(&Value) -> Option<T>) -> Result<T, Unknown> {
    let parsed = shape(data).ok_or(ParseError::Malformed("unexpected shape"));
    parsed.map_err(|error| Unknown::from_parse(Source::Api, error))
}

/// `GET /api/node/status`.
pub fn read_status(context: &ApiContext<'_>) -> Result<ApiStatus, Unknown> {
    let data = read(context, "/api/node/status")?;
    shaped(&data, |data| {
        Some(ApiStatus {
            synced: data.get("synced")?.as_bool()?,
            now: number(data, "now")?,
            blocks_count: data.get("blocksCount")?.as_i64()?,
        })
    })
}

/// `GET /api/node/syncing`.
pub fn read_syncing(context: &ApiContext<'_>) -> Result<ApiSyncing, Unknown> {
    let data = read(context, "/api/node/syncing")?;
    shaped(&data, |data| {
        Some(ApiSyncing {
            syncing: data.get("syncing")?.as_bool()?,
            blocks: data.get("blocks")?.as_i64()?,
            height: number(data, "height")?,
        })
    })
}

/// `GET /api/node/configuration`.
pub fn read_configuration(context: &ApiContext<'_>) -> Result<ApiConfiguration, Unknown> {
    let data = read(context, "/api/node/configuration")?;
    shaped(&data, |data| {
        data.as_object()?;
        let constants = data.get("constants");
        Some(ApiConfiguration {
            core_version: data
                .pointer("/core/version")
                .and_then(|v| Some(v.as_str()?.to_owned())),
            nethash: text(data, "nethash"),
            block_time_s: constants.and_then(|c| number(c, "blockTime")),
            active_delegates: constants.and_then(|c| number(c, "activeDelegates")),
        })
    })
}

/// One peer of a page of `GET /api/peers`; `None` for an entry without a usable address.
fn peer(entry: &Value) -> Option<ApiPeer> {
    Some(ApiPeer {
        ip: entry.get("ip")?.as_str()?.parse().ok()?,
        port: u16::try_from(entry.get("port")?.as_u64()?).ok()?,
        height: number(entry, "height"),
        version: text(entry, "version"),
    })
}

/// `GET /api/peers`, at most [`PEERS_PAGES`] pages of [`PEERS_PAGE`].
pub fn read_peers(context: &ApiContext<'_>) -> Result<ApiPeers, Unknown> {
    let mut peers = Vec::new();
    let mut total_count = 0_u32;
    for page in 1..=PEERS_PAGES {
        let path = format!("/api/peers?page={page}&limit={PEERS_PAGE}");
        let body = document(context, &path)?;
        let entries = body.get("data").and_then(Value::as_array);
        let entries = entries.ok_or(ParseError::Malformed("no peers"));
        let entries = entries.map_err(|error| Unknown::from_parse(Source::Api, error))?;
        total_count = body
            .pointer("/meta/totalCount")
            .and_then(Value::as_u64)
            .map_or(0, |n| u32::try_from(n).unwrap_or(u32::MAX));
        peers.extend(entries.iter().filter_map(peer));
        if entries.len() < PEERS_PAGE as usize || peers.len() >= total_count as usize {
            break;
        }
    }
    Ok(ApiPeers { total_count, peers })
}

/// A block of the API.
fn block(data: &Value) -> Option<ApiBlock> {
    let generator = data.get("generator");
    Some(ApiBlock {
        id: text(data, "id")?,
        height: number(data, "height")?,
        timestamp: data
            .get("timestamp")
            .and_then(|stamp| number(stamp, "epoch")),
        generator_public_key: generator.and_then(|g| text(g, "publicKey")),
        username: generator.and_then(|g| text(g, "username")),
    })
}

/// `GET /api/blocks/last`.
pub fn read_last_block(context: &ApiContext<'_>) -> Result<ApiBlock, Unknown> {
    let data = read(context, "/api/blocks/last")?;
    shaped(&data, block)
}

/// `GET /api/blocks/{height}`; `None` when the relay has no block there.
pub fn read_block(context: &ApiContext<'_>, height: u32) -> Result<Option<ApiBlock>, Unknown> {
    let (status, body) = fetch(context, &format!("/api/blocks/{height}"))?;
    if status == 404 {
        return Ok(None);
    }
    let data = parse_data(&body).map_err(|error| Unknown::from_parse(Source::Api, error))?;
    shaped(&data, block).map(Some)
}

/// `GET /api/wallets/{publicKey}`; `None` when there is no such wallet.
pub fn read_wallet(
    context: &ApiContext<'_>,
    public_key: &str,
) -> Result<Option<ApiWallet>, Unknown> {
    let (status, body) = fetch(context, &format!("/api/wallets/{public_key}"))?;
    if status == 404 {
        return Ok(None);
    }
    let data = parse_data(&body).map_err(|error| Unknown::from_parse(Source::Api, error))?;
    let wallet = |data: &Value| {
        Some(ApiWallet {
            address: text(data, "address")?,
            public_key: text(data, "publicKey"),
            username: data
                .pointer("/attributes/delegate/username")
                .and_then(|name| Some(name.as_str()?.to_owned())),
        })
    };
    shaped(&data, wallet).map(Some)
}
