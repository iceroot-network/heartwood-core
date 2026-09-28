//! The served API end to end over HTTP, on the committed devnet chain at its tip: the item and
//! list envelopes, the headers of every answer (errors included), CORS, trailing slashes, 404 for
//! unknown paths and other methods, the body limit, a preflight, and the component's start and
//! stop. These run with the store's readers, the query grammar and the list envelope of the other
//! parts of the crate, so they are integration tests.

mod snapshot;

use std::sync::Arc;

use heartwood_api::server::{ApiConfig, serve};
use heartwood_api::{Api, ServeError};
use heartwood_kernel::config::{Env, Options};
use heartwood_kernel::lifecycle::Component;
use serde_json::{Value, json};

use snapshot::{chain, context, get, http, local_config, serve_chain};

fn boom(status: u16, error: &str, message: &str) -> Value {
    json!({ "statusCode": status, "error": error, "message": message })
}

#[tokio::test]
#[ignore = "integration"]
async fn every_answer_has_the_envelope_and_the_headers() {
    let chain = chain();
    let (address, server) =
        serve_chain("envelope", chain.pool(chain.transactions()), Vec::new()).await;
    let item = get(address, "/api/node/syncing/").await;
    assert_eq!(item.status, 200);
    assert_eq!(item.body["data"]["height"], 265);
    let list = get(address, "/api/transactions/unconfirmed?limit=5&page=2").await;
    let meta = list.body["meta"].as_object().unwrap();
    let keys: Vec<&String> = meta.keys().collect();
    assert_eq!(
        keys,
        [
            "totalCountIsEstimate",
            "count",
            "pageCount",
            "totalCount",
            "next",
            "previous",
            "self",
            "first",
            "last"
        ]
    );
    assert_eq!(
        (meta["count"].clone(), meta["totalCount"].clone()),
        (json!(5), json!(42))
    );
    for answer in [&item, &list] {
        assert_eq!(answer.header("x-block-height"), Some("265"));
        assert_eq!(answer.header("connection"), Some("close"));
        assert_eq!(
            answer.header("content-type"),
            Some("application/json; charset=utf-8")
        );
        assert_eq!(answer.header("accept-ranges"), Some("bytes"));
        assert_eq!(
            answer.header("access-control-expose-headers"),
            Some("WWW-Authenticate,Server-Authorization")
        );
    }
    server.stop().await;
}

#[tokio::test]
#[ignore = "integration"]
async fn errors_are_boom_bodies_with_the_same_headers() {
    let chain = chain();
    let (address, server) = serve_chain("errors", chain.pool(Vec::new()), Vec::new()).await;
    let not_found = boom(404, "Not Found", "Not Found");
    for (method, target) in [
        ("GET", "/api/nothing"),
        ("DELETE", "/api/node/status"),
        ("POST", "/api/node/status"),
        ("GET", "/api/node/status//"),
    ] {
        let answer = http(address, method, target, &[], b"").await;
        assert_eq!(
            (answer.status, &answer.body),
            (404, &not_found),
            "{method} {target}"
        );
        assert_eq!(answer.header("x-block-height"), Some("265"));
        assert_eq!(answer.header("connection"), Some("close"));
        assert_eq!(
            answer.header("access-control-expose-headers"),
            Some("WWW-Authenticate,Server-Authorization")
        );
        assert_eq!(answer.header("accept-ranges"), None);
    }
    let large = vec![b' '; 1_048_577];
    let answer = http(
        address,
        "POST",
        "/api/transactions",
        &[("content-type", "application/json")],
        &large,
    )
    .await;
    let message = "Payload content length greater than maximum allowed: 1048576";
    assert_eq!(
        (answer.status, answer.body),
        (413, boom(413, "Payload Too Large", message))
    );
    let answer = http(
        address,
        "GET",
        "/api/transactions/unconfirmed?limit=0",
        &[],
        b"",
    )
    .await;
    assert_eq!(answer.status, 422);
    server.stop().await;
}

#[tokio::test]
#[ignore = "integration"]
async fn cors_and_the_preflight() {
    let chain = chain();
    let (address, server) = serve_chain("cors", chain.pool(Vec::new()), Vec::new()).await;
    let origin = ("origin", "https://wallet.example");
    let answer = http(address, "GET", "/api/node/status", &[origin], b"").await;
    assert_eq!(
        answer.header("access-control-allow-origin"),
        Some("https://wallet.example")
    );
    assert_eq!(answer.header("vary"), Some("origin"));
    let preflight = http(
        address,
        "OPTIONS",
        "/api/transactions",
        &[origin, ("access-control-request-method", "POST")],
        b"",
    )
    .await;
    assert_eq!(preflight.status, 204);
    assert_eq!(
        preflight.header("access-control-allow-methods"),
        Some("POST")
    );
    server.stop().await;
}

#[tokio::test]
#[ignore = "integration"]
async fn the_component_serves_until_disposed_and_refuses_https() {
    let chain = chain();
    let config = local_config();
    let address = config.address;
    let served = context("component", chain.pool(Vec::new()), Vec::new(), config);
    let mut api = Api::new(Arc::new(served), tokio::runtime::Handle::current());
    assert!(api.enabled());
    api.boot().await.unwrap();
    assert_eq!(get(address, "/api/node/status").await.status, 200);
    api.dispose().await;
    assert!(tokio::net::TcpStream::connect(address).await.is_err());
    // HTTPS is not served: the start fails, and the relay stops (the component is required).
    let env = Env::from_vars([("CORE_API_SSL".to_owned(), "true".to_owned())]);
    let https = ApiConfig::from_options(&json!({}), &env).unwrap();
    let refusing = Arc::new(context("https", chain.pool(Vec::new()), Vec::new(), https));
    let refused = serve(Arc::clone(&refusing), &tokio::runtime::Handle::current()).unwrap_err();
    assert_eq!(refused, ServeError::Https);
    let mut api = Api::new(refusing, tokio::runtime::Handle::current());
    let failure = api.boot().await.unwrap_err();
    assert_eq!(failure.package, "@solar-network/api");
    const { assert!(Api::REQUIRED) };
}
