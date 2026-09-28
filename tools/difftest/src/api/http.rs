//! A small HTTP/1.1 client for the route corpus: one connection per request (`connection: close`),
//! optionally from a given local address, so a section of the corpus can come from an address
//! that the nodes' rate-limit whitelist does not name.

use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpSocket;

/// How long one request may take.
const TIMEOUT: Duration = Duration::from_secs(10);

/// A request as the runner sends it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Request {
    /// The method.
    pub method: String,
    /// The path.
    pub path: String,
    /// The raw query string, empty for none.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub query: String,
    /// Extra headers.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub headers: Vec<(String, String)>,
    /// The body, as text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
}

/// A node's answer, or why there is none.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Reply {
    /// An HTTP answer.
    Answer {
        /// The status.
        status: u16,
        /// The headers, by lowercase name.
        headers: Map<String, Value>,
        /// The body: JSON when it parses, else its text.
        body: Value,
    },
    /// No answer.
    Failed {
        /// Why.
        error: String,
    },
}

/// A node's API, from a URL such as `http://192.0.2.1:4003`.
#[derive(Debug, Clone)]
pub struct Node {
    host: String,
    address: SocketAddr,
}

impl Node {
    /// The node at `url` (`http://` only).
    pub fn parse(url: &str) -> Result<Node, String> {
        let rest = url
            .strip_prefix("http://")
            .ok_or_else(|| format!("{url}: only http:// is served"))?;
        let host = rest.split('/').next().unwrap_or(rest);
        let with_port = if host.rsplit(']').next().unwrap_or(host).contains(':') {
            host.to_owned()
        } else {
            format!("{host}:80")
        };
        let address = with_port
            .to_socket_addrs()
            .map_err(|e| format!("{url}: {e}"))?
            .next()
            .ok_or_else(|| format!("{url}: no address"))?;
        Ok(Node {
            host: host.to_owned(),
            address,
        })
    }

    /// Send `request`, from `source` when given.
    pub async fn send(&self, request: &Request, source: Option<IpAddr>) -> Reply {
        let bytes = self.encode(request);
        match tokio::time::timeout(TIMEOUT, exchange(self.address, source, bytes)).await {
            Ok(Ok(raw)) => parse(&raw).unwrap_or_else(|error| Reply::Failed { error }),
            Ok(Err(error)) => Reply::Failed {
                error: error.to_string(),
            },
            Err(_) => Reply::Failed {
                error: format!("no answer within {} s", TIMEOUT.as_secs()),
            },
        }
    }

    fn encode(&self, request: &Request) -> Vec<u8> {
        let target = if request.query.is_empty() {
            request.path.clone()
        } else {
            format!("{}?{}", request.path, request.query)
        };
        let mut head = format!(
            "{} {target} HTTP/1.1\r\nhost: {}\r\nconnection: close\r\n",
            request.method, self.host
        );
        for (name, value) in &request.headers {
            head.push_str(&format!("{name}: {value}\r\n"));
        }
        if let Some(body) = &request.body {
            head.push_str(&format!("content-length: {}\r\n", body.len()));
        }
        head.push_str("\r\n");
        let mut bytes = head.into_bytes();
        bytes.extend(request.body.as_deref().unwrap_or("").as_bytes());
        bytes
    }
}

async fn exchange(
    address: SocketAddr,
    source: Option<IpAddr>,
    bytes: Vec<u8>,
) -> std::io::Result<Vec<u8>> {
    let socket = if address.is_ipv4() {
        TcpSocket::new_v4()?
    } else {
        TcpSocket::new_v6()?
    };
    if let Some(source) = source {
        socket.bind(SocketAddr::new(source, 0))?;
    }
    let mut stream = socket.connect(address).await?;
    // A server may answer and close before it has read a large body; its answer still counts.
    let written = stream.write_all(&bytes).await;
    let mut out = Vec::new();
    let read = stream.read_to_end(&mut out).await;
    match (written, read) {
        (_, Ok(_)) if !out.is_empty() => Ok(out),
        (Err(error), _) | (_, Err(error)) => Err(error),
        (Ok(()), Ok(_)) => Ok(out),
    }
}

/// An HTTP/1.1 answer: the status line, the headers, and the body (chunked or not).
fn parse(raw: &[u8]) -> Result<Reply, String> {
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or("an answer without the end of its headers")?;
    let head = String::from_utf8_lossy(raw.get(..split).unwrap_or_default());
    let mut lines = head.split("\r\n");
    let status = lines
        .next()
        .and_then(|line| line.split(' ').nth(1))
        .and_then(|code| code.parse().ok())
        .ok_or("an answer without a status")?;
    let mut headers = Map::new();
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let name = name.trim().to_ascii_lowercase();
        let value = value.trim().to_owned();
        let joined = match headers.get(&name).and_then(Value::as_str) {
            Some(before) => format!("{before}, {value}"),
            None => value,
        };
        headers.insert(name, Value::String(joined));
    }
    let rest = raw.get(split + 4..).unwrap_or_default();
    let chunked = headers
        .get("transfer-encoding")
        .and_then(Value::as_str)
        .is_some_and(|v| v.eq_ignore_ascii_case("chunked"));
    let body = if chunked {
        dechunk(rest)?
    } else {
        rest.to_vec()
    };
    let body = serde_json::from_slice(&body)
        .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&body).into_owned()));
    Ok(Reply::Answer {
        status,
        headers,
        body,
    })
}

fn dechunk(mut rest: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    loop {
        let end = rest
            .windows(2)
            .position(|w| w == b"\r\n")
            .ok_or("a chunk without its size line")?;
        let line = String::from_utf8_lossy(rest.get(..end).unwrap_or_default());
        let size = usize::from_str_radix(line.split(';').next().unwrap_or("").trim(), 16)
            .map_err(|_| format!("a chunk size {line:?}"))?;
        if size == 0 {
            return Ok(out);
        }
        let data = rest
            .get(end + 2..end + 2 + size)
            .ok_or("a chunk shorter than its size")?;
        out.extend_from_slice(data);
        rest = rest.get(end + 4 + size..).unwrap_or_default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_parse_with_and_without_chunks() {
        let plain =
            b"HTTP/1.1 404 Not Found\r\nX-Block-Height: 7\r\nconnection: close\r\n\r\n{\"a\":1}";
        let Reply::Answer {
            status,
            headers,
            body,
        } = parse(plain).unwrap()
        else {
            panic!("an answer")
        };
        assert_eq!(
            (status, &headers["x-block-height"], body),
            (404, &Value::from("7"), serde_json::json!({"a": 1}))
        );
        let chunked = b"HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\n3\r\nabc\r\n2\r\nde\r\n0\r\n\r\n";
        let Reply::Answer { body, .. } = parse(chunked).unwrap() else {
            panic!("an answer")
        };
        assert_eq!(body, Value::from("abcde"));
    }
}
