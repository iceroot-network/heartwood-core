//! The forger's attestation session: from a loopback source only, at most one (a newer session
//! replaces the older with a warning), the long poll `getStatusToSign` answered with each new
//! status or `Unchanged` after 10 s, every signature of `postStatusSignatures` verified against
//! the offer it names, and the attested set: the keys whose signatures verified, in declared
//! order, duplicates kept. A session ends with its socket, with its replacement, or when no poll
//! has been pending for 15 s; its attested set is then empty.

use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use heartwood_crypto::{PublicKeyBytes, Signature};
use heartwood_kernel::Clock;
use heartwood_p2p_wire::internal::{StatusSignatures, StatusToSignRequest, StatusToSignResponse};

use crate::node_services::{AttestedKeys, NodeServices};
use crate::server::seams::{ServerRefusal, SessionEvents, SessionId};

/// The forger's attestation session.
pub struct AttestationSession {}

impl AttestationSession {
    /// A session over `node`'s registered delegates, on `clock`.
    pub fn new(node: Arc<dyn NodeServices>, clock: Arc<dyn Clock>) -> AttestationSession {
        let _ = (node, clock);
        todo!("the attestation session")
    }

    /// Offer `status`, the canonical status text, to the session's forger.
    pub fn offer(&self, status: String) {
        let _ = status;
        todo!("an offer")
    }

    /// getStatusToSign from `session` at `ip`.
    pub async fn status_to_sign(
        &self,
        session: SessionId,
        ip: IpAddr,
        request: StatusToSignRequest,
    ) -> Result<StatusToSignResponse, ServerRefusal> {
        let _ = (session, ip, request);
        todo!("the long poll")
    }

    /// postStatusSignatures from `session`.
    pub fn post_signatures(
        &self,
        session: SessionId,
        signatures: StatusSignatures,
    ) -> Result<(), ServerRefusal> {
        let _ = (session, signatures);
        todo!("the signatures' verification")
    }

    /// The verified signatures of the offer of `status`, in the offered order, waiting for them
    /// at most `wait`; `None` when they did not come.
    pub async fn signatures(
        &self,
        status: &str,
        wait: Duration,
    ) -> Option<Vec<(PublicKeyBytes, Signature)>> {
        let _ = (status, wait);
        todo!("the offer's signatures")
    }
}

impl SessionEvents for AttestationSession {
    fn opened(&self, session: SessionId, ip: IpAddr) {
        let _ = (session, ip);
        todo!("a socket opened")
    }

    fn closed(&self, session: SessionId, ip: IpAddr) {
        let _ = (session, ip);
        todo!("a socket closed")
    }
}

impl AttestedKeys for AttestationSession {
    fn attested(&self) -> Vec<PublicKeyBytes> {
        todo!("the attested set")
    }
}
