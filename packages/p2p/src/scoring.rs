//! The client-side protocol violations: faults that no honest reference peer produces, recorded
//! in the peer's violation window ([`crate::peer::Peer::add_violation`]).
//!
//! A violation counts toward the reference's ignore rule with the infractions, and while a peer
//! has one in its window it is no source of blocks, transactions or peers and is not accepted
//! again. The reference's own infractions (a cleanse ping still running at its deadline) keep
//! exactly their reference effect.

use crate::peer::SharedPeer;

/// A protocol violation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Violation {
    /// A reply that the route's codec refused.
    ReplyDecode,
    /// A reply that failed the route's reply schema.
    ReplySchema,
    /// A nes protocol error: a response to an unknown id, a text frame, a WebSocket control frame,
    /// a ping flood.
    Protocol,
    /// A getCommonBlocks reply naming a block we did not ask for, or at another height.
    BogusCommonBlock,
    /// A fetched fork header that fails its signature, height, delegate or linkage check.
    ForkHeader,
    /// A downloaded block that fails decoding, or a verification rule other than the timestamp.
    DownloadedBlock,
}

/// Record `violation` against `peer` at `now_ms`.
pub fn record(peer: &SharedPeer, violation: Violation, now_ms: i64) {
    let _ = (peer, violation, now_ms);
    todo!("record the violation")
}
