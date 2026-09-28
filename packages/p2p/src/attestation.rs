//! The status attestations of a ping: which delegate keys a peer's status proves online, which
//! peer they are attributed to, and the versions recorded for them (the online record).
//!
//! The loop runs over the reply's `(publicKey, signature)` pairs in order, against the verifying
//! preimage of the checked status; it does nothing when the reply's slot is 0, and skips a pair it
//! has already checked. When the reply's slot is within one slot of ours, an unknown key or a
//! failed signature ends the loop (reproduced). For a key of a registered delegate, with the round
//! of the peer's header height: a key no other peer holds is attributed to this peer and its
//! version recorded; a key another peer holds moves to this peer only when this peer is not forked
//! and either the other is forked, or this peer's header is our last block, or this peer is at
//! least as high and the other's header is not our last block.

use std::net::IpAddr;

use heartwood_crypto::crypto::slots::{self, SlotInfo};
use heartwood_crypto::{Milestones, PublicKeyBytes};
use heartwood_kernel::calculate_round;
use heartwood_p2p_wire::status::{CheckedStatus, verify_attestation, verifying_preimage};

use crate::peer::Peer;

/// The slot at `now_ms` (milliseconds since the Unix epoch) under `milestones`, whose block time
/// and epoch are fixed from height 1; `None` when the epoch does not parse, which the milestone
/// loader never allows.
pub fn current_slot(milestones: &Milestones, now_ms: i64) -> Option<SlotInfo> {
    let first = milestones.at(1);
    let epoch_ms = slots::epoch_unix_ms(first.epoch())?;
    Some(slots::slot_info(
        slots::epoch_time(now_ms, epoch_ms),
        first.block_time(),
    ))
}

/// What the loop reads of a peer: the pinged one, whose state the ping has just set, or another
/// peer of the repository, which may hold a key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Holder {
    /// The peer's address.
    pub ip: IpAddr,
    /// The delegate keys attributed to the peer.
    pub public_keys: Vec<PublicKeyBytes>,
    /// Whether the peer's last full verification found it forked.
    pub forked: bool,
    /// The height of the peer's header, when it has a state (the pinged peer always has one).
    pub header_height: Option<u32>,
    /// The id of the peer's header, as its reply gave it.
    pub header_id: Option<String>,
}

impl Holder {
    /// What the loop reads of `peer`.
    pub fn of(peer: &Peer) -> Holder {
        let header = peer.state.as_ref().map(|state| &state.header);
        Holder {
            ip: peer.ip,
            public_keys: peer.public_keys.clone(),
            forked: peer.is_forked(),
            header_height: header.and_then(|header| header.height),
            header_id: header.and_then(|header| header.id.clone()),
        }
    }
}

/// What the loop reads of the node.
#[derive(Debug, Clone, Copy)]
pub struct AttestContext<'a> {
    /// Our current slot.
    pub slot: &'a SlotInfo,
    /// Our last block's height.
    pub last_height: u32,
    /// Our last block's id, as lowercase hex.
    pub last_id: &'a str,
    /// The public keys of every registered delegate (`allByUsername`).
    pub delegate_keys: &'a [PublicKeyBytes],
    /// The milestones, for the round of the peer's header height.
    pub milestones: &'a Milestones,
}

/// A version the loop records in the online record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionRecord {
    /// The delegate's key.
    pub key: PublicKeyBytes,
    /// The round of the peer's header height.
    pub round: u64,
    /// The version the status reports.
    pub version: String,
}

/// What the loop decided.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Attestations {
    /// The keys attributed to the pinged peer, in order.
    pub attributed: Vec<PublicKeyBytes>,
    /// The versions to record, in order.
    pub versions: Vec<VersionRecord>,
    /// The keys taken from other peers: the other peer's address and the key.
    pub moved: Vec<(IpAddr, PublicKeyBytes)>,
}

/// The attestation loop of `ping` over `status`, for the `pinged` peer, with the repository's
/// other peers `holders` in the repository's order.
pub fn attest(
    status: &CheckedStatus,
    pinged: &Holder,
    holders: &[Holder],
    context: &AttestContext<'_>,
) -> Attestations {
    let mut decided = Attestations::default();
    let Some(current_slot) = reply_slot(status, context.slot) else {
        return decided;
    };
    // The window does not change during the loop, so outside it no pair is looked at.
    let ours = context.slot.slot_number;
    if current_slot < ours.saturating_sub(1) || current_slot > ours.saturating_add(1) {
        return decided;
    }
    let preimage = verifying_preimage(status, ours < 0);
    let delegates: Vec<String> = context.delegate_keys.iter().map(|k| k.to_hex()).collect();
    let height = pinged.header_height.unwrap_or(0);
    let round = calculate_round(height, context.milestones).round;
    let version = status
        .message
        .config
        .as_ref()
        .and_then(|config| config.version.clone())
        .unwrap_or_default();
    let mut holders: Vec<Holder> = holders
        .iter()
        .filter(|holder| holder.ip != pinged.ip)
        .cloned()
        .collect();
    let ours_is_last =
        height == context.last_height && pinged.header_id.as_deref() == Some(context.last_id);
    // `publicKey + signature`, as the reference concatenates them; a missing key reads
    // "undefined", which no pair that gets past the delegate test has.
    let mut checked: Vec<String> = Vec::new();
    for (index, signature) in status.message.signatures.iter().enumerate() {
        let key = status.message.public_keys.get(index);
        let pair = format!("{}{signature}", key.map_or("undefined", String::as_str));
        if checked.contains(&pair) {
            continue;
        }
        // An unknown key or a failed signature ends the loop (reproduced).
        let Some(key) = key.filter(|key| delegates.contains(key)) else {
            break;
        };
        if !verify_attestation(&preimage, key, signature) {
            break;
        }
        let Ok(key) = PublicKeyBytes::from_hex(key) else {
            break;
        };
        let other = holders
            .iter()
            .position(|holder| holder.public_keys.contains(&key));
        let taken = match other.and_then(|index| holders.get_mut(index)) {
            None => true,
            Some(holder) => {
                let moves = !pinged.forked
                    && (holder.forked
                        || ours_is_last
                        || (holder.header_height.is_some_and(|theirs| height >= theirs)
                            && holder.header_id.as_deref() != Some(context.last_id)));
                if moves {
                    holder.public_keys.retain(|held| *held != key);
                    decided.moved.push((holder.ip, key));
                }
                moves
            }
        };
        if taken {
            decided.attributed.push(key);
            decided.versions.push(VersionRecord {
                key,
                round,
                version: version.clone(),
            });
        }
        checked.push(pair);
    }
    decided
}

/// The reply's slot as the loop reads it: `None` for 0, which ends nothing but checks nothing;
/// read as a signed 32-bit value (`currentSlot >> 0`) when our own slot is negative.
fn reply_slot(status: &CheckedStatus, ours: &SlotInfo) -> Option<i64> {
    let slot = status.message.state.as_ref()?.current_slot?;
    if slot == 0 {
        return None;
    }
    Some(if ours.slot_number < 0 {
        i64::from(i32::from_ne_bytes(slot.to_ne_bytes()))
    } else {
        i64::from(slot)
    })
}
