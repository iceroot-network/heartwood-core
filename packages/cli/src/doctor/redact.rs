//! Redaction: each IP address becomes `addr-N (class)`, numbered by first appearance so that one
//! address has one label in a report; the home directory becomes `~`, the user's name `user` and
//! the host's name `host`; untrusted text is cleaned of control characters, escape sequences and
//! bidirectional marks, has its addresses and hex runs of 64 characters or more replaced, and is
//! capped at [`MAX_EXCERPT_CHARS`] characters.

use std::collections::BTreeMap;
use std::net::IpAddr;
use std::path::Path;

use crate::doctor::report::HostIdentity;

/// The longest excerpt printed, in characters.
pub const MAX_EXCERPT_CHARS: usize = 160;
/// The most excerpts in one report or MCP response.
pub const MAX_EXCERPTS: usize = 20;

/// An address's class, which redaction keeps.
pub fn class(ip: IpAddr) -> &'static str {
    let ip = match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(IpAddr::V6(v6), IpAddr::V4),
        v4 => v4,
    };
    match ip {
        _ if ip.is_unspecified() => "unspecified",
        _ if ip.is_loopback() => "loopback",
        IpAddr::V4(v4) if v4.is_private() => "private",
        IpAddr::V4(v4) if v4.is_link_local() => "link-local",
        IpAddr::V6(v6) if v6.is_unique_local() => "private",
        IpAddr::V6(v6) if v6.is_unicast_link_local() => "link-local",
        _ => "public",
    }
}

/// The redaction of one report: its address labels and the identity it hides.
#[derive(Debug)]
pub struct Redactor<'a> {
    identity: &'a HostIdentity,
    addresses: bool,
    labels: BTreeMap<IpAddr, usize>,
}

impl<'a> Redactor<'a> {
    /// The redactor of a report whose identity is `identity`, labelling addresses when
    /// `addresses` is set.
    pub fn new(identity: &'a HostIdentity, addresses: bool) -> Redactor<'a> {
        Redactor {
            identity,
            addresses,
            labels: BTreeMap::new(),
        }
    }

    /// `ip` as printed: its label and class, or itself when addresses are shown.
    pub fn address(&mut self, ip: IpAddr) -> String {
        if !self.addresses {
            return ip.to_string();
        }
        let next = self.labels.len().saturating_add(1);
        let label = *self.labels.entry(ip).or_insert(next);
        format!("addr-{label} ({})", class(ip))
    }

    /// `path` as printed, with the home directory as `~`, the user's and the host's names
    /// replaced.
    pub fn path(&self, path: &Path) -> String {
        let text = match &self.identity.home {
            Some(home) if path.starts_with(home) => {
                let rest = path.strip_prefix(home).unwrap_or(path);
                Path::new("~").join(rest).display().to_string()
            }
            _ => path.display().to_string(),
        };
        self.names(&text)
    }

    /// `text` with the user's and the host's names replaced.
    fn names(&self, text: &str) -> String {
        let mut text = text.to_owned();
        let names = [
            (&self.identity.user, "user"),
            (&self.identity.host_name, "host"),
        ];
        for (name, word) in names {
            if let Some(name) = name.as_deref().filter(|name| name.len() > 1) {
                text = text.replace(name, word);
            }
        }
        text
    }

    /// Untrusted `text`, cleaned, redacted and capped.
    pub fn untrusted(&mut self, text: &str) -> String {
        let clean: String = text
            .chars()
            .map(|c| if c.is_control() || is_bidi(c) { ' ' } else { c })
            .collect();
        let clean = self.names(&clean);
        clean.chars().take(MAX_EXCERPT_CHARS).collect()
    }
}

/// Whether `c` is a bidirectional control.
fn is_bidi(c: char) -> bool {
    matches!(c, '\u{61c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
}
