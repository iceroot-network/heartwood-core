//! The server: the API's options, and the accept loop on the runtime the node's binary hands in,
//! with a 5 s socket timeout, serving [`crate::plugins::router`] under `/api`.

use std::net::SocketAddr;
use std::sync::Arc;

use heartwood_kernel::config::{Env, Options, OptionsError};
use serde_json::Value;

use crate::context::ApiContext;
use crate::errors::ServeError;

/// The rate limit's options (`plugins.rateLimit`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RateLimit {
    /// Whether it applies (`CORE_API_RATE_LIMIT_DISABLED` turns it off).
    pub enabled: bool,
    /// The points of an address per window (`CORE_API_RATE_LIMIT_USER_LIMIT`, 100).
    pub points: u32,
    /// The window, in seconds (`CORE_API_RATE_LIMIT_USER_EXPIRES`, 60).
    pub duration_s: u32,
    /// Addresses without a limit (`CORE_API_RATE_LIMIT_WHITELIST`).
    pub whitelist: Vec<String>,
    /// Addresses always refused (`CORE_API_RATE_LIMIT_BLACKLIST`).
    pub blacklist: Vec<String>,
}

/// One level of the semaphore.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SemaphoreLevel {
    /// Requests served at once.
    pub concurrency: u32,
    /// Requests waiting before a refusal.
    pub queue_limit: u32,
    /// The largest offset of the level, when it has one.
    pub max_offset: Option<u64>,
}

/// The semaphore's options (`plugins.semaphore`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Semaphore {
    /// Whether it applies (`CORE_API_SEMAPHORE_DISABLED` turns it off).
    pub enabled: bool,
    /// The store's routes, first and second level.
    pub database: [SemaphoreLevel; 2],
    /// The memory's routes, first and second level.
    pub memory: [SemaphoreLevel; 2],
}

/// The API's options, as the reference's defaults and the node's variables give them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiConfig {
    /// Whether the API is served (`CORE_API_DISABLED` turns it off).
    pub enabled: bool,
    /// The address it listens on (`CORE_API_HOST`, `0.0.0.0`, and `CORE_API_PORT`).
    pub address: SocketAddr,
    /// Whether HTTPS was asked for (`CORE_API_SSL`): the start stops.
    pub https: bool,
    /// The rate limit.
    pub rate_limit: RateLimit,
    /// The semaphore.
    pub semaphore: Semaphore,
    /// The socket timeout, in milliseconds (5,000).
    pub socket_timeout_ms: u64,
    /// The addresses allowed to use the API (`["*"]`).
    pub whitelist: Vec<String>,
    /// Whether the client address comes from `X-Forwarded-For` (`CORE_API_TRUST_PROXY`).
    pub trust_proxy: bool,
    /// The base path (`/api`).
    pub base_path: String,
    /// Whether list counts are estimates (`CORE_API_ESTIMATED_TOTAL_COUNT`).
    pub estimate_total_count: bool,
}

impl Options for ApiConfig {
    const PACKAGE: &'static str = "@solar-network/api";

    fn from_options(value: &Value, env: &Env) -> Result<ApiConfig, OptionsError> {
        let _ = (value, env);
        todo!()
    }
}

/// A served API: its task, until it is stopped.
#[derive(Debug)]
pub struct ServerHandle {
    task: tokio::task::JoinHandle<()>,
    stop: tokio::sync::watch::Sender<bool>,
}

impl ServerHandle {
    /// Stop accepting connections, and wait for the served requests to end.
    pub async fn stop(self) {
        let _ = (self.task, self.stop);
        todo!()
    }
}

/// Serve the API of `context` on `runtime`.
pub fn serve(
    context: Arc<ApiContext>,
    runtime: &tokio::runtime::Handle,
) -> Result<ServerHandle, ServeError> {
    let _ = (context, runtime);
    todo!()
}
