//! The attester: the forger signs the statuses its relay offers, over the forger's own session.
//!
//! - It long-polls `getStatusToSign` on the selected host with its declared keys, and polls again
//!   after each answer; when the forger's selection moves, it polls the new host only.
//! - It signs only an offer whose text is exactly the canonical form of a status tree
//!   ([`heartwood_p2p_wire::status::parse_preimage`]), whose state's height is its header's, whose
//!   slot is within one of the forger's own slot, whose nethash is its own network's, and whose
//!   keys are all its own.
//! - It answers one legacy Schnorr signature per offered key, in the offer's order (a key offered
//!   twice signs twice, as the reference signs its duplicates), naming the offer by the SHA-256 of
//!   its bytes.

use std::sync::Arc;

use heartwood_crypto::Milestones;
use heartwood_kernel::Clock;
use heartwood_p2p_wire::internal::StatusSignatures;

use crate::errors::AttestError;
use crate::keys::DelegateKeys;
use crate::seams::{AttestLink, ShutdownSignal};

/// The forger's attester.
pub struct Attester {
    #[expect(dead_code, reason = "the attester reads it")]
    keys: Arc<DelegateKeys>,
    #[expect(dead_code, reason = "the attester reads it")]
    nethash: String,
    #[expect(dead_code, reason = "the attester reads it")]
    milestones: Milestones,
    #[expect(dead_code, reason = "the attester reads it")]
    clock: Arc<dyn Clock>,
}

impl Attester {
    /// An attester with the forger's keys, its network's nethash (hex), its milestones and clock.
    pub fn new(
        keys: Arc<DelegateKeys>,
        nethash: String,
        milestones: Milestones,
        clock: Arc<dyn Clock>,
    ) -> Attester {
        Attester {
            keys,
            nethash,
            milestones,
            clock,
        }
    }

    /// The signatures of an offer: `status`, the offered text, signed with each of `public_keys`,
    /// after the checks above.
    pub fn sign_offer(
        &self,
        status: &str,
        public_keys: &[String],
    ) -> Result<StatusSignatures, AttestError> {
        let _ = (status, public_keys);
        todo!("the attester's checks and signatures")
    }

    /// Poll `link` for offers and answer them until `shutdown` is asked for.
    pub async fn run<L: AttestLink>(&self, link: &L, shutdown: ShutdownSignal) {
        let _ = (link, shutdown);
        todo!("the attester's long poll")
    }
}

/// The name of an offer: the SHA-256 of its text's bytes, in hex.
pub fn offer_hash(status: &str) -> String {
    let _ = status;
    todo!("an offer's name")
}
