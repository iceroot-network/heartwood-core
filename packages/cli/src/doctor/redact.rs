//! Redaction: each IP address becomes `addr-N (class)`, numbered by first appearance so that one
//! address has one label in a report (an IPv4-mapped IPv6 address shares its IPv4 address's
//! label); under `--hide-identity` delegate names become `delegate-N` and public keys `pk-N`; the
//! home directory becomes `~`, the user's name `user` and the host's name `host`.
//!
//! Untrusted text is cleaned first: escape sequences, bidirectional marks and invisible format
//! characters are removed and every other control character becomes a space, so no excerpt can
//! move the cursor, recolour a terminal, reorder what is shown or start a line of its own. Then
//! hex runs of 64 characters or more become `[hex, N characters]`, the names and addresses are
//! replaced, and the excerpt is capped at [`MAX_EXCERPT_CHARS`] characters.

use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::path::{Component, Path};

use crate::doctor::report::{Fact, HostIdentity, Id, IdKind, Report};

/// The longest excerpt printed, in characters.
pub const MAX_EXCERPT_CHARS: usize = 160;
/// The most excerpts in one report or MCP response.
pub const MAX_EXCERPTS: usize = 20;
/// The shortest hex run replaced in untrusted text.
pub const MIN_HEX_RUN: usize = 64;

/// An address's class, which redaction keeps: `loopback`, `private`, `link-local`, `public` or
/// `unspecified`. An IPv4-mapped IPv6 address has its IPv4 address's class.
pub fn class(ip: IpAddr) -> &'static str {
    match ip.to_canonical() {
        ip if ip.is_unspecified() => "unspecified",
        ip if ip.is_loopback() => "loopback",
        IpAddr::V4(v4) if v4.is_private() => "private",
        IpAddr::V4(v4) if v4.is_link_local() => "link-local",
        IpAddr::V6(v6) if v6.is_unique_local() => "private",
        IpAddr::V6(v6) if v6.is_unicast_link_local() => "link-local",
        _ => "public",
    }
}

/// The redaction of one report: its address and identity labels and the names it hides.
#[derive(Debug)]
pub struct Redactor<'a> {
    identity: &'a HostIdentity,
    addresses: bool,
    hide_identity: bool,
    labels: BTreeMap<IpAddr, usize>,
    delegates: BTreeMap<String, usize>,
    keys: BTreeMap<String, usize>,
}

impl<'a> Redactor<'a> {
    /// The redactor of a report whose identity is `identity`, labelling addresses when
    /// `addresses` is set.
    pub fn new(identity: &'a HostIdentity, addresses: bool) -> Redactor<'a> {
        Redactor {
            identity,
            addresses,
            hide_identity: false,
            labels: BTreeMap::new(),
            delegates: BTreeMap::new(),
            keys: BTreeMap::new(),
        }
    }

    /// This redactor, labelling delegate names and public keys when `hide` is set.
    pub fn hide_identity(mut self, hide: bool) -> Redactor<'a> {
        self.hide_identity = hide;
        self
    }

    /// The redactor of `report`, with its identity redaction, every label numbered in the
    /// report's order (the findings' facts and advice values, then their untrusted text), so
    /// that its text and its JSON give each address and name the same label.
    pub fn for_report(report: &'a Report, addresses: bool) -> Redactor<'a> {
        let mut redactor =
            Redactor::new(&report.identity, addresses).hide_identity(report.redaction.identity);
        for finding in &report.findings {
            let values = finding.facts.iter().chain(&finding.advice.values);
            for (_, fact) in values {
                match fact {
                    Fact::Address(ip) => {
                        let _ = redactor.address(*ip);
                    }
                    Fact::Id(id) => {
                        let _ = redactor.id(id);
                    }
                    _ => {}
                }
            }
        }
        for excerpt in report.findings.iter().flat_map(|f| &f.untrusted) {
            let _ = redactor.untrusted(&excerpt.text);
        }
        redactor
    }

    /// `ip` as printed: its label and class, or itself when addresses are shown.
    pub fn address(&mut self, ip: IpAddr) -> String {
        if !self.addresses {
            return ip.to_string();
        }
        let next = self.labels.len().saturating_add(1);
        let label = *self.labels.entry(ip.to_canonical()).or_insert(next);
        format!("addr-{label} ({})", class(ip))
    }

    /// `id` as printed: a delegate name as `delegate-N` and a public key as `pk-N` when the
    /// identity is hidden, anything else as itself.
    pub fn id(&mut self, id: &Id) -> String {
        let (map, word) = match id.kind() {
            IdKind::Username if self.hide_identity => (&mut self.delegates, "delegate"),
            IdKind::PublicKey if self.hide_identity => (&mut self.keys, "pk"),
            _ => return id.as_str().to_owned(),
        };
        let next = map.len().saturating_add(1);
        let label = *map.entry(id.as_str().to_owned()).or_insert(next);
        format!("{word}-{label}")
    }

    /// `path` as printed: the home directory as `~`, a component equal to the user's or the
    /// host's name replaced, control characters as spaces.
    pub fn path(&self, path: &Path) -> String {
        let home = self.identity.home.as_deref();
        let (mut text, rest) = match home.and_then(|home| path.strip_prefix(home).ok()) {
            Some(rest) if home.is_some_and(|home| home.components().count() > 1) => {
                ("~".to_owned(), rest)
            }
            _ => (String::new(), path),
        };
        for component in rest.components() {
            let part = component.as_os_str().to_string_lossy();
            let part = self.names(&part);
            if !(text.is_empty() || text.ends_with('/')) {
                text.push('/');
            }
            text.push_str(if component == Component::RootDir {
                "/"
            } else {
                &part
            });
        }
        clean(&text)
    }

    /// Untrusted `text`, cleaned, redacted and capped at [`MAX_EXCERPT_CHARS`] characters.
    pub fn untrusted(&mut self, text: &str) -> String {
        let mut text = hex_runs(&clean(text));
        let home = self.identity.home.as_deref().and_then(Path::to_str);
        if let Some(home) = home.filter(|home| home.len() > 1) {
            text = replace_word(&text, home.trim_end_matches('/'), "~");
        }
        text = self.names(&text);
        if self.hide_identity {
            for (name, label) in &self.delegates {
                text = replace_word(&text, name, &format!("delegate-{label}"));
            }
        }
        if self.addresses {
            text = self.v6(&text);
            text = self.v4(&text);
        }
        text.chars().take(MAX_EXCERPT_CHARS).collect()
    }

    /// `text` with the user's name, the host's name and its first label replaced.
    fn names(&self, text: &str) -> String {
        let host = self.identity.host_name.as_deref();
        let short = host
            .and_then(|host| host.split_once('.'))
            .map(|(short, _)| short);
        let names = [
            (self.identity.user.as_deref(), "user"),
            (host, "host"),
            (short, "host"),
        ];
        let mut text = text.to_owned();
        for (name, word) in names {
            if let Some(name) = name.filter(|name| !name.is_empty()) {
                text = replace_word(&text, name, word);
            }
        }
        text
    }

    /// `text` with every IPv6 address (compressed, IPv4-mapped, or with a zone) labelled.
    fn v6(&mut self, text: &str) -> String {
        let class = |c: char| c.is_ascii_hexdigit() || c == ':' || c == '.';
        map_runs(text, class, |run, before, after| {
            let Some((from, to, ip)) = v6_in(run, before, after.chars().next()) else {
                return (run.to_owned(), 0);
            };
            let (head, tail) = (run.get(..from), run.get(to..));
            let label = self.address(IpAddr::V6(ip));
            let zone = |c: char| !(c.is_ascii_alphanumeric() || matches!(c, '%' | '_' | '-' | '.'));
            let skip = match to == run.len() && after.starts_with('%') {
                true => after.find(zone).unwrap_or(after.len()),
                false => 0,
            };
            let (head, tail) = (head.unwrap_or_default(), tail.unwrap_or_default());
            (format!("{head}{label}{tail}"), skip)
        })
    }

    /// `text` with every dotted IPv4 address labelled.
    fn v4(&mut self, text: &str) -> String {
        map_runs(
            text,
            |c| c.is_ascii_digit() || c == '.',
            |run, _, _| {
                let trimmed = run.trim_matches('.');
                match trimmed.parse::<Ipv4Addr>() {
                    Ok(ip) => (run.replacen(trimmed, &self.address(IpAddr::V4(ip)), 1), 0),
                    Err(_) => (run.to_owned(), 0),
                }
            },
        )
    }
}

/// `text` with each longest run of `class` characters replaced by what `replace` gives for it,
/// the character before it and the text after it; `replace` also says how many bytes of the
/// text after it to drop.
fn map_runs(
    text: &str,
    class: impl Fn(char) -> bool,
    mut replace: impl FnMut(&str, Option<char>, &str) -> (String, usize),
) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find(&class) {
        let (before, tail) = rest.split_at(start);
        let (run, after) = tail.split_at(tail.find(|c| !class(c)).unwrap_or(tail.len()));
        out.push_str(before);
        let (with, skip) = replace(run, before.chars().next_back(), after);
        out.push_str(&with);
        rest = after.get(skip..).unwrap_or_default();
    }
    out.push_str(rest);
    out
}

/// The longest text of an IPv6 address (an IPv4-mapped one with a zone-free tail).
const MAX_V6_TEXT: usize = 45;

/// The IPv6 address in `run` (hex digits, colons and dots, between `before` and `after`), with
/// its start and end: the leftmost, then longest, piece of `run` that starts at its start or
/// after a dot or colon, ends at its end or before one, and parses. A piece of one or two groups
/// glued to a letter, a digit or `_` is not an address (`std::io`, `Vec::new`).
fn v6_in(run: &str, before: Option<char>, after: Option<char>) -> Option<(usize, usize, Ipv6Addr)> {
    if run.matches(':').count() < 2 {
        return None;
    }
    let glued = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
    let cuts: Vec<usize> = run.match_indices(['.', ':']).map(|(at, _)| at).collect();
    let starts = std::iter::once(0).chain(cuts.iter().map(|at| at.saturating_add(1)));
    for from in starts.take(9) {
        let ends = std::iter::once(run.len()).chain(cuts.iter().rev().copied());
        for to in ends.filter(|to| *to > from && to.saturating_sub(from) <= MAX_V6_TEXT) {
            let candidate = run.get(from..to).unwrap_or_default();
            let before = match from {
                0 => before,
                _ => run.get(..from).and_then(|r| r.chars().next_back()),
            };
            let after = run.get(to..).and_then(|r| r.chars().next()).or(after);
            let groups = candidate.split(':').filter(|g| !g.is_empty()).count();
            if candidate == "::" || (groups < 3 && (glued(before) || glued(after))) {
                continue;
            }
            if let Ok(ip) = candidate.parse::<Ipv6Addr>() {
                return Some((from, to, ip));
            }
        }
    }
    None
}

/// `text` without escape sequences, bidirectional marks and invisible format characters, with
/// every other control character (line breaks and tabs included) as a space.
pub fn clean(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match c {
            '\u{1b}' => match chars.next() {
                Some('[') => csi(&mut chars),
                Some(']' | 'P' | 'X' | '^' | '_') => {
                    let mut last = ' ';
                    for c in chars.by_ref() {
                        if c == '\u{7}' || c == '\u{9c}' || (last == '\u{1b}' && c == '\\') {
                            break;
                        }
                        last = c;
                    }
                }
                _ => {}
            },
            '\u{9b}' => csi(&mut chars),
            '\u{61c}' | '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' => {}
            '\u{2060}'..='\u{2069}' | '\u{feff}' => {}
            c if c.is_control() || matches!(c, '\u{2028}' | '\u{2029}') => out.push(' '),
            c => out.push(c),
        }
    }
    out
}

/// Skip a control sequence's parameters and its final character.
fn csi(chars: &mut std::str::Chars<'_>) {
    for c in chars.by_ref() {
        if !('\u{20}'..='\u{3f}').contains(&c) {
            break;
        }
    }
}

/// `text` with every hex run of [`MIN_HEX_RUN`] characters or more as `[hex, N characters]`.
fn hex_runs(text: &str) -> String {
    map_runs(
        text,
        |c| c.is_ascii_hexdigit(),
        |run, _, _| match run.len() >= MIN_HEX_RUN {
            true => (format!("[hex, {} characters]", run.len()), 0),
            false => (run.to_owned(), 0),
        },
    )
}

/// `text` with each occurrence of `needle` (ASCII case ignored) replaced by `with`, where it
/// is not part of a longer word: a letter, digit, `_` or `-` next to an end of `needle` that is
/// itself such a character ends the match.
fn replace_word(text: &str, needle: &str, with: &str) -> String {
    let word = |c: char| c.is_alphanumeric() || c == '_' || c == '-';
    let (first, last) = (needle.chars().next(), needle.chars().next_back());
    let (Some(first), Some(last)) = (first, last) else {
        return text.to_owned();
    };
    let lower = text.to_ascii_lowercase();
    let needle_lower = needle.to_ascii_lowercase();
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    while let Some(found) = lower.get(at..).and_then(|rest| rest.find(&needle_lower)) {
        let start = at.saturating_add(found);
        let end = start.saturating_add(needle.len());
        let before = text.get(..start).and_then(|t| t.chars().next_back());
        let after = text.get(end..).and_then(|t| t.chars().next());
        let bounded =
            !(word(first) && before.is_some_and(word)) && !(word(last) && after.is_some_and(word));
        out.push_str(text.get(at..start).unwrap_or_default());
        out.push_str(if bounded {
            with
        } else {
            text.get(start..end).unwrap_or_default()
        });
        at = end;
    }
    out.push_str(text.get(at..).unwrap_or_default());
    out
}
