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
use tokio::task::JoinHandle;

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
    assert_eq!(body["data"]["path"], "/node/status");
    assert_eq!(body["data"]["client"], "127.0.0.1");
    let expected = [
        ("content-type", "application/json; charset=utf-8"),
        ("cache-control", "no-cache"),
        ("vary", "origin"),
        (
            "access-control-expose-headers",
            "WWW-Authenticate,Server-Authorization",
        ),
        ("accept-ranges", "bytes"),
        ("x-block-height", "265"),
        ("connection", "close"),
    ];
    for (name, value) in expected {
        assert_eq!(header(&headers, name), Some(value), "{name}");
    }
    assert_eq!(header(&headers, "access-control-allow-origin"), None);
    assert_eq!(header(&headers, "retry-after"), None);
    // An answer hapi could compress also varies by the accepted encoding.
    let large = pipeline_with(config(), |_, _| {
        Box::pin(async { Ok(ApiReply::data(json!("x".repeat(1024)))) })
    });
    let (_, headers, _) = call(&large, get("/api/node/status")).await;
    assert_eq!(header(&headers, "vary"), Some("origin,accept-encoding"));
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
async fn cors_headers_but_the_origin_answer_a_request_without_one() {
    let mut limited = config();
    limited.rate_limit.points = 2;
    let pipeline = pipeline(limited);
    let head = Request::head("/api/node/status")
        .body(Body::empty())
        .unwrap();
    let put = Request::put("/api/node/status")
        .body(Body::empty())
        .unwrap();
    let cases = [
        (get("/api/node/status"), 200, Some("bytes")),
        (get("/api/node/status?bad=1"), 422, None),
        (head, 429, None),
        (get("/api/node/status"), 429, None),
        (put, 404, None),
    ];
    for (request, status, ranges) in cases {
        let target = format!("{} {}", request.method(), request.uri());
        let (answered, headers, _) = call(&pipeline, request).await;
        assert_eq!(answered, status, "{target}");
        let exposed = header(&headers, "access-control-expose-headers");
        assert_eq!(exposed, Some("WWW-Authenticate,Server-Authorization"));
        assert_eq!(header(&headers, "vary"), Some("origin"), "{target}");
        assert_eq!(header(&headers, "access-control-allow-origin"), None);
        assert_eq!(header(&headers, "accept-ranges"), ranges, "{target}");
    }
    // Only a GET's answer offers ranges: not a HEAD's, nor a POST's.
    let pipeline = pipeline_with(config(), echo);
    let head = Request::head("/api/node/status")
        .body(Body::empty())
        .unwrap();
    for request in [head, post("{}", &[])] {
        let (status, headers, _) = call(&pipeline, request).await;
        assert_eq!((status, header(&headers, "accept-ranges")), (200, None));
    }
}

#[tokio::test]
async fn a_range_of_an_answer_is_served_as_hapi_serves_it() {
    let pipeline = pipeline(config());
    let ranged = |range: &str, if_range: Option<&str>| {
        let mut request = Request::get("/api/node/status").header("range", range);
        if let Some(tag) = if_range {
            request = request.header("if-range", tag);
        }
        request.body(Body::empty()).unwrap()
    };
    let exchange = |request: Request<Body>| {
        let pipeline = &pipeline;
        async move {
            let response = pipeline.handle(request, CLIENT.parse().unwrap()).await;
            let status = response.status().as_u16();
            let range = response.headers().get("content-range").cloned();
            let accept = response.headers().get("accept-ranges").cloned();
            let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
            let text = |value: Option<axum::http::HeaderValue>| {
                value.map(|value| value.to_str().unwrap().to_owned())
            };
            (status, text(range), text(accept), bytes.to_vec())
        }
    };
    let whole = exchange(get("/api/node/status")).await.3;
    let length = whole.len();
    let part = |from: usize, to: usize| whole[from..=to].to_vec();
    let last = length - 1;
    let served = |from: usize, to: usize| Some(format!("bytes {from}-{to}/{length}"));
    let cases = [
        ("bytes=0-9", 206, served(0, 9), part(0, 9)),
        ("BYTES= , 2-4", 206, served(2, 4), part(2, 4)),
        (
            "bytes=-10",
            206,
            served(length - 10, last),
            part(length - 10, last),
        ),
        ("bytes=5-", 206, served(5, last), part(5, last)),
        ("bytes=0-99999", 206, served(0, last), whole.clone()),
        // A suffix longer than the body is the whole body.
        ("bytes=-99999", 206, served(0, last), whole.clone()),
        // Ranges that touch merge into one; hapi gives the merged range the end of the later.
        ("bytes=4-5,0-3", 206, served(0, 5), part(0, 5)),
        ("bytes=0-10, 2-3", 206, served(0, 3), part(0, 3)),
        ("bytes=0-1,3-4", 200, None, whole.clone()),
    ];
    for (range, status, content_range, body) in cases {
        let answer = exchange(ranged(range, None)).await;
        let accept = Some("bytes".to_owned());
        assert_eq!(answer, (status, content_range, accept, body), "{range}");
    }
    let refused = serde_json::to_vec(&boom(
        416,
        "Requested Range Not Satisfiable",
        "Requested Range Not Satisfiable",
    ))
    .unwrap();
    let everything = Some(format!("bytes */{length}"));
    for range in [
        "bytes=5-2",
        &format!("bytes={length}-"),
        "bytes=-0",
        "items=0-9",
        "bytes=0-1,",
        "bytes=0-1 ",
        "bytes=0-1,,2-3",
        "bytes=-",
    ] {
        let answer = exchange(ranged(range, None)).await;
        assert_eq!(
            answer,
            (416, everything.clone(), None, refused.clone()),
            "{range}"
        );
    }
    // An `If-Range` never matches (no answer has an entity tag): the whole answer.
    let answer = exchange(ranged("bytes=0-9", Some("\"x\""))).await;
    assert_eq!(answer, (200, None, Some("bytes".to_owned()), whole.clone()));
    // An error is never cut.
    let request = Request::get("/api/nothing")
        .header("range", "bytes=0-3")
        .body(Body::empty())
        .unwrap();
    assert_eq!(exchange(request).await.0, 404);
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
        // A GET or HEAD that no route matches finds the catch-all route, which has CORS on.
        (
            preflight("/nothing", &[("access-control-request-method", "HEAD")]),
            404,
            boom(404, "Not Found", "CORS error: Missing Origin header"),
        ),
        // A parameter that does not decode fails an assertion of hapi's.
        (
            preflight(
                "/api/wallets/%zz",
                &[origin, ("access-control-request-method", "GET")],
            ),
            500,
            boom(
                500,
                "Internal Server Error",
                "An internal server error occurred",
            ),
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
    // Refused or not, a preflight's answer has no CORS headers of its own: hapi's preflight route
    // has CORS off.
    for (request, status, body) in cases {
        let (answered, headers, answer) = call(&pipeline, request).await;
        assert_eq!((answered, answer), (status, body));
        for name in ["vary", "access-control-expose-headers", "accept-ranges"] {
            assert_eq!(header(&headers, name), None, "{name}");
        }
        assert_eq!(header(&headers, "x-block-height"), Some("265"));
    }
    let get_method = ("access-control-request-method", "get");
    let (status, headers, _) =
        call(&pipeline, preflight("/api/nothing", &[origin, get_method])).await;
    assert_eq!(
        (status, header(&headers, "access-control-allow-methods")),
        (204, Some("get"))
    );
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
    // hapi tests the headers for truth, and reads a repeated one as Node joins it.
    let empty_method = ("access-control-request-method", "");
    let empty_origin = ("origin", "");
    let asked = |headers: &[(&str, &str)]| call(&pipeline, preflight("/api/transactions", headers));
    let missing = |name: &str| {
        boom(
            404,
            "Not Found",
            &format!("CORS error: Missing {name} header"),
        )
    };
    let get = ("access-control-request-method", "GET");
    for (headers, status, body) in [
        (
            vec![origin, empty_method],
            404,
            missing("Access-Control-Request-Method"),
        ),
        (vec![empty_origin, get], 404, missing("Origin")),
        (vec![empty_origin, method], 404, missing("Origin")),
        (
            vec![origin, method, get],
            404,
            boom(404, "Not Found", "Not Found"),
        ),
        (
            vec![origin, empty_method, method],
            404,
            boom(404, "Not Found", "Not Found"),
        ),
        (
            vec![
                origin,
                method,
                ("access-control-request-headers", "accept, accept"),
            ],
            200,
            json!({ "message": "CORS error: Some headers are not allowed" }),
        ),
        (
            vec![
                origin,
                method,
                ("access-control-request-headers", "accept"),
                ("access-control-request-headers", "x-custom"),
            ],
            200,
            json!({ "message": "CORS error: Some headers are not allowed" }),
        ),
    ] {
        let (answered, _, answer) = asked(&headers).await;
        assert_eq!((answered, answer), (status, body), "{headers:?}");
    }
    let (status, headers, _) = asked(&[origin, ("origin", "https://b"), method]).await;
    let allowed = header(&headers, "access-control-allow-origin");
    assert_eq!(
        (status, allowed),
        (204, Some("https://wallet.example, https://b"))
    );
    // The path of a GET or HEAD that no route matches must decode, as a parameter must.
    let undecodable = preflight(
        "/x%zz",
        &[origin, ("access-control-request-method", "HEAD")],
    );
    let (status, headers, _) = call(&pipeline, undecodable).await;
    assert_eq!((status, header(&headers, "vary")), (500, None));
    let unrouted = preflight("/x%zz", &[origin, method]);
    assert_eq!(call(&pipeline, unrouted).await.0, 404);
}

#[tokio::test]
async fn an_unmatched_path_is_answered_as_the_reference_catch_all_answers_it() {
    let mut limited = config();
    limited.rate_limit.points = 2;
    let pipeline = pipeline(limited);
    let head = |uri: &str| Request::head(uri).body(Body::empty()).unwrap();
    // A path that does not decode: hapi's 400, before the rate limit.
    let bad_request = boom(400, "Bad Request", "Bad Request");
    for _ in 0..3 {
        for uri in ["/api/nothing%zz", "/x%zz", "/%C3", "/api/wallets/a%2Fb/x%"] {
            let (status, _, body) = call(&pipeline, get(uri)).await;
            assert_eq!((status, body), (400, bad_request.clone()), "{uri}");
        }
        assert_eq!(call(&pipeline, head("/x%zz")).await.0, 400);
    }
    // A short path that leads above the documentation's directory: inert's 403, charged.
    let (status, headers, body) = call(&pipeline, get("/..%2Fx")).await;
    assert_eq!((status, body), (403, boom(403, "Forbidden", "Forbidden")));
    assert_eq!(header(&headers, "vary"), Some("origin"));
    assert_eq!(call(&pipeline, get("/api/nothing")).await.0, 404);
    assert_eq!(call(&pipeline, get("/..%2Fx")).await.0, 429);
    assert_eq!(call(&pipeline, get("/x%zz")).await.0, 400);
}

/// `request`'s answer from `pipeline`: its status, the bytes of `name`, and its body.
async fn raw_header(pipeline: &Pipeline, request: Request<Body>, name: &str) -> (u16, Vec<u8>) {
    let response = pipeline.handle(request, CLIENT.parse().unwrap()).await;
    let value = response.headers().get(name).map(|v| v.as_bytes().to_vec());
    (response.status().as_u16(), value.unwrap_or_default())
}

#[tokio::test]
async fn request_headers_are_read_as_node_gives_them_to_hapi() {
    let pipeline = pipeline(config());
    let with = |method: &str, uri: &str, headers: &[(&str, &[u8])]| {
        let mut request = Request::builder().method(method).uri(uri);
        for (name, value) in headers {
            request = request.header(*name, axum::http::HeaderValue::from_bytes(value).unwrap());
        }
        request.body(Body::empty()).unwrap()
    };
    let status = "/api/node/status";
    // Repeated values are joined with `, `: two ranges do not read, two empty `If-Range` are one.
    for (headers, answered) in [
        (
            vec![("range", &b"bytes=0-1"[..]), ("range", b"bytes=3-4")],
            416,
        ),
        (vec![("range", &b""[..]), ("range", b"bytes=3-4")], 416),
        (vec![("if-range", &b""[..]), ("range", b"bytes=0-1")], 206),
        (
            vec![
                ("if-range", &b""[..]),
                ("if-range", b""),
                ("range", b"bytes=0-1"),
            ],
            200,
        ),
    ] {
        let (code, _) = raw_header(&pipeline, with("GET", status, &headers), "vary").await;
        assert_eq!(code, answered, "{headers:?}");
    }
    let origins = [("origin", &b"a"[..]), ("origin", b"b")];
    let origin = "access-control-allow-origin";
    let joined = raw_header(&pipeline, with("GET", status, &origins), origin).await;
    assert_eq!(joined, (200, b"a, b".to_vec()));
    // A byte beyond ASCII is a Latin-1 character, which Node writes back in UTF-8 when it writes
    // the head with a body given as text, and in Latin-1 otherwise.
    let latin1 = [("origin", &b"\xe9"[..])];
    for (method, range, answered, bytes) in [
        ("GET", None, 200, &b"\xc3\xa9"[..]),
        ("GET", Some(&b"bytes=0-1"[..]), 206, b"\xe9"),
        ("GET", Some(b"bytes=900-901"), 416, b"\xc3\xa9"),
        ("HEAD", None, 200, b"\xe9"),
    ] {
        let mut headers = latin1.to_vec();
        headers.extend(range.map(|range| ("range", range)));
        let echoed = raw_header(&pipeline, with(method, status, &headers), origin).await;
        assert_eq!(echoed, (answered, bytes.to_vec()), "{method} {range:?}");
    }
    let unmatched = raw_header(&pipeline, with("GET", "/api/nothing", &latin1), origin).await;
    assert_eq!(unmatched, (404, b"\xc3\xa9".to_vec()));
    // hapi writes the head of an answer it compresses (1 KiB or more, for an `Accept-Encoding`
    // from which `@hapi/accept` picks gzip or deflate) in Latin-1.
    let large = format!("{status}?a={}", "x".repeat(1_200));
    for (encoding, bytes) in [
        ("gzip", &b"\xe9"[..]),
        ("deflate, gzip", b"\xe9"),
        ("x-gzip", b"\xe9"),
        ("gzip;q=0.5, identity;q=0.4", b"\xe9"),
        ("identity", b"\xc3\xa9"),
        ("gzip;q=0", b"\xc3\xa9"),
        ("gzip, identity", b"\xc3\xa9"),
        ("*", b"\xc3\xa9"),
        ("gzip;v=1", b"\xc3\xa9"),
    ] {
        let headers = [latin1[0], ("accept-encoding", encoding.as_bytes())];
        let echoed = raw_header(&pipeline, with("GET", &large, &headers), origin).await;
        assert_eq!(echoed, (200, bytes.to_vec()), "{encoding}");
    }
    let small = [latin1[0], ("accept-encoding", &b"gzip"[..])];
    let echoed = raw_header(&pipeline, with("GET", status, &small), origin).await;
    assert_eq!(echoed, (200, b"\xc3\xa9".to_vec()));
    // The last entry of the joined `X-Forwarded-For`, the one the trusted proxy appended.
    let mut trusting = config();
    trusting.trust_proxy = true;
    let proxied = pipeline_with(trusting, echo);
    let forwarded = [
        ("x-forwarded-for", &b" 10.0.0.5"[..]),
        ("x-forwarded-for", b"10.0.0.6 "),
    ];
    let (_, _, body) = call(&proxied, with("GET", status, &forwarded)).await;
    assert_eq!(body["data"]["client"], "10.0.0.6");
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
        ("/api/wallets/top/transactions", "WalletsTransactions"),
        ("/api/blocks/first/transactions", "BlocksTransactions"),
    ];
    for (uri, expected) in cases {
        let (status, _, body) = call(&pipeline, get(uri)).await;
        assert_eq!((status, route(&body)), (200, expected.to_owned()), "{uri}");
    }
    let (_, _, body) = call(&pipeline, get("/api/wallets/genesis%5F34")).await;
    assert_eq!(body["data"]["params"], json!({ "id": "genesis_34" }));
    // Unknown paths, and another method on a known path, are 404 (never 405); so are the paths of
    // the reference's routes that are not served, which a parameter would otherwise take.
    let not_found = boom(404, "Not Found", "Not Found");
    for request in [
        get("/api/node/status//"),
        get("/api/nothing"),
        get("/api/blocks/first"),
        get("/api/blocks/missed/"),
        get("/api/transactions/schemas"),
        get("/api/wallets/top"),
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
    // Behind a trusted proxy the address it appended, the last, is the client's; the reference
    // reads the first, which the client writes.
    config.trust_proxy = true;
    let proxied = pipeline(config);
    let forwarded = |value: &str| {
        Request::get("/api/node/status")
            .header("x-forwarded-for", value)
            .body(Body::empty())
            .unwrap()
    };
    let (status, _, body) = call(&proxied, forwarded("198.51.100.1 , 10.0.0.5 ")).await;
    assert_eq!((status, &body["data"]["client"]), (200, &json!("10.0.0.5")));
    let (status, _, body) = call(&proxied, forwarded(" 10.0.0.5 , 198.51.100.1")).await;
    assert_eq!((status, body), (403, forbidden));
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
    let read = [
        // No content type, or an empty one, is JSON, whose empty body is null.
        (post(r#"{"b":2}"#, &[]), json!({ "b": 2 })),
        (
            post(r#"{"b":3}"#, &[("content-type", "")]),
            json!({ "b": 3 }),
        ),
        (post("", &[json]), Value::Null),
        // A type is read up to a space or `;`, in any case; `+json` is JSON.
        (
            post("[1]", &[("content-type", "Application/JSON charset")]),
            json!([1]),
        ),
        (
            post("[2]", &[("content-type", "application/vnd.api+json")]),
            json!([2]),
        ),
        // Bytes that are not UTF-8 are read as U+FFFD.
        (post(&b"[\"\xff\"]"[..], &[json]), json!(["\u{fffd}"])),
        // Text is text, an empty one included; bytes are text, or null when empty.
        (
            post("words", &[("content-type", "text/plain")]),
            json!("words"),
        ),
        (post("", &[("content-type", "text/csv")]), json!("")),
        (
            post("", &[("content-type", "application/octet-stream")]),
            Value::Null,
        ),
        // A form is read as no fields.
        (
            post(
                "a=b",
                &[("content-type", "application/x-www-form-urlencoded")],
            ),
            json!({}),
        ),
    ];
    for (request, payload) in read {
        let (status, _, body) = call(&pipeline, request).await;
        assert_eq!((status, &body["data"]["payload"]), (200, &payload));
    }
    // Every failure is the reference's 422 with hapi's message, before the rate limit.
    let refused = |message: &str| boom(422, "Unprocessable Entity", message);
    let too_large = refused("Payload content length greater than maximum allowed: 1048576");
    let malformed = refused("Invalid request payload JSON format");
    let unsupported = refused("Unsupported Media Type");
    let invalid = refused("Invalid content-type header");
    let cases = [
        (post("{", &[json]), malformed.clone()),
        (post(r#"{"a":{"__proto__":{}}}"#, &[json]), malformed),
        (
            post("x", &[("content-type", "image/png")]),
            unsupported.clone(),
        ),
        (
            post("x", &[("content-type", "multipart/form-data; boundary=x")]),
            unsupported,
        ),
        (
            post("x", &[("content-type", "multipart/mixed; charset=x")]),
            refused("Invalid content-type header: multipart missing boundary"),
        ),
        (post("x", &[("content-type", "nonsense")]), invalid.clone()),
        (post("x", &[("content-type", "text/")]), invalid),
        (
            post("x", &[json, ("content-length", "1048577")]),
            too_large.clone(),
        ),
        (post(vec![b' '; 1_048_577], &[json]), too_large),
    ];
    let mut limited = config();
    limited.rate_limit.points = 1;
    let limited = pipeline_with(limited, echo);
    for (request, expected) in cases {
        let (status, headers, body) = call(&limited, request).await;
        assert_eq!((status, body), (422, expected));
        assert_eq!(header(&headers, "vary"), Some("origin"));
    }
    assert_eq!(call(&limited, get("/api/node/status")).await.0, 200);
}

#[tokio::test]
async fn a_compressed_payload_is_read_as_hapi_decompresses_it() {
    use std::io::Write;
    let compress = |encoding: &str, bytes: &[u8]| {
        let level = flate2::Compression::default();
        match encoding {
            "gzip" => {
                let mut encoder = flate2::write::GzEncoder::new(Vec::new(), level);
                encoder.write_all(bytes).unwrap();
                encoder.finish().unwrap()
            }
            _ => {
                let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), level);
                encoder.write_all(bytes).unwrap();
                encoder.finish().unwrap()
            }
        }
    };
    let pipeline = pipeline(config());
    let refused = |message: &str| json!({"statusCode": 422, "error": "Unprocessable Entity", "message": message});
    let cases = [
        (
            compress("gzip", br#"{"a":1}"#),
            "gzip",
            200,
            json!({ "a": 1 }),
        ),
        (
            compress("deflate", br#"{"b":2}"#),
            "deflate",
            200,
            json!({ "b": 2 }),
        ),
        // Other encodings, and other spellings, are read as they are.
        (br#"{"c":3}"#.to_vec(), "br", 200, json!({ "c": 3 })),
        (br#"{"d":4}"#.to_vec(), "GZIP", 200, json!({ "d": 4 })),
        (
            b"abc".to_vec(),
            "gzip",
            422,
            refused("Invalid compressed payload"),
        ),
        // The limit holds for the decompressed body.
        (
            compress("gzip", &vec![b' '; 1_048_577]),
            "gzip",
            422,
            refused("Payload content length greater than maximum allowed: 1048576"),
        ),
        // Node's gunzip reads members one after another, and stops at a zero byte after one.
        (
            [compress("gzip", br#"{"e":"#), compress("gzip", b"5}")].concat(),
            "gzip",
            200,
            json!({ "e": 5 }),
        ),
        (
            [compress("gzip", br#"{"f":6}"#), vec![0; 8]].concat(),
            "gzip",
            200,
            json!({ "f": 6 }),
        ),
        (
            [
                compress("gzip", br#"{"g":"#),
                vec![0; 2],
                compress("gzip", b"7}"),
            ]
            .concat(),
            "gzip",
            422,
            refused("Invalid request payload JSON format"),
        ),
        (
            [compress("gzip", br#"{"h":8}"#), b"xyz".to_vec()].concat(),
            "gzip",
            422,
            refused("Invalid compressed payload"),
        ),
        (
            Vec::new(),
            "gzip",
            422,
            refused("Invalid compressed payload"),
        ),
        // Its inflate ignores what follows the stream.
        (
            [compress("deflate", br#"{"i":9}"#), b"xyz".to_vec()].concat(),
            "deflate",
            200,
            json!({ "i": 9 }),
        ),
    ];
    for (body, encoding, status, expected) in cases {
        let headers = [
            ("content-type", "application/json"),
            ("content-encoding", encoding),
        ];
        let (answered, _, answer) = call(&pipeline, post(body, &headers)).await;
        let answer = match answered {
            200 => answer["data"]["payload"].clone(),
            _ => answer,
        };
        assert_eq!((answered, answer), (status, expected), "{encoding}");
    }
    // Repeated encodings are joined, which names no decoder: the body is read as it is.
    let gzip = compress("gzip", br#"{"j":10}"#);
    for encodings in [["gzip", "gzip"], ["gzip", ""], ["", "gzip"]] {
        let headers = [
            ("content-encoding", encodings[0]),
            ("content-encoding", encodings[1]),
        ];
        let (answered, _, answer) = call(&pipeline, post(gzip.clone(), &headers)).await;
        let expected = refused("Invalid request payload JSON format");
        assert_eq!((answered, answer), (422, expected), "{encodings:?}");
    }
    let headers = [("content-encoding", "gzip"), ("content-encoding", "gzip")];
    let (answered, _, answer) = call(&pipeline, post(r#"{"k":11}"#, &headers)).await;
    assert_eq!(
        (answered, &answer["data"]["payload"]),
        (200, &json!({ "k": 11 }))
    );
}

#[tokio::test]
async fn a_content_type_is_read_as_latin1_text() {
    let pipeline = pipeline(config());
    let typed = |content_type: &[u8]| {
        let value = axum::http::HeaderValue::from_bytes(content_type).unwrap();
        Request::post("/api/transactions")
            .header("content-type", value)
            .body(Body::from(r#"{"a":1}"#))
            .unwrap()
    };
    let unsupported = "Unsupported Media Type";
    let missing = "Invalid content-type header: multipart missing boundary";
    for (content_type, expected) in [
        (&b"application/json\xe9"[..], Err(unsupported)),
        (b"\xe9/json", Err(unsupported)),
        (b"APPLICATION/JSON\xc9", Err(unsupported)),
        (b"application/json\x85x", Err(unsupported)),
        (b"application\xa0/json", Err("Invalid content-type header")),
        (b"multipart/mixed;\x85boundary=x", Err(missing)),
        (b"multipart/mixed;\xa0boundary=x", Err(unsupported)),
        (b"multipart/mixed; boundary=\x85x", Err(unsupported)),
        (b"text/plain\xe9", Ok(json!(r#"{"a":1}"#))),
        (b"application/\xe9+json", Ok(json!({ "a": 1 }))),
    ] {
        let (status, _, body) = call(&pipeline, typed(content_type)).await;
        let answer = match status {
            200 => Ok(body["data"]["payload"].clone()),
            _ => Err(body["message"].as_str().unwrap_or_default().to_owned()),
        };
        let expected = expected.map_err(str::to_owned);
        assert_eq!(answer, expected, "{content_type:?}");
    }
    // Node gives hapi the first of two content types.
    let two = Request::post("/api/transactions")
        .header("content-type", "")
        .header("content-type", "text/plain")
        .body(Body::from(r#"{"a":1}"#))
        .unwrap();
    assert_eq!(
        call(&pipeline, two).await.2["data"]["payload"],
        json!({ "a": 1 })
    );
}

#[tokio::test]
async fn a_multipart_type_needs_a_boundary_parameter() {
    let pipeline = pipeline(config());
    for (header, read) in [
        ("multipart/mixed; boundary=x", true),
        ("multipart/mixed;boundary=\"a b\"", true),
        ("Multipart/Mixed; a=1; BOUNDARY=x", true),
        ("multipart/mixed; boundary=\"\"", false),
        ("multipart/mixed; boundary=", false),
        ("multipart/mixed; boundary= x", false),
        ("multipart/mixed boundary=x", false),
    ] {
        let (status, _, body) = call(&pipeline, post("x", &[("content-type", header)])).await;
        let message = match read {
            true => "Unsupported Media Type",
            false => "Invalid content-type header: multipart missing boundary",
        };
        assert_eq!(
            (status, &body["message"]),
            (422, &json!(message)),
            "{header}"
        );
    }
}

#[tokio::test(start_paused = true)]
async fn the_rate_limit_charges_every_routed_request_per_window() {
    let mut config = config();
    config.rate_limit.points = 4;
    let pipeline = pipeline(config);
    // Another method on any path, a POST on a path no route matches and a parameter that does not
    // decode cost nothing; a GET or HEAD on a path no route matches costs a point (the reference's
    // catch-all route takes it), as a request the schema refuses does.
    for _ in 0..5 {
        let put = Request::put("/api/nothing").body(Body::empty()).unwrap();
        let unmatched = Request::post("/api/nothing").body(Body::empty()).unwrap();
        assert_eq!(call(&pipeline, put).await.0, 404);
        assert_eq!(call(&pipeline, unmatched).await.0, 404);
        assert_eq!(call(&pipeline, get("/api/wallets/%zz")).await.0, 400);
    }
    let head = Request::head("/nothing").body(Body::empty()).unwrap();
    assert_eq!(call(&pipeline, head).await.0, 404);
    assert_eq!(call(&pipeline, get("/api/nothing")).await.0, 404);
    assert_eq!(call(&pipeline, get("/api/node/status?bad=1")).await.0, 422);
    assert_eq!(call(&pipeline, get("/api/node/status")).await.0, 200);
    assert_eq!(call(&pipeline, get("/api/nothing")).await.0, 429);
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
async fn the_semaphore_levels_follow_the_served_query_grammar() {
    let mut config = config();
    let [first, second] = &mut config.semaphore.database;
    (first.concurrency, first.queue_limit) = (0, 0);
    (second.concurrency, second.queue_limit) = (1, 0);
    let host = Host {
        config: Arc::new(config),
        height: Box::new(|| 265),
        dispatch: Box::new(echo),
    };
    let pipeline = Pipeline::with_parts(RouteId::ALL.to_vec(), host, Grammar::SCHEMAS);
    let key = "0334f21b095dbab8c1c602ff0df73371605dd9b79193eb55c89b50d86be0defdea";
    // The first level has no place here: 429 means level one, 200 level two. The validated
    // query's defaults (the order by timestamp, then sequence) are served by the indexes.
    let cases = [
        ("/api/transactions".to_owned(), 429),
        ("/api/transactions?limit=1".to_owned(), 429),
        ("/api/transactions?orderBy=timestamp:desc".to_owned(), 429),
        ("/api/transactions?orderBy=nonce:desc".to_owned(), 429),
        (format!("/api/transactions?senderPublicKey={key}"), 429),
        ("/api/wallets/genesis_1/transactions".to_owned(), 429),
        ("/api/blocks/5/transactions".to_owned(), 429),
        ("/api/transactions?memo=x".to_owned(), 200),
        ("/api/transactions?orderBy=amount:desc".to_owned(), 200),
        ("/api/transactions?page=101&limit=100".to_owned(), 200),
        ("/api/wallets/genesis_1/transactions?memo=x".to_owned(), 200),
    ];
    for (uri, expected) in cases {
        assert_eq!(call(&pipeline, get(&uri)).await.0, expected, "{uri}");
    }
}

#[tokio::test]
#[expect(
    clippy::disallowed_types,
    reason = "the test times the answer against the wall clock"
)]
async fn a_long_asset_list_with_casts_is_answered_at_once() {
    let host = Host {
        config: Arc::new(config()),
        height: Box::new(|| 265),
        dispatch: Box::new(|_, request| {
            Box::pin(async move {
                let criteria = crate::controllers::transactions::criteria(&request.query)?;
                let count = criteria.map_or(0, |criteria| criteria.asset.len());
                Ok(ApiReply::data(json!(count)))
            })
        }),
    };
    let pipeline = Pipeline::with_parts(RouteId::ALL.to_vec(), host, Grammar::SCHEMAS);
    // Five casts and a list of 16,384 items from a 40-byte criterion: 32 alternatives, which the
    // criteria keeps as the one value the store reads with its casts. This times the query's
    // parse and cast check; the store's read of the value is timed in heartwood-database's
    // history tests.
    let id = "a".repeat(64);
    let uri = format!("/api/transactions?id={id}&asset.x=1,2,3,true,false&asset.x.16383=c");
    let started = std::time::Instant::now();
    let (status, _, body) = call(&pipeline, get(&uri)).await;
    let elapsed = started.elapsed();
    assert_eq!((status, &body["data"]), (200, &json!(1)), "{body}");
    assert!(elapsed < Duration::from_secs(5), "{elapsed:?}");
}

#[tokio::test]
async fn links_are_relative_to_the_configured_base_path() {
    let mut config = config();
    config.base_path = "/v2".to_owned();
    let host = Host {
        config: Arc::new(config),
        height: Box::new(|| 265),
        dispatch: Box::new(|_, request| {
            Box::pin(async move {
                let pagination = crate::pagination::Pagination::of(&request.query);
                let body =
                    crate::pagination::envelope(&request, pagination, 250, false, Vec::new());
                Ok(ApiReply::ok(body))
            })
        }),
    };
    let pipeline = Pipeline::with_parts(RouteId::ALL.to_vec(), host, Grammar::SCHEMAS);
    for (uri, link) in [
        ("/v2/transactions?limit=10", "/transactions?"),
        ("/v2/transactions/?limit=10", "/transactions?"),
        (
            "/v2/wallets/genesis%5f1/transactions",
            "/wallets/genesis_1/transactions?",
        ),
        (
            "/v2/blocks/%31%7c%7C/transactions",
            "/blocks/1%7C%7C/transactions?",
        ),
    ] {
        let (status, _, body) = call(&pipeline, get(uri)).await;
        assert_eq!(status, 200, "{uri}");
        let first = body["meta"]["first"].as_str().unwrap();
        assert!(first.starts_with(link), "{uri}: {first}");
    }
    assert_eq!(call(&pipeline, get("/api/transactions")).await.0, 404);
}

/// The top-level keys of the request's query, how many values it has and how deep it is (counted
/// without recursion), as the handler saw them.
fn summary(_: RouteId, request: ApiRequest) -> HandlerFuture {
    Box::pin(async move {
        let keys: Vec<&String> = request
            .query
            .as_object()
            .into_iter()
            .flatten()
            .map(|x| x.0)
            .collect();
        let (mut values, mut depth, mut stack) = (0, 0, vec![(&request.query, 1)]);
        while let Some((value, level)) = stack.pop() {
            (values, depth) = (values + 1, depth.max(level));
            match value {
                Value::Array(items) => stack.extend(items.iter().map(|x| (x, level + 1))),
                Value::Object(map) => stack.extend(map.values().map(|x| (x, level + 1))),
                _ => {}
            }
        }
        Ok(ApiReply::data(
            json!({ "keys": keys, "values": values, "depth": depth }),
        ))
    })
}

#[test]
fn hostile_queries_are_answered_on_a_worker_stack() {
    // The API's workers have tokio's default stack of 2 MiB.
    let thread = std::thread::Builder::new().stack_size(2 << 20);
    let answers = thread.spawn(|| {
        let mut runtime = tokio::runtime::Builder::new_current_thread();
        let runtime = runtime.enable_all().build().unwrap();
        runtime.block_on(async {
            let host = Host {
                config: Arc::new(config()),
                height: Box::new(|| 265),
                dispatch: Box::new(summary),
            };
            let pipeline = Pipeline::with_parts(RouteId::ALL.to_vec(), host, Grammar::SCHEMAS);
            let dots = |key: &str| format!("{key}{}=1", ".a".repeat(30_000));
            let gaps: String = (1..=998).map(|i| format!("&a.{}=1", i * 1000)).collect();
            let mut answers = Vec::new();
            for uri in [
                format!("/api/nothing?{}", dots("a")),
                format!("/api/node/status?{}", dots("a")),
                format!("/api/transactions?{}", dots("a")),
                format!("/api/transactions?{}", dots("asset")),
                format!("/api/node/status?a=1,2{gaps}"),
                format!("/api/transactions?a=1,2{gaps}"),
                "/api/transactions?asset.x=1,2&asset.x.4294967294=3".to_owned(),
            ] {
                answers.push(call(&pipeline, get(&uri)).await);
            }
            answers
        })
    });
    let answers = answers.unwrap().join().unwrap();
    let statuses: Vec<u16> = answers.iter().map(|answer| answer.0).collect();
    assert_eq!(statuses, [404, 200, 422, 500, 200, 422, 500]);
    // The handler reads the query written out within its bounds, without the grammar's own key.
    let deep = &answers[1].2["data"];
    assert_eq!(
        (&deep["keys"], &deep["depth"]),
        (&json!(["a"]), &json!(129))
    );
    let sparse = &answers[4].2["data"];
    assert_eq!(
        (&sparse["keys"], &sparse["values"]),
        (&json!(["a"]), &json!(2 + 2 + 2 * 998))
    );
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

#[tokio::test]
async fn a_head_that_node_refuses_is_answered_as_hapi_answers_it() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address: SocketAddr = listener.local_addr().unwrap();
    let pipeline = Arc::new(pipeline(config()));
    let (stop, stopped) = watch::channel(false);
    let timeout = Duration::from_secs(5);
    let task = tokio::spawn(server::accept(listener, pipeline, stopped, timeout));
    let exchange = |parts: Vec<String>| async move {
        let mut client = tokio::net::TcpStream::connect(address).await.unwrap();
        for part in parts {
            client.write_all(part.as_bytes()).await.unwrap();
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let mut answer = String::new();
        client.read_to_string(&mut answer).await.unwrap();
        answer
    };
    // Node counts the target's bytes (and the header names and values) up to 16 KiB; line ends
    // before the request line are skipped, uncounted.
    let bare = "HTTP/1.1 400 Bad Request\r\n\r\n";
    for lead in ["", "\n", "\r\n\r\n"] {
        let get = |length: usize| {
            let target = "q".repeat(length - 5);
            format!("{lead}GET /api/{target} HTTP/1.1\r\n\r\n")
        };
        assert_eq!(exchange(vec![get(16_384)]).await, bare, "{lead:?}");
        let routed = exchange(vec![get(16_383)]).await;
        assert!(routed.starts_with("HTTP/1.1 404 Not Found\r\n"), "{routed}");
    }
    // What Node's parser refuses is refused so, and what it reads is served as Node serves it.
    let fields: String = (0..150).map(|i| format!("x{i}: v\r\n")).collect();
    let cases = [
        ("FOO /api/node/status HTTP/1.1\r\n\r\n".to_owned(), bare),
        ("GET /api/node/status HTTP/1.1\nx: y\n\n".to_owned(), bare),
        (
            "\rGET /api/node/status HTTP/2.0\r\nx y: z\r\n\r\n".to_owned(),
            "HTTP/1.1 200 OK\r\n",
        ),
        (
            format!("GET /api/node/status HTTP/1.1\r\n{fields}\r\n"),
            "HTTP/1.1 200 OK\r\n",
        ),
        (
            "GET /api/node/status HTTP/1.1\r\nTransfer-Encoding:  \r\n\r\n".to_owned(),
            "HTTP/1.1 200 OK\r\n",
        ),
        (
            "GET /api/node/status?a=\"<>\" HTTP/1.1\r\n\r\n".to_owned(),
            "HTTP/1.1 200 OK\r\n",
        ),
        (
            "GET *x HTTP/1.1\r\n\r\n".to_owned(),
            "HTTP/1.1 400 Bad Request\r\ncontent-type",
        ),
        ("CONNECT a:1 HTTP/1.1\r\n\r\n".to_owned(), ""),
    ];
    for (request, start) in cases {
        let answer = exchange(vec![request.clone()]).await;
        match start {
            "" | "HTTP/1.1 400 Bad Request\r\n\r\n" => assert_eq!(answer, start, "{request:?}"),
            _ => assert!(answer.starts_with(start), "{request:?}: {answer}"),
        }
    }
    let invalid = exchange(vec!["GET *x HTTP/1.1\r\n\r\n".to_owned()]).await;
    assert!(
        invalid.ends_with(r#""message":"Invalid URL"}"#),
        "{invalid}"
    );
    let quoted = exchange(vec![
        "GET /api/node/status?a=\"<>\" HTTP/1.1\r\n\r\n".to_owned(),
    ])
    .await;
    assert!(quoted.contains(r#""query":{"a":"\"<>\""}"#), "{quoted}");
    // The bytes read with the head are served again, and the body that comes after them.
    let body = r#"{"transactions":12}"#;
    let head = format!(
        "POST /api/transactions HTTP/1.1\r\ncontent-length: {}\r\n\r\n",
        body.len()
    );
    for parts in [vec![format!("{head}{body}")], vec![head, body.to_owned()]] {
        let answer = exchange(parts).await;
        assert!(answer.starts_with("HTTP/1.1 200 OK\r\n"), "{answer}");
        assert!(
            answer.contains(r#""payload":{"transactions":12}"#),
            "{answer}"
        );
    }
    stop.send(true).unwrap();
    task.await.unwrap();
}

/// `pipeline` served on a local port: its address, and what stops it.
async fn listening(pipeline: Pipeline) -> (SocketAddr, watch::Sender<bool>, JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (stop, stopped) = watch::channel(false);
    let timeout = Duration::from_secs(5);
    let task = tokio::spawn(server::accept(
        listener,
        Arc::new(pipeline),
        stopped,
        timeout,
    ));
    (address, stop, task)
}

/// The answer of the server at `address` to `request`, sent at once and followed by the end of
/// the client's side when `shut`: its status line, its head and its body read as JSON (null
/// when it has none, or none that reads).
async fn exchange_bytes(
    address: SocketAddr,
    request: &[u8],
    shut: bool,
) -> (String, String, Value) {
    let mut client = tokio::net::TcpStream::connect(address).await.unwrap();
    client.write_all(request).await.unwrap();
    if shut {
        client.shutdown().await.unwrap();
    }
    let mut answer = Vec::new();
    let _ = client.read_to_end(&mut answer).await;
    let answer = String::from_utf8_lossy(&answer).into_owned();
    let (head, body) = answer.split_once("\r\n\r\n").unwrap_or((&answer, ""));
    let line = head.lines().next().unwrap_or_default().to_owned();
    (
        line,
        head.to_owned(),
        serde_json::from_str(body).unwrap_or(Value::Null),
    )
}

/// The plugins over the served grammar, and a handler that echoes the request.
fn served_grammar() -> Pipeline {
    let host = Host {
        config: Arc::new(config()),
        height: Box::new(|| 265),
        dispatch: Box::new(echo),
    };
    Pipeline::with_parts(RouteId::ALL.to_vec(), host, Grammar::SCHEMAS)
}

#[tokio::test]
async fn a_target_is_read_as_node_gives_it_to_hapi() {
    let (address, stop, task) = listening(served_grammar()).await;
    // Measured with Node 18.20.8 and hapi 20.2.1: each byte of the target is a Latin-1
    // character, and the query is that text as Node's querystring decodes it (an escape Node
    // does not find leaves a pair as it is, and its byte fallback truncates a character).
    let request =
        b"GET /api/node/status?%41%zz\xe9=1&b=\"<>\"&c=%zz\"\xe9&d=\xc3\xa9&%41\"=%zz\xe9 \
        HTTP/1.1\r\n\r\n";
    let (line, _, body) = exchange_bytes(address, request, false).await;
    assert_eq!(line, "HTTP/1.1 200 OK");
    let query = json!({
        "A%zz\u{fffd}": "1", "b": "\"<>\"", "c": "%zz\"\u{e9}", "d": "\u{c3}\u{a9}",
        "A\"": "%zz\u{e9}",
    });
    assert_eq!(body["data"]["query"], query);
    // The catch-all's short path counts a byte as a character.
    let (line, _, _) =
        exchange_bytes(address, b"GET /..%2F\xe9\xe9\xe9 HTTP/1.1\r\n\r\n", false).await;
    assert_eq!(line, "HTTP/1.1 403 Forbidden");
    // Within Node's 16 KiB, a target of bytes beyond ASCII is served.
    let high = b"\xc3\xa9".repeat(6_000);
    for (start, answered) in [
        (&b"GET /"[..], "404 Not Found"),
        (b"GET /api/node/status?q=", "200 OK"),
    ] {
        let request = [start, &high, b" HTTP/1.1\r\n\r\n"].concat();
        let (line, _, _) = exchange_bytes(address, &request, false).await;
        assert_eq!(line, format!("HTTP/1.1 {answered}"));
    }
    stop.send(true).unwrap();
    task.await.unwrap();
}

#[tokio::test]
async fn a_client_that_ends_its_side_after_its_request_is_answered() {
    // Node's server allows half-open sockets: it answers a request whose client shut its side,
    // and gives no answer when that side ends before the head does.
    let (address, stop, task) = listening(pipeline(config())).await;
    let body = r#"{"transactions":12}"#;
    let post = format!(
        "POST /api/transactions HTTP/1.1\r\ncontent-length: {}\r\n\r\n{body}",
        body.len()
    );
    for request in ["GET /api/node/status HTTP/1.1\r\n\r\n", &post] {
        let (line, _, answer) = exchange_bytes(address, request.as_bytes(), true).await;
        assert_eq!(line, "HTTP/1.1 200 OK", "{request:?}");
        assert!(answer["data"]["route"].is_string(), "{request:?}");
    }
    let unended = exchange_bytes(address, b"GET /api/node/status HTTP/1.1\r\n", true).await;
    assert_eq!(unended.0, "");
    stop.send(true).unwrap();
    task.await.unwrap();
}

#[tokio::test]
async fn a_request_whose_following_bytes_node_refuses_is_answered_with_hapi_s_400() {
    // Node's parser fails on what follows the request while hapi serves it: hapi answers that
    // request with its 400 before any of its steps, so without a charge, with the CORS headers of
    // the route it finds (none for an `OPTIONS`). Measured with Node 18.20.8 and hapi 20.2.1.
    let mut limited = config();
    limited.rate_limit.points = 1;
    let (address, stop, task) = listening(pipeline(limited)).await;
    let refused: [&[u8]; 6] = [
        b"GET /api/node/status HTTP/1.1\r\nOrigin: o\r\n\r\n\x00",
        b"GET /x%zz HTTP/1.1\r\nOrigin: o\r\n\r\nFOO / HTTP/1.1\r\n\r\n",
        b"GET /api/nothing HTTP/1.1\r\nOrigin: o\r\ncontent-length: 2\r\n\r\n{}\x00",
        b"POST /api/transactions HTTP/1.1\r\nOrigin: o\r\ncontent-length: 2\r\n\r\n{}GARBAGE",
        b"POST /api/transactions HTTP/1.1\r\nOrigin: o\r\ntransfer-encoding: gzip\r\n\r\n",
        b"OPTIONS /api/node/status HTTP/1.1\r\nOrigin: o\r\n\r\n\x00",
    ];
    for request in refused {
        let shown = String::from_utf8_lossy(request).into_owned();
        let (line, head, body) = exchange_bytes(address, request, false).await;
        assert_eq!(line, "HTTP/1.1 400 Bad Request", "{shown:?}");
        assert_eq!(body, boom(400, "Bad Request", "Bad Request"), "{shown:?}");
        let cors = !shown.starts_with("OPTIONS");
        assert_eq!(head.contains("vary: origin"), cors, "{shown:?}");
        let echoed = head.contains("access-control-allow-origin: o");
        assert_eq!(echoed, cors, "{shown:?}");
        assert!(head.contains("x-block-height: 265"), "{shown:?}");
    }
    // A `GET` that a route serves is answered before Node's parser reads on after its body. None
    // of the refusals was charged: the window's one point was left for it.
    let request = b"GET /api/node/status HTTP/1.1\r\ncontent-length: 2\r\n\r\n{}\x00";
    let (line, _, _) = exchange_bytes(address, request, false).await;
    assert_eq!(line, "HTTP/1.1 200 OK");
    let request = b"GET /api/node/status HTTP/1.1\r\n\r\n";
    let (line, _, _) = exchange_bytes(address, request, false).await;
    assert_eq!(line, "HTTP/1.1 429 Too Many Requests");
    stop.send(true).unwrap();
    task.await.unwrap();
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
    assert_eq!(defaults.connection_timeout(), Duration::from_secs(5));
    // A socket timeout of 0 is none, in Node; its request timeout of 300 s still bounds a request.
    let off = json!({ "plugins": { "socketTimeout": 0 } });
    let off = ApiConfig::from_options(&off, &Env::default()).unwrap();
    assert_eq!(off.connection_timeout(), Duration::from_secs(300));
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

#[tokio::test]
async fn a_compressed_body_is_read_off_the_runtime() {
    use std::io::Write;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    // Many empty gzip members: work for the decoder that must not hold the API's workers.
    let mut member = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    member.write_all(b"").unwrap();
    let body = member.finish().unwrap().repeat(20_000);
    let mut headers = axum::http::HeaderMap::new();
    headers.insert("content-encoding", "gzip".parse().unwrap());
    headers.insert("content-type", "application/json".parse().unwrap());
    let (done, polls) = (AtomicBool::new(false), AtomicUsize::new(0));
    let read = async {
        let read = super::payload::read(&headers, Body::from(body)).await;
        done.store(true, Ordering::SeqCst);
        read
    };
    let other = async {
        while !done.load(Ordering::SeqCst) {
            polls.fetch_add(1, Ordering::SeqCst);
            tokio::task::yield_now().await;
        }
    };
    let (read, ()) = tokio::join!(read, other);
    assert_eq!(read.unwrap(), Value::Null);
    // The runtime ran other work while the body was decoded.
    assert!(polls.load(Ordering::SeqCst) > 0);
}

#[test]
fn a_server_error_line_names_at_most_128_bytes_of_the_path() {
    let short = super::logged_target("GET", "/api/node/status");
    assert_eq!(short, "GET /api/node/status");
    let long = format!("/{}", "x".repeat(16_300));
    let logged = super::logged_target("GET", &long);
    assert_eq!(logged, format!("GET /{}... (16301 bytes)", "x".repeat(127)));
}

#[tokio::test(start_paused = true)]
async fn server_error_lines_are_written_once_a_second_with_a_count_of_the_rest() {
    let mut lines = super::ErrorLines::default();
    let now = tokio::time::Instant::now();
    assert_eq!(lines.admit(now), Some(0));
    for _ in 0..5 {
        assert_eq!(lines.admit(now + Duration::from_millis(500)), None);
    }
    assert_eq!(lines.admit(now + Duration::from_millis(999)), None);
    assert_eq!(lines.admit(now + Duration::from_secs(1)), Some(6));
    assert_eq!(lines.admit(now + Duration::from_secs(3)), Some(0));
}

#[tokio::test(start_paused = true)]
async fn behind_a_trusted_proxy_the_address_it_appended_keys_the_limit() {
    let mut config = config();
    config.trust_proxy = true;
    config.rate_limit.points = 1;
    let limited = pipeline(config);
    let forwarded = |value: &str| {
        Request::get("/api/node/status")
            .header("x-forwarded-for", value)
            .body(Body::empty())
            .unwrap()
    };
    // The first entry is the client's to write: it no longer escapes the limit.
    let (status, _, body) = call(&limited, forwarded("10.0.0.1, 198.51.100.7")).await;
    assert_eq!(
        (status, &body["data"]["client"]),
        (200, &json!("198.51.100.7"))
    );
    let (status, _, _) = call(&limited, forwarded("10.0.0.2, 198.51.100.7")).await;
    assert_eq!(status, 429);
    // An entry that is no address is the socket's address.
    let (status, _, body) = call(&limited, forwarded("not an address")).await;
    assert_eq!(
        (status, &body["data"]["client"]),
        (200, &json!("127.0.0.1"))
    );
    // The addresses of one IPv6 /64 share one limit.
    let (status, _, _) = call(&limited, forwarded("2001:db8:0:1::1")).await;
    assert_eq!(status, 200);
    let (status, _, _) = call(&limited, forwarded("2001:db8:0:1::2")).await;
    assert_eq!(status, 429);
    let (status, _, _) = call(&limited, forwarded("2001:db8:0:2::1")).await;
    assert_eq!(status, 200);
}

#[test]
fn the_limit_keeps_a_bounded_number_of_windows() {
    let mut limiter = super::limits::RateLimiter::new(1, 60);
    let now = tokio::time::Instant::now();
    for n in 0..super::limits::MAX_WINDOWS {
        assert!(limiter.consume(&n.to_string(), now));
    }
    // Every client beyond shares one window.
    assert!(limiter.consume("new client", now));
    assert!(!limiter.consume("another new client", now));
    assert!(!limiter.consume("0", now));
    assert_eq!(limiter.len(), super::limits::MAX_WINDOWS + 1);
}

#[tokio::test(start_paused = true)]
async fn at_most_eight_posts_are_served_at_once() {
    let (entered, release) = (
        Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        Arc::new(Notify::new()),
    );
    let (counter, gate) = (Arc::clone(&entered), Arc::clone(&release));
    let pipeline = Arc::new(pipeline_with(config(), move |route, request| {
        let (counter, gate) = (Arc::clone(&counter), Arc::clone(&gate));
        Box::pin(async move {
            counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            gate.notified().await;
            echo(route, request).await
        })
    }));
    let posts: Vec<_> = (0..9)
        .map(|_| {
            let pipeline = Arc::clone(&pipeline);
            tokio::spawn(async move { call(&pipeline, post("{}", &[])).await.0 })
        })
        .collect();
    tokio::time::sleep(Duration::from_millis(10)).await;
    assert_eq!(entered.load(std::sync::atomic::Ordering::SeqCst), 8);
    release.notify_waiters();
    tokio::time::sleep(Duration::from_millis(10)).await;
    assert_eq!(entered.load(std::sync::atomic::Ordering::SeqCst), 9);
    release.notify_waiters();
    for post in posts {
        assert_eq!(post.await.unwrap(), 200);
    }
}
