//! What the reference's own clients and a plain HTTP client see of the peer-to-peer port.

#![cfg(feature = "node")]

use heartwood_p2p::server::listener::{Upgrade, upgrade};
use tokio::io::{AsyncReadExt, AsyncWriteExt, duplex};

#[tokio::test]
async fn a_plain_http_request_gets_the_404_with_the_reference_headers() {
    let (mut client, served) = duplex(4096);
    let task = tokio::spawn(upgrade(Box::new(served)));
    client
        .write_all(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n")
        .await
        .unwrap();
    let mut answer = String::new();
    client.read_to_string(&mut answer).await.unwrap();
    assert!(matches!(task.await.unwrap(), Upgrade::NotWebSocket));
    let lines: Vec<&str> = answer.split("\r\n").collect();
    // The order is the reference's: connection, cache-control, content-length, then the date.
    assert_eq!(
        lines[..4],
        [
            "HTTP/1.1 404 Not Found",
            "connection: close",
            "cache-control: no-cache",
            "content-length: 0"
        ]
    );
    let date = lines[4].strip_prefix("Date: ").expect("a Date header");
    // `Mon, 28 Sep 2026 21:27:48 GMT`
    let shape = |i: usize, c: char| date.chars().nth(i) == Some(c);
    assert_eq!(date.len(), 29, "{date}");
    assert!(shape(3, ',') && shape(16, ' ') && shape(19, ':') && date.ends_with(" GMT"));
    assert_eq!(lines[5..], ["", ""]);
}
