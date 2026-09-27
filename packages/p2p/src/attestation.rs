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
use heartwood_p2p_wire::status::CheckedStatus;

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

/// What the loop reads of the pinged peer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attester {
    /// The peer's address.
    pub ip: IpAddr,
    /// Whether the peer's last full verification found it forked.
    pub forked: bool,
    /// The height of the peer's header.
    pub header_height: u32,
    /// The id of the peer's header, as the reply gave it.
    pub header_id: Option<String>,
}

/// What the loop reads of another peer of the repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Holder {
    /// The peer's address.
    pub ip: IpAddr,
    /// The delegate keys attributed to the peer.
    pub public_keys: Vec<PublicKeyBytes>,
    /// Whether the peer's last full verification found it forked.
    pub forked: bool,
    /// The height of the peer's header, when it has a state.
    pub header_height: Option<u32>,
    /// The id of the peer's header, as its reply gave it.
    pub header_id: Option<String>,
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

/// The attestation loop of `ping` over `status`, for `attester`, with the repository's other
/// peers `holders`.
pub fn attest(
    status: &CheckedStatus,
    attester: &Attester,
    holders: &[Holder],
    context: &AttestContext<'_>,
) -> Attestations {
    let _ = (status, attester, holders, context);
    todo!("the attestation loop")
}
