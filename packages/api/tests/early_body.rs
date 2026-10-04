//! A client that ends its side inside a body. The reference's server reads the body of a `POST`,
//! of a preflight and of a request no route takes before it answers, and answers nothing when the
//! connection ends first. This server answers a preflight and an unmatched request without
//! reading the body, and a `POST /api/transactions` whose body ends early with a 422.

mod snapshot;

use std::net::SocketAddr;
use std::sync::Arc;

use serde_json::Value;
use snapshot::{context, local_config};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// The status line and JSON body of the answer to `head` and `sent`, the part of a body the
/// client sends, after which it ends its side of the connection.
async fn early(address: SocketAddr, head: &str, sent: &str) -> (String, Value) {
    let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
    stream.write_all(head.as_bytes()).await.unwrap();
    stream.write_all(sent.as_bytes()).await.unwrap();
    stream.shutdown().await.unwrap();
    let mut answer = Vec::new();
    let _ = stream.read_to_end(&mut answer).await;
    let text = String::from_utf8_lossy(&answer).into_owned();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    let line = head.lines().next().unwrap_or_default().to_owned();
    (line, serde_json::from_str(body).unwrap_or(Value::Null))
}

#[tokio::test]
#[ignore = "integration"]
async fn a_body_that_ends_early_is_answered_without_waiting_for_the_rest() {
    let context = context(
        "early-body",
        snapshot::chain().pool(Vec::new()),
        Vec::new(),
        local_config(),
    );
    let address = context.config.address;
    let server =
        heartwood_api::server::serve(Arc::new(context), &tokio::runtime::Handle::current())
            .unwrap();

    // A submission whose body ends inside its `content-length` is answered with a 422.
    let (line, body) = early(
        address,
        "POST /api/transactions HTTP/1.1\r\nhost: node\r\ncontent-length: 64\r\n\r\n",
        r#"{"transactions":["#,
    )
    .await;
    assert_eq!(line, "HTTP/1.1 422 Unprocessable Entity");
    assert_eq!(body["statusCode"], 422, "{body}");

    // A preflight and a request no route takes are answered without their bodies being read.
    let (line, _) = early(
        address,
        "OPTIONS /api/transactions HTTP/1.1\r\nhost: node\r\norigin: o\r\n\
         access-control-request-method: POST\r\ncontent-length: 64\r\n\r\n",
        "{",
    )
    .await;
    assert!(line.starts_with("HTTP/1.1 2"), "{line}");
    let (line, body) = early(
        address,
        "GET /api/nothing HTTP/1.1\r\nhost: node\r\ncontent-length: 64\r\n\r\n",
        "{",
    )
    .await;
    assert_eq!(line, "HTTP/1.1 404 Not Found");
    assert_eq!(body["statusCode"], 404, "{body}");
    server.stop().await;
}
