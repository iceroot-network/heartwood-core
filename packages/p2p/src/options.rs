//! The p2p options, with the reference's defaults, and the node's own facts the client needs: its
//! version, its seeds and peer sources.
//!
//! [`P2pOptions::default`] is the reference's defaults without any environment variable; the
//! node's configuration fills the options from `app.json`, the environment and `peers.json`.

use std::net::IpAddr;
use std::time::Duration;

use heartwood_p2p_wire::replies::PeerAddress;
use serde_json::Value;

use crate::errors::OptionsError;

/// The p2p options.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct P2pOptions {
    /// The node's version, which every request's headers carry.
    pub version: String,
    /// `server.port` (`CORE_P2P_PORT`): the port every request's headers carry; 4002.
    pub port: u16,
    /// `minimumVersions`: the version ranges a peer must satisfy when the milestone names none;
    /// `^3.2` and `^3.2.0-next.0`.
    pub minimum_versions: Vec<String>,
    /// `minimumNetworkReach` (`CORE_P2P_MIN_NETWORK_REACH`): the peers below which the node
    /// re-seeds and discovers more often; 20.
    pub minimum_network_reach: usize,
    /// `verifyTimeout`: the time a new peer's ping may take, and a full cleanse's ping; 60 s.
    pub verify_timeout: Duration,
    /// `getBlocksTimeout`: the timeout of a getBlocks without its own; 30 s.
    pub get_blocks_timeout: Duration,
    /// `maxSameSubnetPeers` (`CORE_P2P_MAX_PEERS_SAME_SUBNET`): the most peers of one /24, seeds
    /// aside; 256.
    pub max_same_subnet_peers: usize,
    /// `maxPeerSequentialErrors` (`CORE_P2P_MAX_PEER_SEQUENTIAL_ERRORS`): the failed requests in
    /// a row after which a peer is disconnected and forgotten; 3.
    pub max_peer_sequential_errors: u32,
    /// `whitelist`: the addresses a peer must match (the kernel's globs); `*`.
    pub whitelist: Vec<String>,
    /// `blacklist`: the addresses a peer must not match; none.
    pub blacklist: Vec<String>,
    /// `rateLimit` (`CORE_P2P_RATE_LIMIT`): the requests per second to one peer; 100.
    pub rate_limit: u32,
    /// `rateLimitPostTransactions` (`CORE_P2P_RATE_LIMIT_POST_TRANSACTIONS`): the postTransactions
    /// per second to one peer; 25.
    pub rate_limit_post_transactions: u32,
    /// `disableDiscovery`: no candidate is accepted, and no discovery runs.
    pub disable_discovery: bool,
    /// `skipDiscovery`: no discovery at boot.
    pub skip_discovery: bool,
    /// `ignoreMinimumNetworkReach`: the minimum number of peers always counts as reached.
    pub ignore_minimum_network_reach: bool,
    /// `networkStart`: the node starts a new network (a cold start).
    pub network_start: bool,
    /// `CORE_P2P_ALLOW_PRIVATE_IP_RANGES` set to any non-empty value: private, link-local and
    /// unique-local addresses are valid peers.
    pub allow_private_ip_ranges: bool,
    /// The pool's `maxTransactionsPerRequest`: the size of the batches transactions are
    /// broadcast in; 40.
    pub max_transactions_per_request: usize,
    /// The seeds of `peers.json`'s `list`.
    pub seeds: Vec<PeerAddress>,
    /// `peers.json`'s `sources`: local files (a path starting with `/`) or plain HTTP URLs.
    pub sources: Vec<String>,
}

impl Default for P2pOptions {
    fn default() -> P2pOptions {
        P2pOptions {
            version: String::new(),
            port: 4002,
            minimum_versions: vec!["^3.2".to_owned(), "^3.2.0-next.0".to_owned()],
            minimum_network_reach: 20,
            verify_timeout: Duration::from_secs(60),
            get_blocks_timeout: Duration::from_secs(30),
            max_same_subnet_peers: 256,
            max_peer_sequential_errors: 3,
            whitelist: vec!["*".to_owned()],
            blacklist: Vec::new(),
            rate_limit: 100,
            rate_limit_post_transactions: 25,
            disable_discovery: false,
            skip_discovery: false,
            ignore_minimum_network_reach: false,
            network_start: false,
            allow_private_ip_ranges: false,
            max_transactions_per_request: 40,
            seeds: Vec::new(),
            sources: Vec::new(),
        }
    }
}

impl P2pOptions {
    /// The options of the p2p package's entry in `app.json`, over [`P2pOptions::default`], coerced
    /// as the reference's option schema coerces them (a number from numeric text, a boolean from
    /// `true` or `false` text, arrays replaced whole). The node's version, seeds and sources are
    /// not options and keep their defaults.
    ///
    /// Every key the schema names is checked when present, those the client does not use too
    /// (the server's, `remoteAccess`, `dns`, `ntp`); other keys are allowed, as the schema allows
    /// them.
    pub fn from_json(options: &Value) -> Result<P2pOptions, OptionsError> {
        if !options.is_object() {
            return Err(invalid("options", "must be an object"));
        }
        let get = |path: &str| {
            path.split('.')
                .try_fold(options, |value, key| value.get(key))
        };
        let int = |path: &str, min: u64| get(path).map(|v| integer(v, path, min)).transpose();
        let list = |key: &str| get(key).map(|v| strings(v, key)).transpose();
        let flag = |key: &str| get(key).map(|v| boolean(v, key)).transpose();
        let mut o = P2pOptions::default();
        int("server.banSeconds", 0)?;
        int("server.logLevel", 0)?;
        if let Some(hostname) = get("server.hostname") {
            ip(hostname.as_str().unwrap_or_default(), "server.hostname")?;
        }
        if let Some(port) = int("server.port", 1)? {
            o.port = u16::try_from(port).map_err(|_| invalid("server.port", "is over 65535"))?;
        }
        let count = |n: u64| usize::try_from(n).unwrap_or(usize::MAX);
        let small = |n: u64| u32::try_from(n).unwrap_or(u32::MAX);
        o.minimum_network_reach =
            int("minimumNetworkReach", 0)?.map_or(o.minimum_network_reach, count);
        o.verify_timeout = int("verifyTimeout", 0)?.map_or(o.verify_timeout, Duration::from_millis);
        o.get_blocks_timeout =
            int("getBlocksTimeout", 0)?.map_or(o.get_blocks_timeout, Duration::from_millis);
        o.max_same_subnet_peers =
            int("maxSameSubnetPeers", 0)?.map_or(o.max_same_subnet_peers, count);
        let errors = int("maxPeerSequentialErrors", 0)?;
        o.max_peer_sequential_errors = errors.map_or(o.max_peer_sequential_errors, small);
        o.rate_limit = int("rateLimit", 1)?.map_or(o.rate_limit, small);
        let posts = int("rateLimitPostTransactions", 1)?;
        o.rate_limit_post_transactions = posts.map_or(o.rate_limit_post_transactions, small);
        o.minimum_versions = list("minimumVersions")?.unwrap_or(o.minimum_versions);
        o.whitelist = list("whitelist")?.unwrap_or(o.whitelist);
        o.blacklist = list("blacklist")?.unwrap_or(o.blacklist);
        list("ntp")?;
        for key in ["remoteAccess", "dns"] {
            for entry in list(key)?.unwrap_or_default() {
                ip(&entry, key)?;
            }
        }
        o.network_start = flag("networkStart")?.unwrap_or(o.network_start);
        o.disable_discovery = flag("disableDiscovery")?.unwrap_or(o.disable_discovery);
        o.skip_discovery = flag("skipDiscovery")?.unwrap_or(o.skip_discovery);
        let ignore = flag("ignoreMinimumNetworkReach")?;
        o.ignore_minimum_network_reach = ignore.unwrap_or(o.ignore_minimum_network_reach);
        Ok(o)
    }
}

fn invalid(key: &str, reason: &str) -> OptionsError {
    OptionsError {
        key: key.to_owned(),
        reason: reason.to_owned(),
    }
}

/// A safe integer of at least `min`, from a JSON number or numeric text, as the schema's
/// `Joi.number().integer()` converts it (the kernel's reading of Joi's numbers).
fn integer(value: &Value, key: &str, min: u64) -> Result<u64, OptionsError> {
    let low = i64::try_from(min).unwrap_or(i64::MAX);
    let whole = heartwood_kernel::config::app::integer(value, low, i64::MAX);
    let whole = whole.and_then(|whole| u64::try_from(whole).ok());
    whole.ok_or_else(|| invalid(key, &format!("must be a safe integer of at least {min}")))
}

/// A boolean, or `true` or `false` text of any case, as `Joi.bool()` converts it.
fn boolean(value: &Value, key: &str) -> Result<bool, OptionsError> {
    match value.as_str().map(str::to_ascii_lowercase).as_deref() {
        Some("true") => Ok(true),
        Some("false") => Ok(false),
        _ => value
            .as_bool()
            .ok_or_else(|| invalid(key, "must be a boolean")),
    }
}

/// A list of non-empty strings, as `Joi.array().items(Joi.string())` accepts it.
fn strings(value: &Value, key: &str) -> Result<Vec<String>, OptionsError> {
    let items = value
        .as_array()
        .ok_or_else(|| invalid(key, "must be an array"))?;
    let text = |item: &Value| item.as_str().filter(|t| !t.is_empty()).map(str::to_owned);
    items
        .iter()
        .map(|item| text(item).ok_or_else(|| invalid(key, "must hold non-empty strings")))
        .collect()
}

/// An IPv4 or IPv6 address, with an optional prefix length, as `Joi.string().ip()` accepts it.
fn ip(text: &str, key: &str) -> Result<(), OptionsError> {
    let (address, prefix) = text
        .split_once('/')
        .map_or((text, None), |(a, p)| (a, Some(p)));
    let most = match address.parse::<IpAddr>() {
        Ok(IpAddr::V4(_)) => 32,
        Ok(IpAddr::V6(_)) => 128,
        Err(_) => return Err(invalid(key, "must be an IP address")),
    };
    match prefix.map(str::parse::<u8>) {
        None => Ok(()),
        Some(Ok(bits)) if bits <= most => Ok(()),
        Some(_) => Err(invalid(key, "has an invalid prefix length")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn options_are_coerced_over_the_defaults() {
        let o = P2pOptions::from_json(&json!({
            "server": {"banSeconds": 10, "hostname": "0.0.0.0", "port": "4003", "logLevel": 0},
            "minimumNetworkReach": " 5 ",
            "verifyTimeout": 30000,
            "whitelist": ["192.0.2.*"],
            "remoteAccess": ["127.0.0.1", "::1", "10.0.0.0/8"],
            "disableDiscovery": "TRUE",
            "skipDiscovery": false,
            "unknown": {"any": 1},
        }))
        .unwrap();
        assert_eq!(o.port, 4003);
        assert_eq!(o.minimum_network_reach, 5);
        assert_eq!(o.verify_timeout, Duration::from_secs(30));
        assert_eq!(o.whitelist, ["192.0.2.*"]);
        assert!(o.disable_discovery && !o.skip_discovery);
        assert_eq!(
            o.get_blocks_timeout,
            P2pOptions::default().get_blocks_timeout
        );
        assert_eq!(
            P2pOptions::from_json(&json!({})).unwrap(),
            P2pOptions::default()
        );
    }

    #[test]
    fn invalid_options_are_refused() {
        for (options, key) in [
            (json!({"server": {"port": 0}}), "server.port"),
            (json!({"server": {"port": "4002.5"}}), "server.port"),
            (
                json!({"server": {"hostname": "localhost"}}),
                "server.hostname",
            ),
            (json!({"rateLimit": 0}), "rateLimit"),
            (
                json!({"minimumNetworkReach": "20.50"}),
                "minimumNetworkReach",
            ),
            (json!({"minimumNetworkReach": "1.5"}), "minimumNetworkReach"),
            (json!({"minimumNetworkReach": null}), "minimumNetworkReach"),
            (json!({"whitelist": "*"}), "whitelist"),
            (json!({"blacklist": [""]}), "blacklist"),
            (json!({"dns": ["8.8.8.8", "dns.google"]}), "dns"),
            (json!({"networkStart": "yes"}), "networkStart"),
            (
                json!({"maxSameSubnetPeers": 9_007_199_254_740_992_u64}),
                "maxSameSubnetPeers",
            ),
        ] {
            let error = P2pOptions::from_json(&options).unwrap_err();
            assert_eq!(error.key, key, "{options}");
        }
        assert!(P2pOptions::from_json(&json!([])).is_err());
    }

    #[test]
    fn numeric_text_is_read_as_joi_reads_it() {
        let read = |text: &str| integer(&json!(text), "k", 0).ok();
        assert_eq!(read("20"), Some(20));
        assert_eq!(read(" +20 "), Some(20));
        assert_eq!(read("20.0"), Some(20));
        assert_eq!(read("020"), Some(20));
        assert_eq!(read("2e1"), Some(20));
        assert_eq!(read("0"), Some(0));
        // Joi reads `-0` as 0, and refuses as unsafe a number that does not print back as its text.
        assert_eq!(read("-0"), Some(0));
        assert_eq!(read(".2e2"), None);
        assert_eq!(read("1.5"), None);
        assert_eq!(read("0x10"), None);
        assert_eq!(read("Infinity"), None);
        assert_eq!(read(""), None);
        assert_eq!(read("."), None);
        assert_eq!(read("1e"), None);
        assert_eq!(integer(&json!(20.0), "k", 0).ok(), Some(20));
        assert_eq!(integer(&json!(-1), "k", 0).ok(), None);
    }
}
