//! What the routes read, and the route table's types.
//!
//! [`ApiContext`] holds the node's published snapshots (the state, the pool, the node's status),
//! the peer list, the read-only store connections, the submission of transactions to the node's
//! pool, the delegates' productivity and the chain's totals ([`Productivity`]), the node's
//! configuration and the API's options. A route is a [`Route`]: its
//! [`RouteId`] (method and path) and its handler, which takes the request with its query already
//! parsed and validated ([`ApiRequest`]) and answers a body ([`ApiReply`]) or an error. The
//! server's plugins wrap every handler with the envelope's headers, the rate limit, the whitelist
//! and the semaphore, so a handler is a plain function of the context and the request, testable
//! without HTTP.

use std::future::Future;
use std::net::IpAddr;
use std::pin::Pin;
use std::sync::Arc;

use heartwood_crypto::Block;
use heartwood_database::Readers;
use heartwood_kernel::config::CryptoFiles;
use heartwood_p2p::monitor::PeerInfo;
use heartwood_pool::{Entry, PoolView, ProcessorResult};
use heartwood_state::StateView;
use indexmap::IndexMap;
use serde_json::{Map, Value};
use tokio::sync::watch;

use crate::errors::ApiError;
use crate::server::ApiConfig;

/// The node's status as the node routes show it: filled by the node's binary.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NodeStatus {
    /// Whether the node is synced with the network.
    pub synced: bool,
    /// The network's height: the median of the peers' heights.
    pub network_height: u32,
}

/// The node's configuration as the configuration routes show it.
#[derive(Debug, Clone, PartialEq)]
pub struct NodeInfo {
    /// The loaded crypto files.
    pub crypto: CryptoFiles,
    /// The version the node reports (`4.3.1-heartwood.<crate version>`).
    pub version: String,
}

/// The peers, as the peer routes list them.
pub trait PeerList: Send + Sync {
    /// Every peer the node knows.
    fn peers(&self) -> Vec<PeerInfo>;
}

/// A submission could not reach the node's pool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("the node's pool cannot take transactions now")]
pub struct SubmitError;

/// The answer of a submission.
pub type SubmitFuture = Pin<Box<dyn Future<Output = Result<ProcessorResult, SubmitError>> + Send>>;

/// `POST /transactions`: entries to the node's API processor, on the node thread.
pub trait SubmitTransactions: Send + Sync {
    /// Submit `entries`, and answer the processor's result.
    fn submit(&self, entries: Vec<Entry>) -> SubmitFuture;
}

/// A productivity or missed count as the reference's `ROUND(numeric, 2)` gives it: an exact number
/// of hundredths, which a route writes as a JavaScript number (`9950` is `99.5`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Hundredths(pub u32);

/// A delegate's production over the missed-blocks lookback: the reference's wallet attributes
/// `delegate.missedBlocks` and `delegate.productivity`. `None` where the reference has forgotten
/// the attribute (a delegate with neither a produced nor a missed block in the window).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DelegateProduction {
    /// The missed slots in the window.
    pub missed: Option<u32>,
    /// The produced blocks over the produced and missed ones, in hundredths of a percent.
    pub productivity: Option<Hundredths>,
}

/// The production of every delegate that has one, by name, at one tip.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProductivityTable {
    /// The delegates' production, by name.
    pub by_username: IndexMap<String, DelegateProduction>,
}

/// The chain's totals that `GET /blockchain` shows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ChainTotals {
    /// The fees burned by every block.
    pub burned_fees: u128,
    /// The amounts of every burn transaction.
    pub burned_transactions: u128,
    /// The supply as the store's sums give it: what the genesis block issued and the blocks
    /// rewarded, less what they burned. `GET /blockchain` shows the sum of the balances, as the
    /// reference does; the two differ by the unburned share of a genesis transaction's fee.
    pub supply: i128,
}

/// The delegates' productivity and the chain's totals, read from the store per tip: a node
/// implements it over its read-only connections; the routes read it.
pub trait Productivity: Send + Sync {
    /// The production table at `tip`, cached by the tip's id.
    fn table(&self, tip: &Block) -> Result<Arc<ProductivityTable>, ApiError>;
    /// The chain's totals at `tip`, kept up to date while the tip grows.
    fn totals(&self, tip: &Block) -> Result<ChainTotals, ApiError>;
}

/// What the routes read.
pub struct ApiContext {
    /// The published state.
    pub state: watch::Receiver<Arc<StateView>>,
    /// The published pool and online record.
    pub pool: watch::Receiver<Arc<PoolView>>,
    /// The node's status.
    pub status: watch::Receiver<NodeStatus>,
    /// The peers.
    pub peers: Arc<dyn PeerList>,
    /// The store's read-only connections.
    pub history: Arc<Readers>,
    /// The submission of transactions.
    pub submit: Arc<dyn SubmitTransactions>,
    /// The delegates' productivity and the chain's totals.
    pub productivity: Arc<dyn Productivity>,
    /// The node's configuration.
    pub node: Arc<NodeInfo>,
    /// The API's options.
    pub config: Arc<ApiConfig>,
}

impl ApiContext {
    /// The state now.
    pub fn state(&self) -> Arc<StateView> {
        Arc::clone(&self.state.borrow())
    }

    /// The pool now.
    pub fn pool(&self) -> Arc<PoolView> {
        Arc::clone(&self.pool.borrow())
    }

    /// The node's status now.
    pub fn status(&self) -> NodeStatus {
        *self.status.borrow()
    }
}

/// An HTTP method of a route.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    /// `GET`.
    Get,
    /// `POST`.
    Post,
}

/// The served routes, by the reference's controller and method names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum RouteId {
    /// `GET /blocks/last`.
    BlocksLast,
    /// `GET /blocks/{id}`.
    BlocksShow,
    /// `GET /blocks/{id}/transactions`.
    BlocksTransactions,
    /// `GET /node/status`.
    NodeStatus,
    /// `GET /node/syncing`.
    NodeSyncing,
    /// `GET /node/configuration`.
    NodeConfiguration,
    /// `GET /node/configuration/crypto`.
    NodeConfigurationCrypto,
    /// `GET /peers`.
    PeersIndex,
    /// `GET /peers/{ip}`.
    PeersShow,
    /// `GET /transactions`.
    TransactionsIndex,
    /// `POST /transactions`.
    TransactionsStore,
    /// `GET /transactions/{id}`.
    TransactionsShow,
    /// `GET /transactions/unconfirmed`.
    TransactionsUnconfirmed,
    /// `GET /transactions/unconfirmed/{id}`.
    TransactionsShowUnconfirmed,
    /// `GET /transactions/types`.
    TransactionsTypes,
    /// `GET /transactions/fees`.
    TransactionsFees,
    /// `GET /wallets/{id}`.
    WalletsShow,
    /// `GET /wallets/{id}/transactions`.
    WalletsTransactions,
    /// `GET /blockchain`.
    BlockchainIndex,
    /// `GET /blockchain/search/{id}`.
    BlockchainSearch,
    /// `GET /blocks`.
    BlocksIndex,
    /// `GET /blocks/missed`.
    BlocksMissed,
    /// `GET /blocks/first`.
    BlocksFirst,
    /// `GET /delegates`.
    DelegatesIndex,
    /// `GET /delegates/{id}`.
    DelegatesShow,
    /// `GET /delegates/{id}/voters`.
    DelegatesVoters,
    /// `GET /delegates/{id}/blocks`.
    DelegatesBlocks,
    /// `GET /delegates/{id}/blocks/missed`.
    DelegatesMissedBlocks,
    /// `GET /node/fees`.
    NodeFees,
    /// `GET /rounds/{id}/delegates`.
    RoundsDelegates,
    /// `GET /transactions/schemas`.
    TransactionsSchemas,
    /// `GET /votes`.
    VotesIndex,
    /// `GET /votes/{id}`.
    VotesShow,
    /// `GET /wallets`.
    WalletsIndex,
    /// `GET /wallets/top`.
    WalletsTop,
    /// `GET /wallets/{id}/transactions/sent`.
    WalletsTransactionsSent,
    /// `GET /wallets/{id}/transactions/received`.
    WalletsTransactionsReceived,
    /// `GET /wallets/{id}/votes`.
    WalletsVotes,
    /// `GET /api.json`, the API's OpenAPI document: outside the base path, so it is in no route
    /// table under it and not in [`RouteId::ALL`].
    ApiSpec,
}

impl RouteId {
    /// Every route under the base path ([`RouteId::ApiSpec`] is outside it).
    pub const ALL: [RouteId; 38] = [
        RouteId::BlocksLast,
        RouteId::BlocksShow,
        RouteId::BlocksTransactions,
        RouteId::NodeStatus,
        RouteId::NodeSyncing,
        RouteId::NodeConfiguration,
        RouteId::NodeConfigurationCrypto,
        RouteId::PeersIndex,
        RouteId::PeersShow,
        RouteId::TransactionsIndex,
        RouteId::TransactionsStore,
        RouteId::TransactionsShow,
        RouteId::TransactionsUnconfirmed,
        RouteId::TransactionsShowUnconfirmed,
        RouteId::TransactionsTypes,
        RouteId::TransactionsFees,
        RouteId::WalletsShow,
        RouteId::WalletsTransactions,
        RouteId::BlockchainIndex,
        RouteId::BlockchainSearch,
        RouteId::BlocksIndex,
        RouteId::BlocksMissed,
        RouteId::BlocksFirst,
        RouteId::DelegatesIndex,
        RouteId::DelegatesShow,
        RouteId::DelegatesVoters,
        RouteId::DelegatesBlocks,
        RouteId::DelegatesMissedBlocks,
        RouteId::NodeFees,
        RouteId::RoundsDelegates,
        RouteId::TransactionsSchemas,
        RouteId::VotesIndex,
        RouteId::VotesShow,
        RouteId::WalletsIndex,
        RouteId::WalletsTop,
        RouteId::WalletsTransactionsSent,
        RouteId::WalletsTransactionsReceived,
        RouteId::WalletsVotes,
    ];

    /// The route's method.
    pub const fn method(self) -> Method {
        match self {
            RouteId::TransactionsStore => Method::Post,
            _ => Method::Get,
        }
    }

    /// The route's path under the base path `/api`, with its parameters in braces;
    /// [`RouteId::ApiSpec`]'s is its path from the server's root.
    pub const fn path(self) -> &'static str {
        match self {
            RouteId::BlocksLast => "/blocks/last",
            RouteId::BlocksShow => "/blocks/{id}",
            RouteId::BlocksTransactions => "/blocks/{id}/transactions",
            RouteId::NodeStatus => "/node/status",
            RouteId::NodeSyncing => "/node/syncing",
            RouteId::NodeConfiguration => "/node/configuration",
            RouteId::NodeConfigurationCrypto => "/node/configuration/crypto",
            RouteId::PeersIndex => "/peers",
            RouteId::PeersShow => "/peers/{ip}",
            RouteId::TransactionsIndex | RouteId::TransactionsStore => "/transactions",
            RouteId::TransactionsShow => "/transactions/{id}",
            RouteId::TransactionsUnconfirmed => "/transactions/unconfirmed",
            RouteId::TransactionsShowUnconfirmed => "/transactions/unconfirmed/{id}",
            RouteId::TransactionsTypes => "/transactions/types",
            RouteId::TransactionsFees => "/transactions/fees",
            RouteId::WalletsShow => "/wallets/{id}",
            RouteId::WalletsTransactions => "/wallets/{id}/transactions",
            RouteId::BlockchainIndex => "/blockchain",
            RouteId::BlockchainSearch => "/blockchain/search/{id}",
            RouteId::BlocksIndex => "/blocks",
            RouteId::BlocksMissed => "/blocks/missed",
            RouteId::BlocksFirst => "/blocks/first",
            RouteId::DelegatesIndex => "/delegates",
            RouteId::DelegatesShow => "/delegates/{id}",
            RouteId::DelegatesVoters => "/delegates/{id}/voters",
            RouteId::DelegatesBlocks => "/delegates/{id}/blocks",
            RouteId::DelegatesMissedBlocks => "/delegates/{id}/blocks/missed",
            RouteId::NodeFees => "/node/fees",
            RouteId::RoundsDelegates => "/rounds/{id}/delegates",
            RouteId::TransactionsSchemas => "/transactions/schemas",
            RouteId::VotesIndex => "/votes",
            RouteId::VotesShow => "/votes/{id}",
            RouteId::WalletsIndex => "/wallets",
            RouteId::WalletsTop => "/wallets/top",
            RouteId::WalletsTransactionsSent => "/wallets/{id}/transactions/sent",
            RouteId::WalletsTransactionsReceived => "/wallets/{id}/transactions/received",
            RouteId::WalletsVotes => "/wallets/{id}/votes",
            RouteId::ApiSpec => "/api.json",
        }
    }
}

/// A request as a handler sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct ApiRequest {
    /// The route.
    pub route: RouteId,
    /// The path parameters, by name (`id`, `ip`).
    pub params: Map<String, Value>,
    /// The path as requested under the base path, without the query, for the pagination links.
    pub path: String,
    /// The query, parsed and validated (defaults added, values coerced) for the route.
    pub query: Value,
    /// The JSON payload of a `POST`.
    pub payload: Option<Value>,
    /// The client's address.
    pub client: IpAddr,
}

impl ApiRequest {
    /// The path parameter `name`, as text.
    pub fn param(&self, name: &str) -> Option<&str> {
        self.params.get(name).and_then(Value::as_str)
    }
}

/// A handler's answer: its status (200 unless the route says otherwise) and its body.
#[derive(Debug, Clone, PartialEq)]
pub struct ApiReply {
    /// The HTTP status.
    pub status: u16,
    /// The body.
    pub body: Value,
}

impl ApiReply {
    /// 200 with `body` as it is.
    pub fn ok(body: Value) -> ApiReply {
        ApiReply { status: 200, body }
    }

    /// 200 with `{"data": data}`.
    pub fn data(data: Value) -> ApiReply {
        let mut body = Map::new();
        body.insert("data".into(), data);
        ApiReply::ok(Value::Object(body))
    }
}

/// A handler's future.
pub type HandlerFuture = Pin<Box<dyn Future<Output = Result<ApiReply, ApiError>> + Send>>;

/// A handler: the context and the request to the answer.
pub type Handler = fn(Arc<ApiContext>, ApiRequest) -> HandlerFuture;

/// A route: its id and its handler.
#[derive(Clone, Copy)]
pub struct Route {
    /// The route.
    pub id: RouteId,
    /// The handler.
    pub handler: Handler,
}

impl std::fmt::Debug for Route {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Route").field("id", &self.id).finish()
    }
}
