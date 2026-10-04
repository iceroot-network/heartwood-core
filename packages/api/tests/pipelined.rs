//! Requests a client sends after the first on one connection (pipelined). The reference's server
//! reads every request that came with the first and serves each one, so each is charged a
//! rate-limit point and runs its handler, though only the first is answered (its answer ends the
//! connection). This server reads the first request of a connection and nothing after it: one
//! answer and one point.
//!
//! The points are counted by what a second and a third client find: with two points in the
//! window, one point used leaves the second client its answer and the third a refusal, where
//! three points used would refuse both.

mod snapshot;

use std::net::SocketAddr;

use snapshot::{Answer, context, get, local_config};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// What `count` requests written at once to one connection at `address` got back, read to the
/// connection's end.
async fn pipelined(address: SocketAddr, count: usize) -> String {
    let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
    let request = "GET /api/node/status HTTP/1.1\r\nhost: node\r\n\r\n".repeat(count);
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut answer = Vec::new();
    stream.read_to_end(&mut answer).await.unwrap();
    String::from_utf8(answer).unwrap()
}

#[tokio::test]
#[ignore = "integration"]
async fn three_pipelined_requests_are_answered_once_and_charged_once() {
    let mut config = local_config();
    config.rate_limit.whitelist.clear();
    config.rate_limit.points = 2;
    let context = context(
        "pipelined",
        snapshot::chain().pool(Vec::new()),
        Vec::new(),
        config,
    );
    let address = context.config.address;
    let server = heartwood_api::server::serve(
        std::sync::Arc::new(context),
        &tokio::runtime::Handle::current(),
    )
    .unwrap();

    let answered = pipelined(address, 3).await;
    assert_eq!(
        answered.matches("HTTP/1.1 ").count(),
        1,
        "one answer, not three: {answered:?}"
    );
    assert!(answered.starts_with("HTTP/1.1 200 OK\r\n"), "{answered:?}");
    assert!(answered.contains("connection: close"), "{answered:?}");

    // One point was used: the window's second point is still there, and its third is not.
    let second: Answer = get(address, "/api/node/status").await;
    assert_eq!(
        second.status, 200,
        "the pipelined requests cost one point, not three"
    );
    let third: Answer = get(address, "/api/node/status").await;
    assert_eq!(third.status, 429);
    server.stop().await;
}
