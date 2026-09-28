//! The server: the API's options, and the accept loop on the runtime the node's binary hands in,
//! with a 5 s socket timeout, serving [`crate::plugins`]' pipeline under `/api`.
//!
//! The options are the reference's defaults (`CORE_API_*` read as the reference reads them: a
//! switch is on for any non-empty value, a missing or empty value takes the default), with the
//! entry of `app.json` merged over them and each value read as the reference's option schema
//! reads it (numbers from their text, booleans from `"true"` and `"false"`). Keys the API does not
//! read are ignored.
//!
//! Each connection is served for at most the socket timeout (300 s when it is 0), headers
//! included, and answers one request (`connection: close`). Its head is read first as Node's
//! parser reads it (`head`): a head that parser refuses, one of 16 KiB or more included, is
//! answered as the reference answers it, with a bare 400. Stopping the server closes the listener
//! and waits for the connections in flight, each bounded by that timeout.

mod head;

use std::convert::Infallible;
use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use heartwood_crypto::utils::js;
use heartwood_kernel::config::merge::deep_merge;
use heartwood_kernel::config::{Env, Options, OptionsError};
use hyper::server::conn::http1;
use hyper_util::rt::{TokioIo, TokioTimer};
use serde_json::{Value, json};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tokio::task::JoinSet;

use crate::context::ApiContext;
use crate::controllers;
use crate::errors::ServeError;
use crate::plugins::{InvalidUrl, Pipeline};

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
    /// The socket timeout, in milliseconds (5,000); 0 turns it off
    /// ([`ApiConfig::connection_timeout`]).
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

impl ApiConfig {
    /// How long a connection is served, headers included: the socket timeout, or where it is 0
    /// (no socket timeout in the reference) Node's request timeout of 300 s, which then still
    /// bounds a request there.
    pub fn connection_timeout(&self) -> Duration {
        match self.socket_timeout_ms {
            0 => Duration::from_secs(300),
            ms => Duration::from_millis(ms),
        }
    }

    /// The reference's default options, with the variables of `env` that they read.
    pub fn defaults(env: &Env) -> Value {
        // `process.env.X || default`: a missing or empty variable takes the default.
        let or = |name: &str, default: Value| {
            env.get(name)
                .filter(|value| !value.is_empty())
                .map_or(default, Value::from)
        };
        let list = |name: &str| {
            let values = env.get(name).filter(|value| !value.is_empty());
            Value::from(values.map_or_else(Vec::new, |value| value.split(',').collect()))
        };
        let first = |concurrency: u32, queue_limit: u32, max_offset: u32| {
            json!({
                "concurrency": concurrency, "queueLimit": queue_limit, "maxOffset": max_offset
            })
        };
        let second = json!({ "concurrency": 1, "queueLimit": 5 });
        let database = json!({ "levelOne": first(10, 100, 10_000), "levelTwo": second });
        let memory = json!({ "levelOne": first(3, 30, 1_000), "levelTwo": second });
        json!({
            "server": {
                "http": {
                    "enabled": !env.is_set("CORE_API_DISABLED"),
                    "host": or("CORE_API_HOST", json!("0.0.0.0")),
                    "port": or("CORE_API_PORT", json!(4003)),
                },
                "https": { "enabled": env.is_set("CORE_API_SSL") },
            },
            "plugins": {
                "semaphore": {
                    "enabled": !env.is_set("CORE_API_SEMAPHORE_DISABLED"),
                    "database": database,
                    "memory": memory,
                },
                "rateLimit": {
                    "enabled": !env.is_set("CORE_API_RATE_LIMIT_DISABLED"),
                    "points": or("CORE_API_RATE_LIMIT_USER_LIMIT", json!(100)),
                    "duration": or("CORE_API_RATE_LIMIT_USER_EXPIRES", json!(60)),
                    "whitelist": list("CORE_API_RATE_LIMIT_WHITELIST"),
                    "blacklist": list("CORE_API_RATE_LIMIT_BLACKLIST"),
                },
                "socketTimeout": 5000,
                "whitelist": ["*"],
                "trustProxy": env.is_set("CORE_API_TRUST_PROXY"),
            },
            "options": {
                "basePath": "/api",
                "estimateTotalCount": env.is_set("CORE_API_ESTIMATED_TOTAL_COUNT"),
            },
        })
    }
}

impl Options for ApiConfig {
    const PACKAGE: &'static str = "@solar-network/api";

    fn from_options(value: &Value, env: &Env) -> Result<ApiConfig, OptionsError> {
        const MAX: u64 = js::MAX_SAFE_INTEGER;
        const U32: u64 = u32::MAX as u64;
        let options = Read(deep_merge(&ApiConfig::defaults(env), value));
        let level = |kind: &str, level: &str| {
            let key = |name: &str| format!("plugins.semaphore.{kind}.{level}.{name}");
            let max_offset =
                (level == "levelOne").then(|| options.integer(&key("maxOffset"), 0, MAX));
            Ok::<_, OptionsError>(SemaphoreLevel {
                concurrency: options.integer(&key("concurrency"), 0, U32)?,
                queue_limit: options.integer(&key("queueLimit"), 0, U32)?,
                max_offset: max_offset.transpose()?,
            })
        };
        let levels = |kind: &str| {
            Ok::<_, OptionsError>([level(kind, "levelOne")?, level(kind, "levelTwo")?])
        };
        let host = options.text("server.http.host")?;
        let port: u16 = options.integer("server.http.port", 1, 65_535)?;
        let addresses = (host.as_str(), port).to_socket_addrs().ok();
        let address = addresses.and_then(|mut addresses| addresses.next());
        let unknown = || refused("server.http.host", "is not an address to listen on");
        let address = address.ok_or_else(unknown)?;
        Ok(ApiConfig {
            enabled: options.flag("server.http.enabled")?,
            address,
            https: options.flag("server.https.enabled")?,
            rate_limit: RateLimit {
                enabled: options.flag("plugins.rateLimit.enabled")?,
                points: options.integer("plugins.rateLimit.points", 0, U32)?,
                duration_s: options.integer("plugins.rateLimit.duration", 0, U32)?,
                whitelist: options.texts("plugins.rateLimit.whitelist")?,
                blacklist: options.texts("plugins.rateLimit.blacklist")?,
            },
            semaphore: Semaphore {
                enabled: options.flag("plugins.semaphore.enabled")?,
                database: levels("database")?,
                memory: levels("memory")?,
            },
            socket_timeout_ms: options.integer("plugins.socketTimeout", 0, MAX)?,
            whitelist: options.texts("plugins.whitelist")?,
            trust_proxy: options.flag("plugins.trustProxy")?,
            base_path: options.text("options.basePath")?,
            estimate_total_count: options.flag("options.estimateTotalCount")?,
        })
    }
}

fn refused(key: &str, reason: &str) -> OptionsError {
    OptionsError {
        package: ApiConfig::PACKAGE,
        key: key.to_owned(),
        reason: reason.to_owned(),
    }
}

/// The merged options, read as the reference's schema reads them.
struct Read(Value);

impl Read {
    fn get(&self, key: &str) -> Result<&Value, OptionsError> {
        key.split('.')
            .try_fold(&self.0, |value, name| value.get(name))
            .ok_or_else(|| refused(key, "is required"))
    }

    /// A boolean, or the text `true` or `false` in any case.
    fn flag(&self, key: &str) -> Result<bool, OptionsError> {
        match self.get(key)? {
            Value::Bool(flag) => Ok(*flag),
            Value::String(text) if text.eq_ignore_ascii_case("true") => Ok(true),
            Value::String(text) if text.eq_ignore_ascii_case("false") => Ok(false),
            _ => Err(refused(key, "must be a boolean")),
        }
    }

    /// A safe integer from `min` to `max`, or a decimal text of one, as a `T`.
    fn integer<T: TryFrom<u64>>(&self, key: &str, min: u64, max: u64) -> Result<T, OptionsError> {
        let number = match self.get(key)? {
            Value::Number(number) => Some(js::number_value(number)),
            Value::String(text) => number_text(text),
            _ => None,
        };
        let range = (min as f64)..=(max as f64);
        let whole = number.filter(|number| number.fract() == 0.0 && range.contains(number));
        let refusal = || refused(key, &format!("must be an integer from {min} to {max}"));
        whole
            .and_then(|number| T::try_from(number as u64).ok())
            .ok_or_else(refusal)
    }

    /// A non-empty text.
    fn text(&self, key: &str) -> Result<String, OptionsError> {
        match self.get(key)? {
            Value::String(text) if !text.is_empty() => Ok(text.clone()),
            _ => Err(refused(key, "must be a non-empty string")),
        }
    }

    /// A list of texts.
    fn texts(&self, key: &str) -> Result<Vec<String>, OptionsError> {
        let refusal = || refused(key, "must be an array of strings");
        let items = self.get(key)?.as_array().ok_or_else(refusal)?;
        let text = |item: &Value| item.as_str().map(str::to_owned).ok_or_else(refusal);
        items.iter().map(text).collect()
    }
}

/// A number in `text` as the reference's schema converts one: optional white space and sign,
/// digits with an optional fraction, an optional exponent (Rust's grammar of a number, without its
/// infinity and NaN).
fn number_text(text: &str) -> Option<f64> {
    let text = text.trim();
    let letters = text.contains(|c: char| c.is_ascii_alphabetic() && !matches!(c, 'e' | 'E'));
    text.parse().ok().filter(|_| !letters)
}

/// A served API: its task, until it is stopped.
#[derive(Debug)]
pub struct ServerHandle {
    task: tokio::task::JoinHandle<()>,
    stop: watch::Sender<bool>,
}

impl ServerHandle {
    /// Stop accepting connections, and wait for the served requests to end.
    pub async fn stop(self) {
        let _ = self.stop.send(true);
        let _ = self.task.await;
    }
}

/// Serve the API of `context` on `runtime`.
pub fn serve(
    context: Arc<ApiContext>,
    runtime: &tokio::runtime::Handle,
) -> Result<ServerHandle, ServeError> {
    let config = Arc::clone(&context.config);
    if config.https {
        return Err(ServeError::Https);
    }
    let address = config.address;
    let bind_error = |error: std::io::Error| ServeError::Bind {
        address,
        detail: error.to_string(),
    };
    let listener = std::net::TcpListener::bind(address).map_err(bind_error)?;
    listener.set_nonblocking(true).map_err(bind_error)?;
    let listener = {
        let _entered = runtime.enter();
        TcpListener::from_std(listener).map_err(bind_error)?
    };
    let pipeline = Arc::new(Pipeline::new(controllers::routes(), context));
    let timeout = config.connection_timeout();
    let (stop, stopped) = watch::channel(false);
    let task = runtime.spawn(accept(listener, pipeline, stopped, timeout));
    Ok(ServerHandle { task, stop })
}

/// Accept connections until `stopped` changes, then wait for those in flight.
pub(crate) async fn accept(
    listener: TcpListener,
    pipeline: Arc<Pipeline>,
    mut stopped: watch::Receiver<bool>,
    timeout: Duration,
) {
    let mut connections = JoinSet::new();
    loop {
        tokio::select! {
            _ = stopped.changed() => break,
            Some(_) = connections.join_next(), if !connections.is_empty() => {}
            accepted = listener.accept() => match accepted {
                Ok((stream, peer)) => {
                    connections.spawn(connection(stream, peer, Arc::clone(&pipeline), timeout));
                }
                // Out of descriptors, or a connection reset before it was accepted: try again
                // shortly, without spinning.
                Err(_) => tokio::time::sleep(Duration::from_millis(50)).await,
            },
        }
    }
    drop(listener);
    while connections.join_next().await.is_some() {}
}

/// Serve one connection for at most `timeout`, headers included: its head is read first as Node's
/// parser reads it, and refused where that parser refuses it ([`head`]).
async fn connection(
    mut stream: TcpStream,
    peer: SocketAddr,
    pipeline: Arc<Pipeline>,
    timeout: Duration,
) {
    let served = async move {
        let Ok(Some(head)) = head::read(&mut stream).await else {
            return;
        };
        let invalid_url = head.invalid_url;
        let service = hyper::service::service_fn(move |request: hyper::Request<_>| {
            let pipeline = Arc::clone(&pipeline);
            let mut request = request.map(Body::new);
            if invalid_url {
                request.extensions_mut().insert(InvalidUrl);
            }
            async move { Ok::<_, Infallible>(pipeline.handle(request, peer).await) }
        });
        let mut builder = http1::Builder::new();
        builder.timer(TokioTimer::new());
        builder.header_read_timeout(timeout);
        // hyper's default is 100 fields; Node's server takes any number.
        if head.fields > 100 {
            builder.max_headers(head.fields);
        }
        let stream = TokioIo::new(head::Rewind::new(head.bytes, stream));
        let _ = builder.serve_connection(stream, service).await;
    };
    let _ = tokio::time::timeout(timeout, served).await;
}
