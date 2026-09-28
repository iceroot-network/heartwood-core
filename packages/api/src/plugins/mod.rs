//! The server's plugins, around every route: the rate limit (100 points per 60 s per address,
//! charged before validation, its whitelist and blacklist; 429 without `Retry-After`), the address
//! whitelist (403), `X-Block-Height` (errors included), `connection: close`, CORS in hapi's shape,
//! trailing slashes stripped, 404 (not 405) for another method, the 1 MiB body limit, the
//! semaphore (bounded concurrency of the store's routes), the query grammar and the routes'
//! validation ([`crate::schemas`]), and Boom's error bodies.
//!
//! A request goes through the reference's lifecycle in its order: the whitelist; the query
//! grammar (a query it cannot read is a 500 on any path); the route (a preflight `OPTIONS` is
//! answered here); the payload of a `POST` (its size, type and JSON, in `payload`: any failure is
//! a 422); the rate limit; the path parameters, the query and the payload against the route's
//! schema (422); the semaphore; the handler. Every answer, errors included, gets `X-Block-Height`
//! (the published tip), `connection: close`, `cache-control: no-cache` and, but for a preflight's,
//! the route's CORS headers: `vary: origin` and `access-control-expose-headers` always (hapi's
//! origin setting `*` matches a request without an `Origin` too), `access-control-allow-origin`
//! when the request has an `Origin`. A `GET`'s 200 answer with a body offers byte ranges
//! (`accept-ranges: bytes`) and serves one (in `ranges`). A `GET` or `HEAD` that no route matches,
//! anywhere, is answered as the reference's catch-all route answers it (in `catch_all`): 400
//! uncharged when its path does not decode, else charged a point and answered 404 (403 for a short
//! path that leads above the documentation's directory); another method on an unmatched path is
//! answered 404 before the rate limit.
//!
//! The request's headers are read as Node's server gives them to hapi ([`node_header`]): as
//! Latin-1, and a repeated header's values joined with `, `.

mod catch_all;
mod limits;
mod payload;
mod ranges;
#[cfg(test)]
mod tests;

use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex, PoisonError};

use axum::Router;
use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::header::{self, HeaderName};
use axum::http::{HeaderMap, HeaderValue, Method as Verb, Request, Response, StatusCode};
use heartwood_crypto::utils::{hex, js};
use heartwood_kernel::ip;
use serde_json::{Map, Value, json};
use tokio::time::Instant;

use self::catch_all::CatchAll;
use self::limits::{Gates, RateLimiter};
use crate::context::{ApiContext, ApiRequest, HandlerFuture, Method, Route, RouteId};
use crate::errors::ApiError;
use crate::schemas;
use crate::server::ApiConfig;

/// The headers that CORS exposes, hapi's default.
const EXPOSED: &str = "WWW-Authenticate,Server-Authorization";

/// The headers Node's server gives hapi the first value of, when a request repeats them; it joins
/// the values of any other.
const FIRST_ONLY: [&str; 18] = [
    "age",
    "authorization",
    "content-length",
    "content-type",
    "etag",
    "expires",
    "from",
    "host",
    "if-modified-since",
    "if-unmodified-since",
    "last-modified",
    "location",
    "max-forwards",
    "proxy-authorization",
    "referer",
    "retry-after",
    "server",
    "user-agent",
];

/// The headers a preflight may ask for, hapi's default, and `origin`.
const ALLOWED: [&str; 5] = [
    "accept",
    "authorization",
    "content-type",
    "if-none-match",
    "origin",
];

/// The router of `routes` under the base path, wrapped in the plugins. The client's address is
/// the connection's (`ConnectInfo`), when the router is served with it.
pub fn router(routes: Vec<Route>, context: Arc<ApiContext>) -> Router {
    let pipeline = Arc::new(Pipeline::new(routes, context));
    Router::new().fallback(move |request: Request<Body>| {
        let pipeline = Arc::clone(&pipeline);
        async move {
            let connection = request.extensions().get::<ConnectInfo<SocketAddr>>();
            let peer = connection.map_or(SocketAddr::from(([0, 0, 0, 0], 0)), |info| info.0);
            pipeline.handle(request, peer).await
        }
    })
}

/// The query grammar and the routes' schemas: [`crate::schemas`] in a node, fakes in the plugins'
/// own tests.
#[derive(Clone, Copy)]
pub(crate) struct Grammar {
    pub(crate) parse: fn(&str) -> Value,
    pub(crate) params: fn(RouteId, &Params) -> Result<(), ApiError>,
    pub(crate) query: fn(RouteId, &Value) -> Result<Value, ApiError>,
    pub(crate) payload: fn(RouteId, &Value) -> Result<Value, ApiError>,
}

impl Grammar {
    /// The served grammar.
    const SCHEMAS: Grammar = Grammar {
        parse: schemas::parse_query,
        params: schemas::validate_params,
        query: schemas::validate_query,
        payload: schemas::validate_payload,
    };
}

/// What the plugins read of the node (the options and the published tip's height) and the
/// handlers they call: the context's in a node, fakes in the plugins' own tests.
pub(crate) struct Host {
    pub(crate) config: Arc<ApiConfig>,
    pub(crate) height: Box<dyn Fn() -> u32 + Send + Sync>,
    pub(crate) dispatch: Box<dyn Fn(RouteId, ApiRequest) -> HandlerFuture + Send + Sync>,
}

/// A request's path parameters, by name.
type Params = Map<String, Value>;

/// Headers of an answer.
type Headers = Vec<(HeaderName, HeaderValue)>;

/// An answer before the headers every response gets.
struct Answer {
    status: u16,
    body: Option<Value>,
    headers: Headers,
    /// Whether the route's CORS headers apply (not to a preflight's answer).
    cors: bool,
}

impl Answer {
    fn new(status: u16, body: Option<Value>, headers: Headers, cors: bool) -> Answer {
        Answer {
            status,
            body,
            headers,
            cors,
        }
    }

    fn json(status: u16, body: Value) -> Answer {
        Answer::new(status, Some(body), Vec::new(), true)
    }
}

impl From<ApiError> for Answer {
    fn from(error: ApiError) -> Answer {
        Answer::json(error.status, error.body())
    }
}

/// The request's target is one hapi cannot read as a URL (it starts with `*`): 400
/// "Invalid URL", once the whitelist let the request through. The server marks such a request.
#[derive(Debug, Clone, Copy)]
pub(crate) struct InvalidUrl;

/// What an answer's headers depend on in its request.
struct Asked {
    /// The request's `Origin`.
    origin: Option<String>,
    /// Whether it is a `GET`: hapi offers byte ranges of a `GET`'s answer only.
    get: bool,
    /// Whether it is a `HEAD`, whose answer Node sends without a body.
    head: bool,
    /// Its `Range`, unless an `If-Range` makes hapi ignore it (no answer has an entity tag); an
    /// empty one is none.
    range: Option<String>,
}

impl Asked {
    fn of(request: &Request<Body>) -> Asked {
        let headers = request.headers();
        let given = |name: HeaderName| node_header(headers, &name).filter(|v| !v.is_empty());
        let conditional = given(header::IF_RANGE).is_some();
        Asked {
            origin: node_header(headers, &header::ORIGIN),
            get: request.method() == Verb::GET,
            head: request.method() == Verb::HEAD,
            range: given(header::RANGE).filter(|_| !conditional),
        }
    }
}

/// The header `name` of `headers` as Node's server gives it to hapi: its bytes read as Latin-1,
/// and the values of a repeated header joined with `, ` (the first kept, for the headers Node does
/// not join).
pub(crate) fn node_header(headers: &HeaderMap, name: &HeaderName) -> Option<String> {
    let latin1 = |value: &HeaderValue| value.as_bytes().iter().map(|&b| char::from(b)).collect();
    let mut values = headers.get_all(name).iter();
    let mut text: String = latin1(values.next()?);
    if !FIRST_ONLY.contains(&name.as_str()) {
        for value in values {
            text.push_str(", ");
            text.push_str(&latin1(value));
        }
    }
    Some(text)
}

/// JavaScript's white space (`\s`) in the Latin-1 range, where a header's characters are.
fn is_js_space(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\u{b}' | '\u{c}' | '\r' | ' ' | '\u{a0}')
}

/// `text`, whose characters are Latin-1, as the bytes Node writes it with in a header: Latin-1,
/// or UTF-8 when the answer's body is written with its head (text, which hapi's JSON bodies are).
fn header_bytes(text: &str, with_text: bool) -> Option<HeaderValue> {
    let latin1 = || {
        text.chars()
            .map(|c| u8::try_from(c).unwrap_or(b'?'))
            .collect::<Vec<u8>>()
    };
    let bytes = match with_text {
        true => text.as_bytes().to_vec(),
        false => latin1(),
    };
    HeaderValue::from_bytes(&bytes).ok()
}

/// The routes, the host and the plugins' state: what serves a request.
pub(crate) struct Pipeline {
    routes: Vec<RouteId>,
    host: Host,
    grammar: Grammar,
    limiter: Mutex<RateLimiter>,
    gates: Gates,
}

impl Pipeline {
    /// The pipeline of `routes` over `context`.
    pub(crate) fn new(routes: Vec<Route>, context: Arc<ApiContext>) -> Pipeline {
        let ids = routes.iter().map(|route| route.id).collect();
        let tip = Arc::clone(&context);
        let host = Host {
            config: Arc::clone(&context.config),
            height: Box::new(move || tip.state().height()),
            dispatch: Box::new(move |id, request| {
                match routes.iter().find(|route| route.id == id) {
                    Some(route) => (route.handler)(Arc::clone(&context), request),
                    None => Box::pin(async { Err(not_found()) }),
                }
            }),
        };
        Pipeline::with_parts(ids, host, Grammar::SCHEMAS)
    }

    /// The pipeline of the routes `routes` of `host`, reading requests with `grammar`.
    pub(crate) fn with_parts(routes: Vec<RouteId>, host: Host, grammar: Grammar) -> Pipeline {
        let limit = &host.config.rate_limit;
        let limiter = Mutex::new(RateLimiter::new(limit.points, limit.duration_s));
        let gates = Gates::new(&host.config.semaphore);
        Pipeline {
            routes,
            host,
            grammar,
            limiter,
            gates,
        }
    }

    /// Serve `request` from `peer`.
    pub(crate) async fn handle(&self, request: Request<Body>, peer: SocketAddr) -> Response<Body> {
        let asked = Asked::of(&request);
        let target = format!("{} {}", request.method(), request.uri().path());
        let answer = self.run(request, peer).await.unwrap_or_else(Answer::from);
        if answer.status >= 500 {
            heartwood_kernel::error!("Public API: {target} answered {}", answer.status);
        }
        self.respond(answer, &asked)
    }

    async fn run(&self, request: Request<Body>, peer: SocketAddr) -> Result<Answer, ApiError> {
        let config = &self.host.config;
        let (parts, body) = request.into_parts();
        let client = client_address(&parts.headers, peer, config.trust_proxy);
        if !ip::is_whitelisted(&config.whitelist, &client) {
            return Err(ApiError::forbidden("Forbidden"));
        }
        if parts.extensions.get::<InvalidUrl>().is_some() {
            return Err(ApiError::bad_request("Invalid URL"));
        }
        // The reference's query plugins run before its router and its rate limit: a query they
        // throw on is a 500 on any path, and is not charged. The query is parsed again once the
        // request is charged, so a request whose body is slow to come holds only its text.
        let grammar = &self.grammar;
        let raw = parts.uri.query().unwrap_or("");
        if (grammar.parse)(raw).is_null() {
            return Err(ApiError::internal());
        }
        let path = parts.uri.path();
        let path = path.strip_suffix('/').filter(|_| path.len() > 1);
        let path = path.unwrap_or(parts.uri.path());
        let method = match parts.method {
            // hapi's preflight route has CORS off: its refusals carry no CORS headers.
            Verb::OPTIONS => {
                let refused = |error: ApiError| {
                    Answer::new(error.status, Some(error.body()), Vec::new(), false)
                };
                let full = parts.uri.path();
                let answer = self.preflight(&parts.headers, path, full);
                return Ok(answer.unwrap_or_else(refused));
            }
            Verb::GET | Verb::HEAD => Method::Get,
            Verb::POST => Method::Post,
            _ => return Err(not_found()),
        };
        let Some((route, params)) = self.lookup(method, path)? else {
            // The reference's catch-all route (its documentation's) takes a GET or HEAD that no
            // route matches, anywhere.
            if method == Method::Post {
                return Err(not_found());
            }
            let answer = catch_all::answer(parts.uri.path());
            if answer == CatchAll::Undecodable {
                return Err(ApiError::bad_request("Bad Request"));
            }
            if config.rate_limit.enabled && !self.admit(&client) {
                return Err(ApiError::too_many_requests("Too Many Requests"));
            }
            return Err(match answer {
                CatchAll::Forbidden => ApiError::forbidden("Forbidden"),
                CatchAll::Undecodable | CatchAll::NotFound => not_found(),
            });
        };
        let payload = match method {
            Method::Post => Some(payload::read(&parts.headers, body).await?),
            Method::Get => None,
        };
        if config.rate_limit.enabled && !self.admit(&client) {
            return Err(ApiError::too_many_requests("Too Many Requests"));
        }
        (grammar.params)(route, &params)?;
        let query = (grammar.query)(route, &(grammar.parse)(raw))?;
        let payload = payload.map(|payload| (grammar.payload)(route, &payload));
        let payload = payload.transpose()?;
        let _permit = self.gates.enter(route, &query).await?;
        // The pagination links are relative to the base path.
        let path = path.strip_prefix(config.base_path.as_str()).unwrap_or(path);
        let path = path.to_owned();
        let client = client.parse::<IpAddr>().unwrap_or(peer.ip());
        let request = ApiRequest {
            route,
            params,
            path,
            query,
            payload,
            client,
        };
        let reply = (self.host.dispatch)(route, request).await?;
        Ok(Answer::json(reply.status, reply.body))
    }

    /// The route of `method` at `path` under the base path, and its decoded parameters: a literal
    /// segment wins over a parameter. `Ok(None)` when none matches, or when the path is one of a
    /// route of the reference that is not served here; 400 for a parameter that is not
    /// percent-encoded UTF-8.
    fn lookup(&self, method: Method, path: &str) -> Result<Option<(RouteId, Params)>, ApiError> {
        let Some(rest) = path.strip_prefix(self.host.config.base_path.as_str()) else {
            return Ok(None);
        };
        if method == Method::Get && UNSERVED.contains(&rest) {
            return Ok(None);
        }
        let segments: Vec<&str> = rest.split('/').collect();
        let routes = self.routes.iter().filter(|route| route.method() == method);
        let matched = routes.filter_map(|&route| Some((route, matches(route, &segments)?)));
        let Some((route, (_, raw))) = matched.max_by_key(|(_, (literals, _))| *literals) else {
            return Ok(None);
        };
        let decode =
            |(name, value): (&str, &str)| Some((name.into(), percent_decode(value)?.into()));
        let params = raw.into_iter().map(decode).collect::<Option<Params>>();
        let params = params.ok_or_else(|| ApiError::bad_request("Bad Request"))?;
        Ok(Some((route, params)))
    }

    /// A CORS preflight, as hapi answers it for a route with CORS on.
    fn preflight(&self, headers: &HeaderMap, path: &str, full: &str) -> Result<Answer, ApiError> {
        let missing = |name: &str| format!("CORS error: Missing {name} header");
        // hapi tests these headers for truth: an empty value is a missing one.
        let given = |name: HeaderName| node_header(headers, &name).filter(|v| !v.is_empty());
        let Some(requested) = given(header::ACCESS_CONTROL_REQUEST_METHOD) else {
            return Err(ApiError::not_found(missing(
                "Access-Control-Request-Method",
            )));
        };
        let method = requested.to_ascii_lowercase();
        let method = match method.as_str() {
            "get" | "head" => Method::Get,
            "post" => Method::Post,
            "options" => return Ok(message("CORS is disabled for this route")),
            _ => return Err(not_found()),
        };
        // The route hapi finds for the method asked: a GET or HEAD that no route matches finds the
        // catch-all route, which has CORS on. A path or a parameter that does not decode fails
        // hapi's assertion that the route it finds is not its bad-request route: a 500.
        match self.lookup(method, path) {
            Ok(Some(_)) => {}
            Ok(None) if method == Method::Get => {
                if catch_all::answer(full) == CatchAll::Undecodable {
                    return Err(ApiError::internal());
                }
            }
            Ok(None) => return Err(not_found()),
            Err(_) => return Err(ApiError::internal()),
        }
        let Some(origin) = given(header::ORIGIN) else {
            return Err(ApiError::not_found(missing("Origin")));
        };
        let asked = node_header(headers, &header::ACCESS_CONTROL_REQUEST_HEADERS);
        let asked = asked.unwrap_or_default().to_ascii_lowercase();
        if !asked.is_empty() && !allowed_headers(&asked) {
            return Ok(message("CORS error: Some headers are not allowed"));
        }
        let fixed = HeaderValue::from_static;
        let echo = |text: &str| header_bytes(text, false).unwrap_or(fixed(""));
        let headers = vec![
            (header::ACCESS_CONTROL_ALLOW_ORIGIN, echo(&origin)),
            (header::ACCESS_CONTROL_ALLOW_METHODS, echo(&requested)),
            (
                header::ACCESS_CONTROL_ALLOW_HEADERS,
                fixed("Accept,Authorization,Content-Type,If-None-Match"),
            ),
            (header::ACCESS_CONTROL_MAX_AGE, fixed("86400")),
            (header::ACCESS_CONTROL_EXPOSE_HEADERS, fixed(EXPOSED)),
        ];
        Ok(Answer::new(204, None, headers, false))
    }

    /// Charge a point to `client`, unless it is on the limit's whitelist; `false` when it is on
    /// the blacklist or over its limit.
    fn admit(&self, client: &str) -> bool {
        let limit = &self.host.config.rate_limit;
        let listed = |list: &[String]| !list.is_empty() && ip::is_whitelisted(list, client);
        if listed(&limit.whitelist) || listed(&limit.blacklist) {
            return listed(&limit.whitelist);
        }
        let mut limiter = self.limiter.lock().unwrap_or_else(PoisonError::into_inner);
        limiter.consume(client, Instant::now())
    }

    /// The response of `answer` to the request `asked`, with the headers every response gets.
    fn respond(&self, answer: Answer, asked: &Asked) -> Response<Body> {
        let Answer {
            mut status,
            body,
            headers: mut fixed,
            cors,
        } = answer;
        let mut body = body.map(|body| js::stringify(&body, 0).into_bytes());
        // Node writes the head with the first chunk of a body given as text, in UTF-8.
        let mut with_text = body.is_some() && !asked.head;
        let json = body.as_ref().map(|_| "application/json; charset=utf-8");
        // hapi varies an answer it could compress (1 KiB or more) by `accept-encoding` too; the
        // whole answer counts, before a range is cut from it.
        let mut large = body.as_ref().is_some_and(|body| body.len() >= 1024);
        if let Some(whole) = body.take_if(|body| asked.get && status == 200 && !body.is_empty()) {
            let (served, part, headers) = ranges::serve(asked.range.as_deref(), whole);
            large &= served != 416;
            with_text &= served != 206;
            (status, body) = (served, Some(part));
            fixed.extend(headers);
        }
        let mut response = Response::new(body.map_or_else(Body::empty, Body::from));
        *response.status_mut() =
            StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        let mut add = |name: HeaderName, value: &'static str| {
            fixed.push((name, HeaderValue::from_static(value)));
        };
        if let Some(json) = json {
            add(header::CONTENT_TYPE, json);
        }
        add(header::CACHE_CONTROL, "no-cache");
        if cors {
            add(
                header::VARY,
                if large {
                    "origin,accept-encoding"
                } else {
                    "origin"
                },
            );
            add(header::ACCESS_CONTROL_EXPOSE_HEADERS, EXPOSED);
            let origin = asked.origin.as_deref();
            if let Some(origin) = origin.and_then(|origin| header_bytes(origin, with_text)) {
                fixed.push((header::ACCESS_CONTROL_ALLOW_ORIGIN, origin));
            }
        }
        let height = HeaderValue::from((self.host.height)());
        fixed.push((HeaderName::from_static("x-block-height"), height));
        fixed.push((header::CONNECTION, HeaderValue::from_static("close")));
        response.headers_mut().extend(fixed);
        response
    }
}

/// The paths of the reference's routes that are not served here and that a served route with a
/// parameter would take (`/blocks/{id}`, `/transactions/{id}`, `/wallets/{id}`): they are unmatched
/// paths, as the paths of the other routes that are not served are.
const UNSERVED: [&str; 4] = [
    "/blocks/first",
    "/blocks/missed",
    "/transactions/schemas",
    "/wallets/top",
];

fn not_found() -> ApiError {
    ApiError::not_found("Not Found")
}

/// Whether hapi allows the headers a preflight asks for, `asked` in lower case: split at its
/// commas with the white space around them, each must be an allowed header, and none twice.
fn allowed_headers(asked: &str) -> bool {
    let names: Vec<&str> = asked.split(',').collect();
    let last = names.len().saturating_sub(1);
    let names = names.iter().enumerate().map(|(at, name)| match at {
        0 if last == 0 => *name,
        0 => name.trim_end_matches(is_js_space),
        _ if at == last => name.trim_start_matches(is_js_space),
        _ => name.trim_matches(is_js_space),
    });
    let mut seen = Vec::new();
    for name in names {
        if !ALLOWED.contains(&name) || seen.contains(&name) {
            return false;
        }
        seen.push(name);
    }
    true
}

/// A 200 answer of `{message}`, without the route's CORS headers.
fn message(text: &str) -> Answer {
    Answer::new(200, Some(json!({ "message": text })), Vec::new(), false)
}

/// Whether `route` matches the path's `segments`: its literal segments' count and its parameters'
/// raw values.
fn matches<'p>(
    route: RouteId,
    segments: &[&'p str],
) -> Option<(usize, Vec<(&'static str, &'p str)>)> {
    let pattern: Vec<&'static str> = route.path().split('/').collect();
    let mut params = Vec::new();
    for (part, &segment) in pattern.iter().zip(segments) {
        let name = part.strip_prefix('{').and_then(|n| n.strip_suffix('}'));
        match name {
            Some(name) if !segment.is_empty() => params.push((name, segment)),
            None if *part == segment => {}
            _ => return None,
        }
    }
    (pattern.len() == segments.len()).then(|| (pattern.len() - params.len(), params))
}

/// The client's address as the reference's rate limit and whitelist key it: the socket's, or with
/// `trust_proxy` the first entry of `X-Forwarded-For` when the request has one.
fn client_address(headers: &HeaderMap, peer: SocketAddr, trust_proxy: bool) -> String {
    let forwarded = HeaderName::from_static("x-forwarded-for");
    let Some(forwarded) = node_header(headers, &forwarded).filter(|_| trust_proxy) else {
        return peer.ip().to_string();
    };
    let first = forwarded.split(',').next();
    first.unwrap_or_default().trim().to_owned()
}

/// `text` with its `%XX` escapes decoded, as JavaScript's `decodeURIComponent` decodes them;
/// `None` for an escape that is not two hex digits or bytes that are not UTF-8.
pub(crate) fn percent_decode(text: &str) -> Option<String> {
    let mut parts = text.split('%');
    let mut bytes = parts.next().unwrap_or_default().as_bytes().to_vec();
    for part in parts {
        bytes.extend(hex::decode(part.get(..2)?).ok()?);
        bytes.extend_from_slice(part.get(2..)?.as_bytes());
    }
    String::from_utf8(bytes).ok()
}
