//! The plugins over a fake host and a fake grammar: the lifecycle's order, the answers' headers,
//! CORS, the router's rules, the payload, the rate limit and the semaphore (tokio's paused clock),
//! and the accept loop over a real socket.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::body::{Body, to_bytes};
use axum::http::{Request, Response};
use heartwood_kernel::config::{Env, Options};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{Notify, watch};

use super::{Grammar, Host, Pipeline};
use crate::context::{ApiReply, ApiRequest, HandlerFuture, RouteId};
use crate::errors::ApiError;
use crate::server::{self, ApiConfig};

const CLIENT: &str = "127.0.0.1:50000";

fn config() -> ApiConfig {
    ApiConfig::from_options(&json!({}), &Env::default()).unwrap()
}

/// A query as `a=1&b=2`, each value as text; `bad` fails the route's query schema.
const GRAMMAR: Grammar = Grammar {
    parse: |raw| {
        let pairs = raw.split('&').filter(|pair| !pair.is_empty());
        let pairs = pairs.map(|pair| {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            (key.to_owned(), json!(value))
        });
        Value::Object(pairs.collect())
    },
    params: |_, _| Ok(()),
    query: |_, query| match query.get("bad") {
        Some(_) => Err(ApiError::unprocessable("\"bad\" is not allowed")),
        None => Ok(query.clone()),
    },
    payload: |_, payload| Ok(payload.clone()),
};

/// The request as the handler saw it.
fn echo(route: RouteId, request: ApiRequest) -> HandlerFuture {
    Box::pin(async move {
        Ok(ApiReply::data(json!({
            "route": format!("{route:?}"),
            "params": request.params,
            "query": request.query,
            "payload": request.payload,
            "client": request.client.to_string(),
            "path": request.path,
        })))
    })
}

fn pipeline_with(
    config: ApiConfig,
    dispatch: impl Fn(RouteId, ApiRequest) -> HandlerFuture + Send + Sync + 'static,
) -> Pipeline {
    let host = Host {
        config: Arc::new(config),
        height: Box::new(|| 265),
        dispatch: Box::new(dispatch),
    };
    Pipeline::with_parts(RouteId::ALL.to_vec(), host, GRAMMAR)
}

fn pipeline(config: ApiConfig) -> Pipeline {
    pipeline_with(config, echo)
}

fn get(uri: &str) -> Request<Body> {
    Request::get(uri).body(Body::empty()).unwrap()
}

/// The status, the headers as text and the body of `response`.
async fn read(response: Response<Body>) -> (u16, Vec<(String, String)>, Value) {
    let status = response.status().as_u16();
    let headers = (response.headers().iter())
        .map(|(name, value)| (name.to_string(), value.to_str().unwrap().to_owned()))
        .collect();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body = match bytes.is_empty() {
        true => Value::Null,
        false => serde_json::from_slice(&bytes).unwrap(),
    };
    (status, headers, body)
}

async fn call(pipeline: &Pipeline, request: Request<Body>) -> (u16, Vec<(String, String)>, Value) {
    read(pipeline.handle(request, CLIENT.parse().unwrap()).await).await
}

fn header<'h>(headers: &'h [(String, String)], name: &str) -> Option<&'h str> {
    let found = headers.iter().find(|(key, _)| key == name);
    found.map(|(_, value)| value.as_str())
}

fn boom(status: u16, error: &str, message: &str) -> Value {
    json!({ "statusCode": status, "error": error, "message": message })
}

#[tokio::test]
async fn an_answer_has_the_reference_headers() {
    let pipeline = pipeline(config());
    let (status, headers, body) = call(&pipeline, get("/api/node/status?a=1")).await;
    assert_eq!(status, 200);
    assert_eq!(body["data"]["route"], "NodeStatus");
    assert_eq!(body["data"]["query"], json!({ "a": "1" }));
    assert_eq!(body["data"]["path"], "/api/node/status");
    assert_eq!(body["data"]["client"], "127.0.0.1");
    let expected = [
        ("content-type", "application/json; charset=utf-8"),
        ("cache-control", "no-cache"),
        ("vary", "origin"),
        ("x-block-height", "265"),
        ("connection", "close"),
    ];
    for (name, value) in expected {
        assert_eq!(header(&headers, name), Some(value), "{name}");
    }
    assert_eq!(header(&headers, "access-control-allow-origin"), None);
    assert_eq!(header(&headers, "retry-after"), None);
}

#[tokio::test]
async fn cors_headers_answer_a_request_with_an_origin_errors_included() {
    let pipeline = pipeline(config());
    for uri in ["/api/node/status", "/api/nothing"] {
        let request = Request::get(uri)
            .header("origin", "https://wallet.example")
            .body(Body::empty())
            .unwrap();
        let (_, headers, _) = call(&pipeline, request).await;
        let allow = header(&headers, "access-control-allow-origin");
        assert_eq!(allow, Some("https://wallet.example"), "{uri}");
        let exposed = header(&headers, "access-control-expose-headers");
        assert_eq!(exposed, Some("WWW-Authenticate,Server-Authorization"));
        assert_eq!(header(&headers, "x-block-height"), Some("265"));
    }
}

#[tokio::test]
async fn a_preflight_is_answered_as_hapi_answers_it() {
    let pipeline = pipeline(config());
    let preflight = |uri: &str, headers: &[(&str, &str)]| {
        let mut request = Request::options(uri);
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        request.body(Body::empty()).unwrap()
    };
    let origin = ("origin", "https://wallet.example");
    let method = ("access-control-request-method", "POST");
    let (status, headers, body) =
        call(&pipeline, preflight("/api/transactions", &[origin, method])).await;
    assert_eq!((status, body), (204, Value::Null));
    let expected = [
        ("access-control-allow-origin", "https://wallet.example"),
        ("access-control-allow-methods", "POST"),
        (
            "access-control-allow-headers",
            "Accept,Authorization,Content-Type,If-None-Match",
        ),
        ("access-control-max-age", "86400"),
        (
            "access-control-expose-headers",
            "WWW-Authenticate,Server-Authorization",
        ),
        ("connection", "close"),
    ];
    for (name, value) in expected {
        assert_eq!(header(&headers, name), Some(value), "{name}");
    }
    assert_eq!(header(&headers, "vary"), None);
    let cases = [
        (
            preflight("/api/transactions", &[origin]),
            404,
            boom(
                404,
                "Not Found",
                "CORS error: Missing Access-Control-Request-Method header",
            ),
        ),
        (
            preflight("/api/transactions", &[method]),
            404,
            boom(404, "Not Found", "CORS error: Missing Origin header"),
        ),
        (
            preflight("/api/nothing", &[origin, method]),
            404,
            boom(404, "Not Found", "Not Found"),
        ),
        (
            preflight(
                "/api/transactions",
                &[
                    origin,
                    method,
                    ("access-control-request-headers", "x-custom"),
                ],
            ),
            200,
            json!({ "message": "CORS error: Some headers are not allowed" }),
        ),
    ];
    for (request, status, body) in cases {
        let (answered, _, answer) = call(&pipeline, request).await;
        assert_eq!((answered, answer), (status, body));
    }
    let allowed = ("access-control-request-headers", "Content-Type, accept");
    assert_eq!(
        call(
            &pipeline,
            preflight("/api/transactions", &[origin, method, allowed])
        )
        .await
        .0,
        204
    );
}

#[tokio::test]
async fn the_router_follows_the_reference_rules() {
    let pipeline = pipeline(config());
    let route = |body: &Value| {
        body["data"]["route"]
            .as_str()
            .unwrap_or_default()
            .to_owned()
    };
    // A trailing slash is stripped once; a literal segment wins over a parameter.
    let cases = [
        ("/api/node/status/", "NodeStatus"),
        ("/api/transactions/unconfirmed", "TransactionsUnconfirmed"),
        ("/api/transactions/types/", "TransactionsTypes"),
        ("/api/blocks/last", "BlocksLast"),
        ("/api/blocks/12", "BlocksShow"),
        ("/api/transactions/abc", "TransactionsShow"),
        (
            "/api/wallets/genesis%5F34/transactions",
            "WalletsTransactions",
        ),
    ];
    for (uri, expected) in cases {
        let (status, _, body) = call(&pipeline, get(uri)).await;
        assert_eq!((status, route(&body)), (200, expected.to_owned()), "{uri}");
    }
    let (_, _, body) = call(&pipeline, get("/api/wallets/genesis%5F34")).await;
    assert_eq!(body["data"]["params"], json!({ "id": "genesis_34" }));
    // Unknown paths, and another method on a known path, are 404 (never 405).
    let not_found = boom(404, "Not Found", "Not Found");
    for request in [
        get("/api/node/status//"),
        get("/api/nothing"),
        get("/api"),
        get("/node/status"),
        get("/api/blocks//transactions"),
        Request::delete("/api/node/status")
            .body(Body::empty())
            .unwrap(),
        Request::post("/api/node/status")
            .body(Body::empty())
            .unwrap(),
    ] {
        let target = request.uri().to_string();
        let (status, headers, body) = call(&pipeline, request).await;
        assert_eq!((status, &body), (404, &not_found), "{target}");
        assert_eq!(header(&headers, "x-block-height"), Some("265"));
    }
    // HEAD is GET; a parameter that is not percent-encoded UTF-8 is a bad request.
    let head = Request::head("/api/node/syncing")
        .body(Body::empty())
        .unwrap();
    assert_eq!(route(&call(&pipeline, head).await.2), "NodeSyncing");
    let bad = call(&pipeline, get("/api/wallets/%zz")).await;
    assert_eq!(
        (bad.0, bad.2),
        (400, boom(400, "Bad Request", "Bad Request"))
    );
    // A handler's error is Boom's body.
    let failing = pipeline_with(config(), |_, _| {
        Box::pin(async { Err(ApiError::not_found("Peer not found")) })
    });
    let (status, _, body) = call(&failing, get("/api/peers/203.0.113.1")).await;
    assert_eq!(
        (status, body),
        (404, boom(404, "Not Found", "Peer not found"))
    );
}

#[tokio::test]
async fn the_whitelist_refuses_before_the_router() {
    let mut config = config();
    config.whitelist = vec!["10.0.0.*".to_owned()];
    let refusing = pipeline(config.clone());
    let forbidden = boom(403, "Forbidden", "Forbidden");
    for uri in ["/api/node/status", "/api/nothing"] {
        let (status, headers, body) = call(&refusing, get(uri)).await;
        assert_eq!((status, body), (403, forbidden.clone()), "{uri}");
        assert_eq!(header(&headers, "connection"), Some("close"));
    }
    // Behind a trusted proxy the first forwarded address is the client's.
    config.trust_proxy = true;
    let proxied = pipeline(config);
    let request = Request::get("/api/node/status")
        .header("x-forwarded-for", " 10.0.0.5 , 198.51.100.1")
        .body(Body::empty())
        .unwrap();
    let (status, _, body) = call(&proxied, request).await;
    assert_eq!((status, &body["data"]["client"]), (200, &json!("10.0.0.5")));
}

fn post(body: impl Into<Body>, headers: &[(&str, &str)]) -> Request<Body> {
    let mut request = Request::post("/api/transactions");
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    request.body(body.into()).unwrap()
}

#[tokio::test]
async fn a_payload_is_read_as_hapi_reads_it() {
    let pipeline = pipeline(config());
    let json = ("content-type", "application/json");
    let (status, _, body) = call(&pipeline, post(r#"{"transactions":[{"a":1}]}"#, &[json])).await;
    assert_eq!(
        (status, &body["data"]["payload"]),
        (200, &json!({ "transactions": [{ "a": 1 }] }))
    );
    // No content type is JSON; an empty body is null; text is text.
    let (_, _, body) = call(&pipeline, post(r#"{"b":2}"#, &[])).await;
    assert_eq!(body["data"]["payload"], json!({ "b": 2 }));
    let (_, _, body) = call(&pipeline, post("", &[json])).await;
    assert_eq!(body["data"]["payload"], Value::Null);
    let (_, _, body) = call(&pipeline, post("words", &[("content-type", "text/plain")])).await;
    assert_eq!(body["data"]["payload"], "words");
    let too_large = boom(
        413,
        "Payload Too Large",
        "Payload content length greater than maximum allowed: 1048576",
    );
    let cases = [
        (
            post("{", &[json]),
            boom(400, "Bad Request", "Invalid request payload JSON format"),
        ),
        (
            post(r#"{"a":{"__proto__":{}}}"#, &[json]),
            boom(400, "Bad Request", "Invalid request payload JSON format"),
        ),
        (
            post("x", &[("content-type", "image/png")]),
            boom(415, "Unsupported Media Type", "Unsupported Media Type"),
        ),
        (
            post("x", &[json, ("content-length", "1048577")]),
            too_large.clone(),
        ),
        (post(vec![b' '; 1_048_577], &[json]), too_large),
    ];
    for (request, expected) in cases {
        let (status, _, body) = call(&pipeline, request).await;
        assert_eq!(status, expected["statusCode"]);
        assert_eq!(body, expected);
    }
}

#[tokio::test(start_paused = true)]
async fn the_rate_limit_charges_every_routed_request_per_window() {
    let mut config = config();
    config.rate_limit.points = 3;
    let pipeline = pipeline(config);
    // An unknown path costs nothing; a request the schema refuses costs a point.
    for _ in 0..5 {
        assert_eq!(call(&pipeline, get("/api/nothing")).await.0, 404);
    }
    assert_eq!(call(&pipeline, get("/api/node/status?bad=1")).await.0, 422);
    for _ in 0..2 {
        assert_eq!(call(&pipeline, get("/api/node/status")).await.0, 200);
    }
    let (status, headers, body) = call(&pipeline, get("/api/node/status")).await;
    assert_eq!(
        (status, body),
        (429, boom(429, "Too Many Requests", "Too Many Requests"))
    );
    assert_eq!(header(&headers, "retry-after"), None);
    // The window started at the first point: it ends 60 s later, and a new one starts.
    tokio::time::advance(Duration::from_secs(59)).await;
    assert_eq!(call(&pipeline, get("/api/node/status")).await.0, 429);
    tokio::time::advance(Duration::from_secs(1)).await;
    assert_eq!(call(&pipeline, get("/api/node/status")).await.0, 200);
}

#[tokio::test]
async fn the_rate_limit_lists_and_switch() {
    let mut config = config();
    config.rate_limit.points = 1;
    config.rate_limit.whitelist = vec!["127.0.0.*".to_owned()];
    let white = pipeline(config.clone());
    for _ in 0..5 {
        assert_eq!(call(&white, get("/api/node/status")).await.0, 200);
    }
    config.rate_limit.whitelist = Vec::new();
    config.rate_limit.blacklist = vec!["127.0.0.1".to_owned()];
    let black = pipeline(config.clone());
    assert_eq!(call(&black, get("/api/node/status")).await.0, 429);
    config.rate_limit.enabled = false;
    let off = pipeline(config);
    for _ in 0..5 {
        assert_eq!(call(&off, get("/api/node/status")).await.0, 200);
    }
}

#[tokio::test]
async fn the_semaphore_bounds_the_store_routes_at_their_level() {
    let mut config = config();
    let [first, second] = &mut config.semaphore.database;
    (first.concurrency, first.queue_limit) = (1, 1);
    (second.concurrency, second.queue_limit) = (1, 0);
    let release = Arc::new(Notify::new());
    let held = Arc::clone(&release);
    let pipeline = Arc::new(pipeline_with(config, move |route, request| {
        let held = Arc::clone(&held);
        Box::pin(async move {
            if request.query.get("hold").is_some() {
                held.notified().await;
            }
            echo(route, request).await
        })
    }));
    let spawn = |uri: &'static str| {
        let pipeline = Arc::clone(&pipeline);
        tokio::spawn(async move { call(&pipeline, get(uri)).await.0 })
    };
    let settle = || tokio::time::sleep(Duration::from_millis(20));
    // One place and one waiting at the first level: the next is refused.
    let served = spawn("/api/transactions/abc?hold=1");
    settle().await;
    let waiting = spawn("/api/transactions/abc");
    settle().await;
    let (status, _, body) = call(&pipeline, get("/api/transactions/abc")).await;
    assert_eq!(
        (status, body),
        (429, boom(429, "Too Many Requests", "Too Many Requests"))
    );
    // A large offset takes the second level; a route without a semaphore is not bounded.
    assert_eq!(
        call(&pipeline, get("/api/transactions/abc?offset=10001"))
            .await
            .0,
        200
    );
    assert_eq!(call(&pipeline, get("/api/node/status")).await.0, 200);
    release.notify_one();
    assert_eq!((served.await.unwrap(), waiting.await.unwrap()), (200, 200));
    // Freed again.
    assert_eq!(call(&pipeline, get("/api/transactions/abc")).await.0, 200);
}

#[tokio::test]
async fn the_semaphore_levels_follow_the_transaction_indexes() {
    let mut config = config();
    let [first, second] = &mut config.semaphore.database;
    (first.concurrency, first.queue_limit) = (0, 0);
    (second.concurrency, second.queue_limit) = (1, 0);
    let pipeline = pipeline(config.clone());
    let status = |uri: &'static str| {
        let pipeline = &pipeline;
        async move { call(pipeline, get(uri)).await.0 }
    };
    // The first level has no place here: 429 means level one, 200 level two.
    let cases = [
        ("/api/transactions", 429),
        ("/api/transactions?senderPublicKey=02ab", 429),
        ("/api/transactions?memo=x", 200),
        ("/api/transactions?orderBy=amount:desc", 200),
        ("/api/transactions?orderBy=nonce:desc", 429),
        ("/api/transactions?page=101&limit=100", 200),
        ("/api/transactions?memo=x&blockId=abc", 429),
        ("/api/transactions?blockId=abc&offset=10001", 429),
        ("/api/wallets/genesis_1/transactions?fee=1", 429),
        ("/api/wallets/genesis_1/transactions?senderId=x", 200),
        ("/api/blocks/5?memo=x", 429),
        ("/api/blocks/5?offset=10001", 200),
    ];
    for (uri, expected) in cases {
        assert_eq!(status(uri).await, expected, "{uri}");
    }
    config.semaphore.enabled = false;
    let off = pipeline_with(config, echo);
    assert_eq!(call(&off, get("/api/transactions")).await.0, 200);
}

#[tokio::test]
async fn a_connection_answers_one_request_then_closes() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address: SocketAddr = listener.local_addr().unwrap();
    let pipeline = Arc::new(pipeline(config()));
    let (stop, stopped) = watch::channel(false);
    let timeout = Duration::from_millis(300);
    let task = tokio::spawn(server::accept(listener, pipeline, stopped, timeout));
    let mut client = tokio::net::TcpStream::connect(address).await.unwrap();
    let request =
        "GET /api/node/status HTTP/1.1\r\nhost: node\r\n\r\nGET /api/node/status HTTP/1.1\r\n\r\n";
    client.write_all(request.as_bytes()).await.unwrap();
    let mut answer = String::new();
    client.read_to_string(&mut answer).await.unwrap();
    assert!(answer.starts_with("HTTP/1.1 200 OK\r\n"), "{answer}");
    assert!(answer.contains("connection: close\r\n"), "{answer}");
    assert_eq!(answer.matches("HTTP/1.1").count(), 1, "{answer}");
    // A client that sends nothing is closed after the socket timeout.
    let mut silent = tokio::net::TcpStream::connect(address).await.unwrap();
    let mut buffer = Vec::new();
    let closed = tokio::time::timeout(Duration::from_secs(5), silent.read_to_end(&mut buffer));
    assert!(closed.await.is_ok());
    stop.send(true).unwrap();
    task.await.unwrap();
    assert!(tokio::net::TcpStream::connect(address).await.is_err());
}

#[test]
fn the_options_are_the_reference_defaults_and_its_variables() {
    let defaults = config();
    assert!(defaults.enabled && !defaults.https && !defaults.trust_proxy);
    assert_eq!(defaults.address, "0.0.0.0:4003".parse().unwrap());
    assert_eq!(
        (defaults.rate_limit.points, defaults.rate_limit.duration_s),
        (100, 60)
    );
    assert_eq!(defaults.whitelist, ["*"]);
    assert_eq!(
        (defaults.socket_timeout_ms, defaults.base_path.as_str()),
        (5000, "/api")
    );
    let [first, second] = defaults.semaphore.database;
    assert_eq!(
        (first.concurrency, first.queue_limit, first.max_offset),
        (10, 100, Some(10_000))
    );
    assert_eq!(
        (second.concurrency, second.queue_limit, second.max_offset),
        (1, 5, None)
    );
    let env = Env::from_vars(
        [
            ("CORE_API_PORT", "6003"),
            ("CORE_API_HOST", "127.0.0.1"),
            ("CORE_API_DISABLED", "false"),
            ("CORE_API_RATE_LIMIT_USER_LIMIT", "250"),
            ("CORE_API_RATE_LIMIT_WHITELIST", "10.0.0.1,10.0.0.2"),
            ("CORE_API_SSL", "1"),
            ("CORE_API_TRUST_PROXY", ""),
        ]
        .map(|(name, value)| (name.to_owned(), value.to_owned())),
    );
    let read = ApiConfig::from_options(&json!({}), &env).unwrap();
    // Any non-empty value turns a switch on, `false` included; an empty one is unset.
    assert!(!read.enabled && read.https && !read.trust_proxy);
    assert_eq!(read.address, "127.0.0.1:6003".parse().unwrap());
    assert_eq!(read.rate_limit.points, 250);
    assert_eq!(read.rate_limit.whitelist, ["10.0.0.1", "10.0.0.2"]);
    // The entry of app.json over the defaults, numbers and booleans from their text.
    let entry = json!({ "plugins": { "rateLimit": { "points": " 7 ", "enabled": "FALSE" } } });
    let merged = ApiConfig::from_options(&entry, &Env::default()).unwrap();
    assert_eq!(
        (merged.rate_limit.points, merged.rate_limit.enabled),
        (7, false)
    );
    for (options, key) in [
        (
            json!({ "server": { "http": { "port": 70_000 } } }),
            "server.http.port",
        ),
        (
            json!({ "server": { "http": { "port": "4003.5" } } }),
            "server.http.port",
        ),
        (
            json!({ "plugins": { "trustProxy": "yes" } }),
            "plugins.trustProxy",
        ),
        (
            json!({ "plugins": { "whitelist": [1] } }),
            "plugins.whitelist",
        ),
        (json!({ "options": { "basePath": "" } }), "options.basePath"),
    ] {
        let refused = ApiConfig::from_options(&options, &Env::default()).unwrap_err();
        assert_eq!(
            (refused.package, refused.key.as_str()),
            ("@solar-network/api", key)
        );
    }
}
