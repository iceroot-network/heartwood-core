//! The client-side protocol violations: faults that no honest reference peer produces, recorded
//! in the peer's violation window ([`crate::peer::Peer::add_violation`]).
//!
//! A violation counts toward the reference's ignore rule with the infractions, and while a peer
//! has one in its window it is no source of blocks, transactions or peers and is not accepted
//! again. The reference's own infractions (a cleanse ping still running at its deadline) keep
//! exactly their reference effect.
//!
//! The violations of a peer that is forgotten are kept by the repository for the rest of their
//! window ([`crate::repository::PeerRepository::has_violation`]), so a violating peer is not
//! accepted again as soon as discovery names it.

use heartwood_p2p_wire::errors::DecodeError;

use crate::errors::{ProtocolError, SessionError};
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

impl Violation {
    /// The violation a failed request shows, if any: every reply the codec refuses, every nes
    /// protocol error but a heartbeat timeout (a peer that has gone quiet, which an honest peer
    /// may), a hello in answer to a request, and nothing for the other session errors (timeouts,
    /// disconnects, server errors).
    pub fn of_session(error: &SessionError) -> Option<Violation> {
        match error {
            SessionError::Protocol(ProtocolError::HeartbeatTimeout) => None,
            SessionError::Protocol(_) | SessionError::NoPayload => Some(Violation::Protocol),
            _ => None,
        }
    }

    /// The violation of a reply the codec refused: an amount beyond 128 bits is a schema
    /// violation (the reference refuses it at its reply schema), every other refusal a decode
    /// violation.
    pub fn of_decode(error: DecodeError) -> Violation {
        match error {
            DecodeError::AmountTooWide => Violation::ReplySchema,
            _ => Violation::ReplyDecode,
        }
    }
}

/// Record `violation` against `peer` at `now_ms`.
pub fn record(peer: &SharedPeer, violation: Violation, now_ms: i64) {
    peer.with(|peer| peer.add_violation(now_ms));
    heartwood_kernel::debug!(
        "Protocol violation by peer {}: {violation:?} :warning:",
        peer.ip()
    );
}

#[cfg(test)]
mod tests {
    use std::net::IpAddr;

    use super::*;
    use crate::peer::Peer;

    #[test]
    fn a_heartbeat_timeout_is_no_violation_and_other_protocol_errors_are() {
        let of = |error| Violation::of_session(&SessionError::Protocol(error));
        assert_eq!(of(ProtocolError::HeartbeatTimeout), None);
        for error in [
            ProtocolError::UnknownRequest,
            ProtocolError::TextFrame,
            ProtocolError::UnknownType,
            ProtocolError::PingFlood,
            ProtocolError::ControlFrame,
        ] {
            assert_eq!(of(error), Some(Violation::Protocol));
        }
        assert_eq!(
            Violation::of_session(&SessionError::NoPayload),
            Some(Violation::Protocol)
        );
        for error in [
            SessionError::Timeout,
            SessionError::Disconnect,
            SessionError::Ws("closed".into()),
            SessionError::Server {
                status: 500,
                payload: String::new(),
            },
        ] {
            assert_eq!(Violation::of_session(&error), None);
        }
        assert_eq!(
            Violation::of_decode(DecodeError::AmountTooWide),
            Violation::ReplySchema
        );
        assert_eq!(
            Violation::of_decode(DecodeError::IndexOutOfRange),
            Violation::ReplyDecode
        );
    }

    #[test]
    fn a_record_lands_in_the_peers_window() {
        let peer = SharedPeer::new(Peer::new(IpAddr::from([192, 0, 2, 1]), 4002));
        record(&peer, Violation::ReplySchema, 5_000);
        assert_eq!(peer.snapshot().violations, [5_000]);
        assert!(peer.with(|peer| peer.has_violation(5_000)));
        assert!(peer.snapshot().infractions.is_empty());
    }
}
