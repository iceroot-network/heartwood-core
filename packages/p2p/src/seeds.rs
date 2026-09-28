//! Seeds, peer sources and the plugin port probes.
//!
//! The seeds are `peers.json`'s `list` plus the peers of its first `sources` entry, a plain HTTP
//! URL fetched with a hand-written `GET`. As in the reference, only the first entry is ever read,
//! and a source that cannot be read gives no peers. No seed at all stops the node. A source over
//! HTTPS is refused when the node starts ([`check_sources`]): the client has no TLS.
//!
//! A source that is a path starting with `/` is read as a local JSON file, as the reference's
//! branch for one does. No configuration reaches that branch on either side: `peers.json` takes
//! only URIs for its sources, as the reference's schema does, so only a caller that builds the
//! options itself can give a path.
//!
//! The port probes are a hand-written HTTP `HEAD /`; the communicator records the port on a 200 and
//! -1 otherwise. The reference writes the probe's URL as `http://<ip>:<port>/`, which for an IPv6
//! address names another host, so an IPv6 peer's probe never answers here either.
//!
//! Each read and the connection wait at most [`HTTP_TIMEOUT`], the reference's idle timeout.

use std::future::Future;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use heartwood_p2p_wire::replies::PeerAddress;
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;

use crate::errors::SeedError;
use crate::seams::PortProbe;

/// How long a connection, or one read of a response, may take (the reference's HTTP helper).
pub const HTTP_TIMEOUT: Duration = Duration::from_millis(1_500);
/// The largest response a peer source may give.
pub const MAX_SOURCE_BYTES: usize = 4 * 1024 * 1024;
/// The largest status response a port probe reads.
const MAX_PROBE_BYTES: usize = 8 * 1024;

/// Refuse an HTTPS source before the node starts, naming it (the reference fetches it).
pub fn check_sources(sources: &[String]) -> Result<(), SeedError> {
    match sources.iter().find(|source| is_https(source)) {
        Some(source) => Err(source_error(
            source,
            "HTTPS sources are not supported; use plain HTTP",
        )),
        None => Ok(()),
    }
}

fn is_https(source: &str) -> bool {
    source
        .get(..8)
        .is_some_and(|scheme| scheme.eq_ignore_ascii_case("https://"))
}

fn source_error(source: &str, reason: impl Into<String>) -> SeedError {
    SeedError::Source {
        source_name: source.to_owned(),
        reason: reason.into(),
    }
}

/// The peers of the first source in `sources`: none without a source, an error when it cannot be
/// read.
pub async fn load_sources(sources: &[String]) -> Result<Vec<PeerAddress>, SeedError> {
    let Some(source) = sources.first() else {
        return Ok(Vec::new());
    };
    let text = if source.starts_with('/') {
        std::fs::read(source).map_err(|e| source_error(source, e.to_string()))?
    } else {
        heartwood_kernel::debug!("GET {source}");
        http_get(source)
            .await
            .map_err(|e| source_error(source, e))?
    };
    let mut value: Value =
        serde_json::from_slice(&text).map_err(|e| source_error(source, e.to_string()))?;
    // A JSON text that holds JSON text is read again, as the reference reads a body.
    if let Value::String(inner) = &value {
        value = serde_json::from_str(inner).map_err(|e| source_error(source, e.to_string()))?;
    }
    parse_peers(&value).ok_or_else(|| source_error(source, "not a list of peers"))
}

/// The peers of a source's list: every entry with an address as text and a port (a number, or
/// decimal text) that fits one. `None` when the value is not a list.
pub fn parse_peers(value: &Value) -> Option<Vec<PeerAddress>> {
    let peer = |entry: &Value| {
        let port = match entry.get("port")? {
            Value::String(text) => text.parse().ok()?,
            port => u16::try_from(port.as_u64()?).ok()?,
        };
        let ip = entry.get("ip")?.as_str()?.to_owned();
        Some(PeerAddress { ip, port })
    };
    Some(value.as_array()?.iter().filter_map(peer).collect())
}

/// The seeds: `list`, then the peers of the sources whose IP no seed before them has, in order.
pub fn merge_seeds(list: &[PeerAddress], from_sources: &[PeerAddress]) -> Vec<PeerAddress> {
    let mut seeds = list.to_vec();
    for peer in from_sources {
        if !seeds.iter().any(|seed| seed.ip == peer.ip) {
            seeds.push(peer.clone());
        }
    }
    seeds
}

/// The body of a plain HTTP `GET` of `url`; an error for a response of 400 or more, as the
/// reference's helper rejects it.
async fn http_get(url: &str) -> Result<Vec<u8>, String> {
    let rest = url
        .strip_prefix("http://")
        .ok_or("only plain HTTP URLs are fetched")?;
    let (authority, path) = rest.find('/').map_or((rest, "/"), |at| rest.split_at(at));
    let default_port = !authority.contains(':') || authority.ends_with(']');
    let target = format!("{authority}{}", if default_port { ":80" } else { "" });
    let request =
        format!("GET {path} HTTP/1.0\r\nHost: {authority}\r\nAccept: application/json\r\n\r\n");
    let response = exchange(&target, &request, MAX_SOURCE_BYTES, HTTP_TIMEOUT, false).await?;
    match status_code(&response) {
        Some(status) if status < 400 => {
            let at = response.windows(4).position(|w| w == b"\r\n\r\n");
            Ok(at
                .and_then(|at| response.get(at.saturating_add(4)..))
                .unwrap_or_default()
                .to_vec())
        }
        Some(status) => Err(format!("the server answered {status}")),
        None => Err("the response is not HTTP".to_owned()),
    }
}

/// `step` within `wait`, its failure as text.
async fn within<T>(wait: Duration, step: impl Future<Output = io::Result<T>>) -> Result<T, String> {
    timeout(wait, step)
        .await
        .map_err(|_| "timed out".to_owned())?
        .map_err(|e| e.to_string())
}

/// Connect to `target`, send `request` and read the response to its end, or only its first line
/// when `status_only` (at most `max` bytes), each step within `wait`.
async fn exchange(
    target: &str,
    request: &str,
    max: usize,
    wait: Duration,
    status_only: bool,
) -> Result<Vec<u8>, String> {
    let mut stream = within(wait, TcpStream::connect(target)).await?;
    within(wait, stream.write_all(request.as_bytes())).await?;
    let (mut response, mut chunk) = (Vec::new(), [0u8; 8192]);
    loop {
        let read = within(wait, stream.read(&mut chunk)).await?;
        let part = chunk.get(..read).unwrap_or_default();
        if part.is_empty() {
            return Ok(response);
        }
        if response.len().saturating_add(part.len()) > max {
            return Err("the response is too large".to_owned());
        }
        response.extend_from_slice(part);
        if status_only && response.windows(2).any(|w| w == b"\r\n") {
            return Ok(response);
        }
    }
}

/// The status code of the status line `HTTP/1.x NNN ...`.
fn status_code(response: &[u8]) -> Option<u16> {
    let line = response.split(|&b| b == b'\r' || b == b'\n').next()?;
    let mut parts = std::str::from_utf8(line).ok()?.split(' ');
    parts
        .next()
        .filter(|version| version.starts_with("HTTP/"))?;
    parts.next().filter(|code| code.len() == 3)?.parse().ok()
}

/// The HTTP probe of plugin ports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HttpProbe {
    /// How long the connection, and each read of the response, may take.
    pub timeout: Duration,
}

impl Default for HttpProbe {
    fn default() -> HttpProbe {
        HttpProbe {
            timeout: HTTP_TIMEOUT,
        }
    }
}

impl PortProbe for HttpProbe {
    async fn head(&self, ip: IpAddr, port: u16) -> Option<u16> {
        if ip.is_ipv6() {
            return None;
        }
        let target = SocketAddr::new(ip, port).to_string();
        let request = format!("HEAD / HTTP/1.0\r\nHost: {target}\r\n\r\n");
        let response = exchange(&target, &request, MAX_PROBE_BYTES, self.timeout, true).await;
        status_code(&response.ok()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn peer(ip: &str, port: u16) -> PeerAddress {
        PeerAddress {
            ip: ip.to_owned(),
            port,
        }
    }

    #[test]
    fn source_lists_and_seeds_merge_by_ip() {
        let parsed = parse_peers(&json!([
            {"ip": "192.0.2.1", "port": 4002},
            {"ip": "192.0.2.2", "port": "4003"},
            {"ip": "192.0.2.3", "port": 70000},
            {"ip": 5, "port": 4002},
            {"port": 4002},
        ]))
        .unwrap();
        assert_eq!(parsed, [peer("192.0.2.1", 4002), peer("192.0.2.2", 4003)]);
        assert!(parse_peers(&json!({"list": []})).is_none());
        let list = [peer("192.0.2.1", 4000)];
        let merged = merge_seeds(
            &list,
            &[parsed[0].clone(), parsed[1].clone(), parsed[1].clone()],
        );
        assert_eq!(merged, [peer("192.0.2.1", 4000), peer("192.0.2.2", 4003)]);
    }

    #[test]
    fn https_sources_are_refused_and_responses_parsed() {
        assert!(check_sources(&["/etc/peers.json".into(), "http://192.0.2.1/p".into()]).is_ok());
        let refused = check_sources(&["http://a".into(), "HTTPS://b/peers".into()]);
        assert!(
            matches!(&refused, Err(SeedError::Source { source_name, .. }) if source_name == "HTTPS://b/peers")
        );
        // It advises only what `peers.json` accepts: a path is not a URI there.
        let reason = refused.unwrap_err().to_string();
        assert!(reason.ends_with("use plain HTTP"), "{reason}");
        assert_eq!(status_code(b"HTTP/1.0 404 Not Found\r\n"), Some(404));
        assert_eq!(status_code(b"SSH-2.0\r\n"), None);
        assert_eq!(status_code(b"HTTP/1.1 20 OK\r\n"), None);
    }
}
