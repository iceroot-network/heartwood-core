//! The reference's address rules (`isValidPeer`), with ipaddr.js's parsing and range
//! classification.
//!
//! ipaddr.js reads IPv6 first: at most one `::`, an optional zone (`%` and letters or digits) and
//! an optional dotted IPv4 tail, eight groups of at most 16 bits; `::a.b.c.d` is read as
//! `::ffff:a.b.c.d`. Otherwise IPv4 in one to four dotted parts, each decimal, octal (a leading
//! `0`) or hexadecimal (`0x`), the last part filling the remaining bytes. Processing turns an
//! IPv4-mapped address into IPv4. The text of an IPv6 address writes the groups in lowercase hex
//! and replaces the first run of zero groups, even a single one, with `::` (the last group only
//! when there is no zone).

use std::net::{IpAddr, Ipv4Addr};

use heartwood_kernel::ip;

/// An address as ipaddr.js holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Address {
    V4([u8; 4]),
    /// The eight groups and the zone (empty when there is none).
    V6([u16; 8], String),
}

/// `ipaddr.process(ip).toString()`: the address in ipaddr.js's text form, an IPv4-mapped IPv6
/// address as IPv4; `None` when ipaddr.js cannot parse `ip`.
pub fn process(ip: &str) -> Option<String> {
    parse(ip).map(|address| text(&unmapped(address)))
}

/// `isValidPeer`: `ip` becomes its processed form when it parses, and is valid when its range is
/// `unicast` (or `private`, `linkLocal` or `uniqueLocal` with `allow_private`), its text does not
/// start with `0`, and it is none of `interfaces`, the host's own addresses, compared as text.
pub fn is_valid_peer(ip: &mut String, allow_private: bool, interfaces: &[IpAddr]) -> bool {
    let Some(address) = parse(ip).map(unmapped) else {
        return false;
    };
    *ip = text(&address);
    let range = range(&address);
    let valid_range = range == "unicast"
        || (allow_private && matches!(range, "private" | "linkLocal" | "uniqueLocal"));
    valid_range && !ip.starts_with('0') && !interfaces.iter().any(|own| interface_text(*own) == *ip)
}

/// `ipaddr.parse`: IPv6 when the text has a `:` and reads as eight groups of 16 bits, else IPv4
/// in one to four parts, the last filling the bytes the others leave.
fn parse(text: &str) -> Option<Address> {
    if text.contains(':') {
        let groups = ip::ipv6_parts(text)?
            .into_iter()
            .map(|group| u16::try_from(group).ok());
        let zone = text.split_once('%').map_or("", |(_, zone)| zone);
        return Some(Address::V6(
            groups.collect::<Option<Vec<u16>>>()?.try_into().ok()?,
            zone.to_owned(),
        ));
    }
    let values = text
        .split('.')
        .map(ip::ipv4_part)
        .collect::<Option<Vec<u64>>>()?;
    let (last, fixed) = values.split_last()?;
    let last = u32::try_from(*last).ok()?.to_be_bytes();
    let mut octets: Vec<u8> = fixed
        .iter()
        .map(|v| u8::try_from(*v).ok())
        .collect::<Option<_>>()?;
    let filled = octets.len();
    if filled > 3 || last.iter().take(filled).any(|byte| *byte != 0) {
        return None;
    }
    octets.extend(last.iter().skip(filled));
    Some(Address::V4(octets.try_into().ok()?))
}

/// `process`: an IPv4-mapped IPv6 address as IPv4, its zone dropped.
fn unmapped(address: Address) -> Address {
    match address {
        Address::V6([0, 0, 0, 0, 0, 0xffff, high, low], _) => {
            let ([a, b], [c, d]) = (high.to_be_bytes(), low.to_be_bytes());
            Address::V4([a, b, c, d])
        }
        other => other,
    }
}

/// ipaddr.js's `toString`.
fn text(address: &Address) -> String {
    match address {
        Address::V4(octets) => Ipv4Addr::from(*octets).to_string(),
        Address::V6(groups, zone) => {
            let zero = |i: usize| groups.get(i) == Some(&0) && (i < 7 || zone.is_empty());
            let hex = |from: usize, to: usize| {
                let written: Vec<String> = (groups.get(from..to).unwrap_or_default().iter())
                    .map(|group| format!("{group:x}"))
                    .collect();
                written.join(":")
            };
            let body = match (0..8).find(|i| zero(*i)) {
                Some(start) => {
                    let end = (start..8).find(|i| !zero(*i)).unwrap_or(8);
                    format!("{}::{}", hex(0, start), hex(end, 8))
                }
                None => hex(0, 8),
            };
            if zone.is_empty() {
                body
            } else {
                format!("{body}%{zone}")
            }
        }
    }
}

/// The range ipaddr.js names for `address`: the first of its special ranges that holds it, in its
/// order, or `unicast`.
fn range(address: &Address) -> &'static str {
    match address {
        Address::V4(octets) => match *octets {
            [0, ..] => "unspecified",
            [255, 255, 255, 255] => "broadcast",
            [224..=239, ..] => "multicast",
            [169, 254, ..] => "linkLocal",
            [127, ..] => "loopback",
            [100, 64..=127, ..] => "carrierGradeNat",
            [10, ..] | [172, 16..=31, ..] | [192, 168, ..] => "private",
            [192, 0, 0 | 2, _] | [192, 88, 99, _] | [198, 51, 100, _] | [203, 0, 113, _] => {
                "reserved"
            }
            [240..=255, ..] => "reserved",
            _ => "unicast",
        },
        Address::V6(groups, _) => match *groups {
            [0, 0, 0, 0, 0, 0, 0, 0] => "unspecified",
            [0xfe80..=0xfebf, ..] => "linkLocal",
            [0xff00..=0xffff, ..] => "multicast",
            [0, 0, 0, 0, 0, 0, 0, 1] => "loopback",
            [0xfc00..=0xfdff, ..] => "uniqueLocal",
            [0, 0, 0, 0, 0, 0xffff, _, _] => "ipv4Mapped",
            [0, 0, 0, 0, 0xffff, 0, _, _] => "rfc6145",
            [0x64, 0xff9b, 0, 0, 0, 0, _, _] => "rfc6052",
            [0x2002, ..] => "6to4",
            [0x2001, 0, ..] => "teredo",
            [0x2001, 0xdb8, ..] => "reserved",
            _ => "unicast",
        },
    }
}

/// The text Node gives one of the host's addresses: dotted IPv4; IPv6 in lowercase hex with the
/// first longest run of at least two zero groups as `::`, and an IPv4-compatible or IPv4-mapped
/// address with a dotted tail.
fn interface_text(address: IpAddr) -> String {
    match address {
        IpAddr::V6(v6) => match v6.octets() {
            [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, a, b, c, d] if [a, b] != [0, 0] => {
                format!("::{}", Ipv4Addr::new(a, b, c, d))
            }
            _ => v6.to_string(),
        },
        IpAddr::V4(v4) => v4.to_string(),
    }
}

#[cfg(test)]
mod tests;
