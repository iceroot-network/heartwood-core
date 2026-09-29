//! The relay's peers: how many, how high they are, and how varied their networks are.

use std::collections::BTreeMap;
use std::net::IpAddr;

use crate::doctor::checks::process::{Res, ints, relay};
use crate::doctor::checks::sync::{ROUND, synced_once};
use crate::doctor::registry::CheckInput;
use crate::doctor::report::Basis::Chain;
use crate::doctor::report::Finding;
use crate::doctor::report::Status::{Fail, Pass, Skipped, Warn};
use crate::doctor::sources::api::ApiPeers;
use crate::doctor::view::{Reason, Source, Unknown, View};

/// How many blocks off the peers' median a relay may be and still pass.
const TOLERANCE: i64 = 2;
/// The fewest public peers the diversity rule judges.
const DIVERSITY_MIN: usize = 4;

/// `peers.count`.
pub fn count(input: &CheckInput<'_>) -> Vec<Finding> {
    relay(input, |view| {
        let peers = view.api_peers()?.total_count;
        let wanted = if peers == 0 {
            0
        } else {
            view.relay_config()?.minimum_network_reach
        };
        let verdict = match peers {
            0 => (Fail, "none"),
            n if n < wanted => (Warn, "few"),
            _ => (Pass, "enough"),
        };
        Ok((Chain, verdict, ints!(peers, wanted)))
    })
}

/// The peers' median height (the higher middle one for an even count); unknown when no peer
/// reported a height.
pub(crate) fn median_height(peers: &ApiPeers) -> Result<u32, Unknown> {
    let mut heights: Vec<u32> = peers.peers.iter().filter_map(|peer| peer.height).collect();
    heights.sort_unstable();
    let middle = heights.get(heights.len() / 2).copied();
    middle.ok_or(Unknown::new(Source::Api, Reason::Missing))
}

/// The blocks a relay is behind the peers' median (negative when ahead), and its height.
pub(crate) fn behind(view: &dyn View) -> Result<(i64, u32), Unknown> {
    let median = median_height(view.api_peers()?)?;
    let height = view.api_status()?.now;
    Ok((i64::from(median) - i64::from(height), height))
}

/// `peers.height`.
pub fn height(input: &CheckInput<'_>) -> Vec<Finding> {
    relay(input, |view| {
        let (behind, height) = behind(view)?;
        let blocks = behind.abs();
        let verdict = match behind {
            _ if blocks <= TOLERANCE => (Pass, "close"),
            ..0 => (Warn, "ahead"),
            _ if behind < ROUND => (Warn, "behind"),
            // A relay that has not finished its first sync is expected to be far behind; when the
            // log cannot say, the check warns rather than fails.
            _ => match synced_once(view) {
                Ok(true) => (Fail, "far-behind"),
                Ok(false) => (Warn, "first-sync"),
                Err(_) => (Warn, "behind"),
            },
        };
        Ok((Chain, verdict, ints!(blocks, height)))
    })
}

/// The /24 (IPv4) or /48 (IPv6) of a public address; `None` for a loopback, private, link-local,
/// shared or unspecified one.
fn public_group(ip: IpAddr) -> Option<u128> {
    match ip.to_canonical() {
        IpAddr::V4(v4) => {
            let [first, second, ..] = v4.octets();
            let local = v4.is_loopback() || v4.is_private() || v4.is_link_local();
            let local = local || v4.is_unspecified() || (first == 100 && second & 0xc0 == 0x40);
            (!local).then(|| u128::from(u32::from(v4) >> 8))
        }
        IpAddr::V6(v6) => {
            let [first, ..] = v6.segments();
            let local = v6.is_loopback() || v6.is_unspecified();
            let local = local || first & 0xfe00 == 0xfc00 || first & 0xffc0 == 0xfe80;
            (!local).then(|| v6.to_bits() >> 80 | 1 << 100)
        }
    }
}

/// `peers.diversity`: the peers are grouped by their raw addresses; redaction comes after.
pub fn diversity(input: &CheckInput<'_>) -> Vec<Finding> {
    relay(input, |view| -> Res {
        let peers = &view.api_peers()?.peers;
        let mut groups: BTreeMap<u128, usize> = BTreeMap::new();
        for group in peers.iter().filter_map(|peer| public_group(peer.ip)) {
            *groups.entry(group).or_default() += 1;
        }
        let (public, total) = (groups.values().sum::<usize>(), peers.len());
        let largest = groups.values().copied().max().unwrap_or(0);
        let verdict = match public {
            n if n < DIVERSITY_MIN => (Skipped, "skipped-few"),
            _ if largest * 2 > public => (Warn, "shared"),
            _ => (Pass, "spread"),
        };
        Ok((Chain, verdict, ints!(public, total, largest)))
    })
}
