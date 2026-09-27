//! `peers.json`: the seed peers and the peer sources, with the reference's schema: an object of
//! only `list` (required: objects of only `ip`, an IPv4 or IPv6 address with an optional prefix
//! length, and `port`, a port number or its text) and `sources` (URIs). The URI rule is the
//! reference's in its form (a scheme, then only the characters a URI may hold, with `%` escapes);
//! a path such as `/peers.json` is no URI, so a local file cannot be a source, as in the
//! reference. The relay then refuses a source over HTTPS, which the reference fetches, with the
//! peer-to-peer client's check, since the client has no TLS.

use std::net::Ipv6Addr;
use std::path::{Path, PathBuf};

use serde_json::Value;
use thiserror::Error;

use crate::config::app::{integer, only};

/// One seed peer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerData {
    /// The address, as the file gives it.
    pub ip: String,
    /// The P2P port.
    pub port: u16,
}

/// `peers.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PeersFile {
    /// The seed peers, in file order.
    pub list: Vec<PeerData>,
    /// The peer sources, in file order.
    pub sources: Vec<String>,
}

/// `peers.json` could not be read.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PeersError {
    /// The file could not be read.
    #[error("cannot read {}: {detail}", path.display())]
    Read {
        /// The file.
        path: PathBuf,
        /// Why.
        detail: String,
    },
    /// The file is not JSON, or its schema refuses it.
    #[error("peers.json is not valid: {detail}")]
    Schema {
        /// What is wrong.
        detail: String,
    },
}

/// `<config_dir>/peers.json`.
pub fn load(config_dir: &Path) -> Result<PeersFile, PeersError> {
    let path = config_dir.join("peers.json");
    let text = std::fs::read_to_string(&path).map_err(|error| PeersError::Read {
        path,
        detail: error.to_string(),
    })?;
    parse(&text)
}

/// The `peers.json` text `text`.
pub fn parse(text: &str) -> Result<PeersFile, PeersError> {
    let schema = |detail: String| PeersError::Schema { detail };
    let json: Value = serde_json::from_str(text).map_err(|error| schema(error.to_string()))?;
    let peer = |entry: &Value| {
        let entry = only(entry, &["ip", "port"])?;
        let ip = entry.get("ip")?.as_str().filter(|ip| is_ip(ip))?.to_owned();
        let port = u16::try_from(integer(entry.get("port")?, 0, 65_535)?).ok()?;
        Some(PeerData { ip, port })
    };
    let uri = |source: &Value| source.as_str().filter(|uri| is_uri(uri)).map(str::to_owned);
    let file = only(&json, &["list", "sources"]).and_then(|file| {
        let list = file.get("list")?.as_array()?;
        let list: Option<Vec<_>> = list.iter().map(peer).collect();
        let sources = match file.get("sources") {
            Some(sources) => sources.as_array()?.iter().map(uri).collect(),
            None => Some(Vec::new()),
        };
        Some((list?, sources?))
    });
    let (list, sources) =
        file.ok_or_else(|| schema("it needs a list of an ip and a port each".into()))?;
    Ok(PeersFile { list, sources })
}

/// Whether `text` is an IPv4 address (octets of at most three digits) with an optional prefix
/// length of 0 to 32, or an IPv6 address with one of 0 to 128.
fn is_ip(text: &str) -> bool {
    let (address, prefix) = text
        .split_once('/')
        .map_or((text, None), |(a, p)| (a, Some(p)));
    let number = |part: &str, most: usize, max: u16| {
        let digits = part.bytes().all(|b| b.is_ascii_digit());
        digits && part.len() <= most && part.parse::<u16>().is_ok_and(|n| n <= max)
    };
    let octets: Vec<&str> = address.split('.').collect();
    if octets.len() == 4 && octets.iter().all(|octet| number(octet, 3, 255)) {
        // `\d|[1-2]\d|3[0-2]`: no leading zero.
        return prefix.is_none_or(|p| number(p, 2, 32) && !(p.len() == 2 && p.starts_with('0')));
    }
    address.contains(':')
        && address.parse::<Ipv6Addr>().is_ok()
        && prefix.is_none_or(|p| number(p, 3, 128))
}

/// Whether `text` is a URI: a scheme, `:`, then only the characters a URI may hold, each `%`
/// followed by two hexadecimal digits.
fn is_uri(text: &str) -> bool {
    let (scheme, rest) = text.split_once(':').unwrap_or_default();
    let rest = rest.as_bytes();
    let hex = |at: usize| {
        rest.get(at + 1..at + 3)
            .is_some_and(|h| h.iter().all(u8::is_ascii_hexdigit))
    };
    let valid = |(at, b): (usize, &u8)| {
        b.is_ascii_alphanumeric() || b"-._~!$&'()*+,;=:@/?#[]".contains(b) || *b == b'%' && hex(at)
    };
    let scheme_valid = |b: u8| b.is_ascii_alphanumeric() || b"+-.".contains(&b);
    scheme.starts_with(|c: char| c.is_ascii_alphabetic())
        && scheme.bytes().all(scheme_valid)
        && rest.iter().enumerate().all(valid)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peers(text: &str) -> Result<PeersFile, PeersError> {
        parse(text)
    }

    #[test]
    fn the_reference_schema_takes_addresses_ports_and_uris() {
        let file = peers(
            r#"{"list":[{"ip":"192.0.2.1","port":"4002"},{"ip":"2001:db8::1","port":1e3},
               {"ip":"198.51.100.0/24","port":0},{"ip":"010.1.1.1","port":" 7 "}],
               "sources":["http://192.0.2.9/peers","file:///x","https://192.0.2.9/p","x:%41"]}"#,
        )
        .unwrap();
        let ports: Vec<u16> = file.list.iter().map(|peer| peer.port).collect();
        assert_eq!(ports, [4002, 1000, 0, 7]);
        assert_eq!(file.list[1].ip, "2001:db8::1");
        assert_eq!(file.sources.len(), 4);
        assert_eq!(peers(r#"{"list":[]}"#).unwrap(), PeersFile::default());
    }

    #[test]
    fn the_reference_schema_refuses_the_rest() {
        for text in [
            "",
            "[]",
            r#"{"sources":[]}"#,
            r#"{"list":[],"extra":1}"#,
            r#"{"list":null}"#,
            r#"{"list":[{"ip":"192.0.2.1"}]}"#,
            r#"{"list":[{"ip":"192.0.2.1","port":4002,"x":1}]}"#,
            r#"{"list":[{"ip":"192.0.2.1","port":65536}]}"#,
            r#"{"list":[{"ip":"192.0.2.1","port":"4002.5"}]}"#,
            r#"{"list":[{"ip":"192.0.2.1","port":"-0e0"}]}"#,
            r#"{"list":[{"ip":"192.0.2.256","port":1}]}"#,
            r#"{"list":[{"ip":"0192.0.2.1","port":1}]}"#,
            r#"{"list":[{"ip":"192.0.2.1/33","port":1}]}"#,
            r#"{"list":[{"ip":"192.0.2.1/08","port":1}]}"#,
            r#"{"list":[{"ip":"fe80::1%eth0","port":1}]}"#,
            r#"{"list":[{"ip":"2001:db8::1/129","port":1}]}"#,
            r#"{"list":[{"ip":"localhost","port":1}]}"#,
            r#"{"list":[],"sources":"http://192.0.2.9/"}"#,
            r#"{"list":[],"sources":["/peers.json"]}"#,
            r#"{"list":[],"sources":["http://192.0.2.9/a b"]}"#,
            r#"{"list":[],"sources":["http://192.0.2.9/%zz"]}"#,
            r#"{"list":[],"sources":[""]}"#,
        ] {
            assert!(
                matches!(peers(text), Err(PeersError::Schema { .. })),
                "{text}"
            );
        }
    }

    #[test]
    fn a_missing_file_is_named() {
        let error = load(Path::new("/nonexistent-heartwood-config")).unwrap_err();
        assert!(error.to_string().contains("peers.json"), "{error}");
    }
}
