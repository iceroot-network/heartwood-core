//! The p2p options, with the reference's defaults, and the node's own facts the client needs: its
//! version, its seeds and peer sources.
//!
//! [`P2pOptions::default`] is the reference's defaults without any environment variable; the
//! node's configuration fills the options from `app.json`, the environment and `peers.json`.

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
    pub fn from_json(options: &Value) -> Result<P2pOptions, OptionsError> {
        let _ = options;
        todo!("coerce the options")
    }
}
