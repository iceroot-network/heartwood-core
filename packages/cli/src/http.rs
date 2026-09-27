//! A blocking HTTP/1.1 GET to a loopback address, over `std::net`, for the devnet's reads of its
//! relays' APIs: no HTTP client crate, no TLS, no redirects. The request asks the server to close
//! the connection, and the response is read until it does; its body is taken by its
//! `Content-Length`, by its chunks, or whole, up to [`MAX_RESPONSE`] bytes.

use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

use serde_json::Value;

use crate::errors::HttpError;
use crate::seams::{ApiBlock, ApiNodeStatus, ApiPeer, NodeApi};

/// The longest response read.
pub const MAX_RESPONSE: usize = 16 * 1024 * 1024;

/// An HTTP response: its status and body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    /// The status code.
    pub status: u16,
    /// The body.
    pub body: Vec<u8>,
}

/// `GET <path>` from the loopback `address`, each step (the connection, the request, each read)
/// within `timeout`.
pub fn get(address: SocketAddr, path: &str, timeout: Duration) -> Result<Response, HttpError> {
    let owned = || path.to_owned();
    let failed = |message: String| HttpError::Connection {
        address,
        path: owned(),
        message,
    };
    let io_error = |error: std::io::Error| match error.kind() {
        ErrorKind::WouldBlock | ErrorKind::TimedOut => HttpError::Timeout {
            address,
            path: owned(),
        },
        _ => failed(error.to_string()),
    };
    if !address.ip().is_loopback() {
        return Err(HttpError::NotLoopback { address });
    }
    if !path.starts_with('/') || path.bytes().any(|b| b <= b' ' || b == 0x7f) {
        return Err(failed("the path is not a request target".to_owned()));
    }
    let mut stream = TcpStream::connect_timeout(&address, timeout).map_err(io_error)?;
    let timeouts = stream.set_read_timeout(Some(timeout));
    timeouts
        .and_then(|()| stream.set_write_timeout(Some(timeout)))
        .map_err(io_error)?;
    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: {address}\r\nAccept: application/json\r\n\
         Connection: close\r\n\r\n"
    );
    stream.write_all(request.as_bytes()).map_err(io_error)?;
    let limit = u64::try_from(MAX_RESPONSE.saturating_add(64 * 1024)).unwrap_or(u64::MAX);
    let mut raw = Vec::new();
    stream.take(limit).read_to_end(&mut raw).map_err(io_error)?;
    parse(&raw).map_err(|reason| HttpError::Response {
        address,
        path: owned(),
        reason,
    })
}

/// The position of the first `needle` in `data`.
fn find(data: &[u8], needle: &[u8]) -> Option<usize> {
    data.windows(needle.len())
        .position(|window| window == needle)
}

/// The response in `raw`, the whole of what the server sent.
fn parse(raw: &[u8]) -> Result<Response, String> {
    let end = find(raw, b"\r\n\r\n").ok_or("it has no complete head")?;
    let head = String::from_utf8_lossy(raw.get(..end).unwrap_or_default());
    let mut lines = head.split("\r\n");
    let first = lines.next().unwrap_or_default();
    let mut parts = first.split(' ');
    let status = match (parts.next(), parts.next().map(str::parse)) {
        (Some("HTTP/1.1" | "HTTP/1.0"), Some(Ok(status))) => status,
        _ => return Err(format!("{first:?} is not a status line")),
    };
    let fields = lines.filter_map(|line| line.split_once(':'));
    let fields: Vec<_> = fields
        .map(|(n, v)| (n.trim().to_ascii_lowercase(), v.trim()))
        .collect();
    let field = |name: &str| {
        fields
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, value)| *value)
    };
    let chunked = field("transfer-encoding").is_some_and(|v| v.contains("chunked"));
    let rest = raw.get(end.saturating_add(4)..).unwrap_or_default();
    let body = match (chunked, field("content-length")) {
        (true, _) => dechunk(rest).ok_or("its chunks are not valid")?,
        (false, Some(length)) => {
            let length: usize = length
                .parse()
                .map_err(|_| format!("{length:?} is no length"))?;
            rest.get(..length)
                .ok_or("it is shorter than its Content-Length")?
                .to_vec()
        }
        (false, None) => rest.to_vec(),
    };
    if body.len() > MAX_RESPONSE {
        return Err(format!("its body is longer than {MAX_RESPONSE} bytes"));
    }
    Ok(Response { status, body })
}

/// The body of a chunked transfer.
fn dechunk(mut data: &[u8]) -> Option<Vec<u8>> {
    let mut body = Vec::new();
    loop {
        let end = find(data, b"\r\n")?;
        let line = std::str::from_utf8(data.get(..end)?).ok()?;
        let size = usize::from_str_radix(line.split(';').next()?.trim(), 16).ok()?;
        data = data.get(end.checked_add(2)?..)?;
        if size == 0 {
            return Some(body);
        }
        body.extend_from_slice(data.get(..size)?);
        data = data.get(size..)?.strip_prefix(b"\r\n")?;
    }
}

/// The [`NodeApi`] over [`get`], each read within `timeout`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoopbackApi {
    /// The time each step of a read may take.
    pub timeout: Duration,
}

impl LoopbackApi {
    /// What `read` takes from the `data` of the answer to `GET path`, which must be 200 and JSON.
    fn read<T>(
        &self,
        api: SocketAddr,
        path: &str,
        read: impl Fn(&Value) -> Option<T>,
    ) -> Result<T, HttpError> {
        let response = get(api, path, self.timeout)?;
        let (address, status, path) = (api, response.status, path.to_owned());
        if status != 200 {
            return Err(HttpError::Status {
                address,
                path,
                status,
            });
        }
        let value = serde_json::from_slice::<Value>(&response.body).ok();
        let read = value.as_ref().and_then(|value| read(value.get("data")?));
        let reason = "its data is missing or not what the reference answers".to_owned();
        read.ok_or(HttpError::Body {
            address,
            path,
            reason,
        })
    }
}

fn height(value: &Value) -> Option<u32> {
    u32::try_from(value.as_u64()?).ok()
}

fn block(data: &Value) -> Option<ApiBlock> {
    let id = data.get("id")?.as_str()?.to_owned();
    Some(ApiBlock {
        id,
        height: height(data.get("height")?)?,
    })
}

impl NodeApi for LoopbackApi {
    fn node_status(&self, api: SocketAddr) -> Result<ApiNodeStatus, HttpError> {
        self.read(api, "/api/node/status", |data| {
            let (synced, now) = (data.get("synced")?.as_bool()?, height(data.get("now")?)?);
            let blocks_count = data.get("blocksCount")?.as_i64()?;
            Some(ApiNodeStatus {
                synced,
                now,
                blocks_count,
            })
        })
    }

    fn last_block(&self, api: SocketAddr) -> Result<ApiBlock, HttpError> {
        self.read(api, "/api/blocks/last", block)
    }

    fn block_at(&self, api: SocketAddr, height: u32) -> Result<Option<ApiBlock>, HttpError> {
        match self.read(api, &format!("/api/blocks/{height}"), block) {
            Err(HttpError::Status { status: 404, .. }) => Ok(None),
            answer => answer.map(Some),
        }
    }

    fn peers(&self, api: SocketAddr) -> Result<Vec<ApiPeer>, HttpError> {
        let peer = |peer: &Value| {
            let ip = peer.get("ip")?.as_str()?.to_owned();
            let port = u16::try_from(peer.get("port")?.as_u64()?).ok()?;
            Some(ApiPeer {
                ip,
                port,
                height: peer.get("height").and_then(height),
            })
        };
        self.read(api, "/api/peers", |data| {
            data.as_array()?.iter().map(peer).collect()
        })
    }
}

#[cfg(test)]
mod tests {
    use std::net::TcpListener;
    use std::thread;

    use super::*;

    /// A server on a loopback port that answers one connection with `answer` and closes it, and
    /// gives the request it read.
    fn serve(answer: &'static [u8]) -> (SocketAddr, thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let handle = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut byte = [0u8; 1];
            while !request.ends_with(b"\r\n\r\n") && socket.read(&mut byte).unwrap() == 1 {
                request.push(byte[0]);
            }
            socket.write_all(answer).unwrap();
            String::from_utf8(request).unwrap()
        });
        (address, handle)
    }

    const SECOND: Duration = Duration::from_secs(1);

    #[test]
    fn a_get_sends_a_closing_request_and_reads_the_body_by_its_length() {
        let (address, server) =
            serve(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello and more");
        let response = get(address, "/api/node/status", SECOND).unwrap();
        assert_eq!(
            response,
            Response {
                status: 200,
                body: b"hello".to_vec()
            }
        );
        let request = server.join().unwrap();
        assert!(request.starts_with("GET /api/node/status HTTP/1.1\r\n"));
        assert!(request.contains("Connection: close\r\n"));
        assert!(request.contains(&format!("Host: {address}\r\n")));
    }

    #[test]
    fn chunked_and_closing_bodies_are_read_whole() {
        let chunked = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4\r\nheal\r\n5;x=1\r\nthwod\r\n0\r\n\r\n";
        let (address, _server) = serve(chunked);
        assert_eq!(get(address, "/", SECOND).unwrap().body, b"healthwod");
        let (address, _server) = serve(b"HTTP/1.0 404 Not Found\r\n\r\nnone");
        assert_eq!(
            get(address, "/", SECOND).unwrap(),
            Response {
                status: 404,
                body: b"none".to_vec()
            }
        );
    }

    #[test]
    fn bad_answers_addresses_and_paths_are_refused() {
        let (address, _server) = serve(b"SSH-2.0-OpenSSH\r\n\r\n");
        assert!(matches!(
            get(address, "/", SECOND),
            Err(HttpError::Response { .. })
        ));
        let (address, _server) = serve(b"HTTP/1.1 200 OK\r\nContent-Length: 9\r\n\r\nshort");
        assert!(matches!(
            get(address, "/", SECOND),
            Err(HttpError::Response { .. })
        ));
        let (address, _server) =
            serve(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nzz\r\n");
        assert!(matches!(
            get(address, "/", SECOND),
            Err(HttpError::Response { .. })
        ));
        let (address, _server) = serve(b"HTTP/1.1 200 OK\r\n");
        assert!(matches!(
            get(address, "/", SECOND),
            Err(HttpError::Response { .. })
        ));
        let remote: SocketAddr = "192.0.2.1:6003".parse().unwrap();
        assert_eq!(
            get(remote, "/", SECOND),
            Err(HttpError::NotLoopback { address: remote })
        );
        let local: SocketAddr = "127.0.0.1:9".parse().unwrap();
        assert!(matches!(
            get(local, "/a b", SECOND),
            Err(HttpError::Connection { .. })
        ));
        assert!(matches!(
            get(local, "relative", SECOND),
            Err(HttpError::Connection { .. })
        ));
    }

    #[test]
    fn a_server_that_never_answers_times_out() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let _held =
            thread::spawn(move || listener.accept().map(|_socket| thread::sleep(SECOND * 2)));
        let answer = get(address, "/", Duration::from_millis(200));
        assert_eq!(
            answer,
            Err(HttpError::Timeout {
                address,
                path: "/".to_owned()
            })
        );
    }

    #[test]
    fn a_closed_port_is_a_connection_error() {
        let address = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap();
        assert!(matches!(
            get(address, "/", SECOND),
            Err(HttpError::Connection { .. })
        ));
    }

    #[test]
    fn the_api_reads_its_four_answers() {
        let api = LoopbackApi { timeout: SECOND };
        let (address, _s) = serve(b"HTTP/1.1 200 OK\r\n\r\n{\"data\":{\"synced\":true,\"now\":12,\"blocksCount\":-1,\"timestamp\":null}}");
        assert_eq!(
            api.node_status(address).unwrap(),
            ApiNodeStatus {
                synced: true,
                now: 12,
                blocks_count: -1
            }
        );
        let (address, _s) = serve(
            b"HTTP/1.1 200 OK\r\n\r\n{\"data\":{\"id\":\"abc\",\"height\":12,\"version\":0}}",
        );
        assert_eq!(
            api.last_block(address).unwrap(),
            ApiBlock {
                id: "abc".to_owned(),
                height: 12
            }
        );
        let (address, _s) = serve(b"HTTP/1.1 404 Not Found\r\n\r\n{\"statusCode\":404}");
        assert_eq!(api.block_at(address, 99).unwrap(), None);
        let (address, _s) = serve(b"HTTP/1.1 200 OK\r\n\r\n{\"meta\":{},\"data\":[{\"ip\":\"127.0.0.12\",\"port\":6002,\"height\":12},{\"ip\":\"127.0.0.13\",\"port\":6002}]}");
        assert_eq!(
            api.peers(address).unwrap(),
            vec![
                ApiPeer {
                    ip: "127.0.0.12".to_owned(),
                    port: 6002,
                    height: Some(12)
                },
                ApiPeer {
                    ip: "127.0.0.13".to_owned(),
                    port: 6002,
                    height: None
                },
            ]
        );
        let (address, _s) = serve(b"HTTP/1.1 200 OK\r\n\r\n{\"data\":{\"id\":\"abc\"}}");
        assert!(matches!(
            api.last_block(address),
            Err(HttpError::Body { .. })
        ));
        let (address, _s) = serve(b"HTTP/1.1 200 OK\r\n\r\nnot json");
        assert!(matches!(
            api.last_block(address),
            Err(HttpError::Body { .. })
        ));
        let (address, _s) = serve(b"HTTP/1.1 503 Service Unavailable\r\n\r\n{}");
        assert!(matches!(
            api.node_status(address),
            Err(HttpError::Status { status: 503, .. })
        ));
    }
}
