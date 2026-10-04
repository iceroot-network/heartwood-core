//! The API's root over HTTP, as a reference relay's port check asks it (`HEAD /`, status 200 or the
//! port counts as closed): `GET` and `HEAD /` answer 200 with a page that names the base path, the
//! OpenAPI document is served at `/api.json` (outside the base path, as the reference's own route),
//! and the documentation's other files, which the relay does not serve, stay 404.

mod snapshot;

use std::net::SocketAddr;

use snapshot::{chain, serve_chain};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// An answer as it came over the wire: its status, its headers (names in lowercase) and its body.
struct Raw {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Raw {
    fn header(&self, name: &str) -> Option<&str> {
        let found = self.headers.iter().find(|(key, _)| key == name);
        found.map(|(_, value)| value.as_str())
    }
}

/// `method target` to `address` over one connection, read to its end.
async fn raw(address: SocketAddr, method: &str, target: &str) -> Raw {
    let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
    let request = format!("{method} {target} HTTP/1.1\r\nhost: node\r\n\r\n");
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut answer = Vec::new();
    stream.read_to_end(&mut answer).await.unwrap();
    let end = answer.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    let head = String::from_utf8(answer[..end].to_vec()).unwrap();
    let mut lines = head.split("\r\n");
    let status = lines.next().unwrap()[9..12].parse().unwrap();
    let headers = lines
        .map(|line| {
            let (name, value) = line.split_once(": ").unwrap();
            (name.to_lowercase(), value.to_owned())
        })
        .collect();
    Raw {
        status,
        headers,
        body: answer[end + 4..].to_vec(),
    }
}

#[tokio::test]
#[ignore = "integration"]
async fn the_root_answers_200_with_a_page_and_the_documentation_files_404() {
    let chain = chain();
    let (address, server) = serve_chain("root", chain.pool(Vec::new()), Vec::new()).await;
    let page = raw(address, "GET", "/").await;
    assert_eq!(page.status, 200);
    assert_eq!(
        page.header("content-type"),
        Some("text/html; charset=utf-8")
    );
    assert_eq!(page.header("cache-control"), Some("no-cache"));
    assert_eq!(page.header("connection"), Some("close"));
    let text = String::from_utf8(page.body.clone()).unwrap();
    assert!(text.contains("href=\"/api\""), "{text}");
    let length = page.header("content-length").map(str::parse::<usize>);
    assert_eq!(length, Some(Ok(page.body.len())));
    // The port check's request: the same status and type, and no body.
    let head = raw(address, "HEAD", "/").await;
    assert_eq!(head.status, 200);
    assert_eq!(
        head.header("content-type"),
        Some("text/html; charset=utf-8")
    );
    assert!(head.body.is_empty());
    assert_eq!(head.header("content-length"), page.header("content-length"));
    // A path that the router resolves to the root is the root.
    assert_eq!(raw(address, "GET", "/a/..").await.status, 200);
    for file in ["/index.html", "/swagger-ui.css", "/swagger-ui.js"] {
        for method in ["GET", "HEAD"] {
            let answer = raw(address, method, file).await;
            assert_eq!(answer.status, 404, "{method} {file}");
        }
    }
    // The OpenAPI document, as JSON, with the plugins of any route; under the base path it is no
    // route.
    let document = raw(address, "GET", "/api.json").await;
    assert_eq!(document.status, 200);
    assert_eq!(document.header("connection"), Some("close"));
    assert!(document.header("x-block-height").is_some());
    let body: serde_json::Value = serde_json::from_slice(&document.body).unwrap();
    assert!(body["openapi"].is_string(), "{body}");
    assert!(body["paths"]["/blocks"].is_object(), "{body}");
    let head = raw(address, "HEAD", "/api.json").await;
    assert_eq!(head.status, 200);
    assert!(head.body.is_empty());
    assert_eq!(raw(address, "GET", "/api/api.json").await.status, 404);
    server.stop().await;
}
