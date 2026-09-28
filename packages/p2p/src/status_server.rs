//! The served status: the header of the tip, the slot info, the reference's peer configuration
//! and the attestations, cached by the reference's key (the tip's id, `forgingAllowed`,
//! `currentSlot`). The key is looked at on a timer at each new tip, slot boundary and half-slot
//! flip, and a changed status is offered to the forger's attestation session at once; getStatus
//! waits for the signatures at most the attestation wait, then serves the header unsigned and
//! does not cache it.

use std::sync::Arc;

use heartwood_kernel::Clock;
use tokio::sync::watch;

use crate::attestation_session::AttestationSession;
use crate::node_services::NodeServices;
use crate::server::seams::ServerRefusal;

/// The served status and its cache.
pub struct StatusServer {}

impl StatusServer {
    /// The status of `node`, attested through `attestation`, on `clock`.
    pub fn new(
        node: Arc<dyn NodeServices>,
        attestation: Arc<AttestationSession>,
        clock: Arc<dyn Clock>,
    ) -> StatusServer {
        let _ = (node, attestation, clock);
        todo!("the served status")
    }

    /// The getStatus answer, encoded.
    pub async fn get_status(&self) -> Result<Vec<u8>, ServerRefusal> {
        todo!("the getStatus answer")
    }

    /// Look at the cache key, and offer a changed status to the forger's session.
    pub fn refresh(&self) {
        todo!("the status's refresh")
    }

    /// Refresh at each new tip, slot boundary and half-slot flip until `shutdown` turns true.
    pub async fn run(&self, shutdown: watch::Receiver<bool>) {
        let _ = shutdown;
        todo!("the status's timer")
    }
}
