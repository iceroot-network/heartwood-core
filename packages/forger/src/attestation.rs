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

use std::future::Future;
use std::pin::pin;
use std::sync::Arc;
use std::task::Poll;
use std::time::Duration;

use heartwood_crypto::Milestones;
use heartwood_crypto::crypto::hash::sha256;
use heartwood_crypto::crypto::slots;
use heartwood_crypto::utils::hex;
use heartwood_kernel::{Clock, debug, warning};
use heartwood_p2p_wire::internal::{StatusSignatures, StatusToSignResponse};
use heartwood_p2p_wire::status::{attest, parse_preimage};

use crate::errors::AttestError;
use crate::keys::DelegateKeys;
use crate::seams::{AttestLink, ShutdownSignal};

/// The wait before polling again after a failed poll.
pub const RETRY: Duration = Duration::from_secs(1);

/// The forger's attester.
pub struct Attester {
    keys: Arc<DelegateKeys>,
    nethash: String,
    milestones: Milestones,
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
        let (state, config) = parse_preimage(status.as_bytes())?;
        let (height, header, slot) = (state.height, state.header.height, state.current_slot);
        let own = self.own_slot();
        if height != header {
            return Err(AttestError::Height {
                state: height,
                header,
            });
        } else if slot.abs_diff(own) > 1 {
            return Err(AttestError::Slot { offered: slot, own });
        } else if config.network.nethash != self.nethash {
            let (offered, own) = (config.network.nethash, self.nethash.clone());
            return Err(AttestError::Network { offered, own });
        }
        let declared = self.keys.declared();
        let sign = |offered: &String| {
            let key = declared.iter().find(|key| key.to_hex() == *offered);
            let foreign = || AttestError::ForeignKey {
                public_key: offered.clone(),
            };
            let key = key.and_then(|key| self.keys.key(key)).ok_or_else(foreign)?;
            Ok(attest(status.as_bytes(), key)?.signature.to_hex())
        };
        let signatures = (public_keys.iter())
            .map(sign)
            .collect::<Result<_, AttestError>>()?;
        Ok(StatusSignatures {
            offer: offer_hash(status),
            signatures,
        })
    }

    /// Poll `link` for offers and answer them until `shutdown` is asked for.
    pub async fn run<L: AttestLink>(&self, link: &L, mut shutdown: ShutdownSignal) {
        let declared: Vec<String> = self.keys.declared().iter().map(|k| k.to_hex()).collect();
        loop {
            // A poll, and its offer checked and signed; after a failed poll, a pause.
            let poll = async {
                match link.status_to_sign(&declared).await {
                    Ok(StatusToSignResponse::Offer {
                        status,
                        public_keys,
                    }) => Some(self.sign_offer(&status, &public_keys)),
                    Ok(StatusToSignResponse::Unchanged) => None,
                    Err(error) => {
                        debug!("The relay gave no status to attest: {error}");
                        tokio::time::sleep(RETRY).await;
                        None
                    }
                }
            };
            match until(&mut shutdown, poll).await {
                None => return,
                Some(None) => {}
                Some(Some(Ok(signatures))) => {
                    if let Err(error) = link.post_signatures(signatures).await {
                        debug!("The status attestations were not delivered: {error}");
                    }
                }
                Some(Some(Err(error))) => warning!("Not attesting the relay's status: {error}"),
            }
        }
    }

    /// The slot of the forger's clock.
    fn own_slot(&self) -> i64 {
        let first = self.milestones.at(1);
        let epoch = slots::epoch_unix_ms(first.epoch()).unwrap_or_default();
        let now = slots::epoch_time(self.clock.now_ms(), epoch);
        slots::slot_number(now, first.block_time())
    }
}

/// The name of an offer: the SHA-256 of its text's bytes, in hex.
pub fn offer_hash(status: &str) -> String {
    hex::encode(&sha256(status.as_bytes()))
}

/// `future`'s output, or `None` once a shutdown is asked for.
async fn until<F: Future>(shutdown: &mut ShutdownSignal, future: F) -> Option<F::Output> {
    if shutdown.is_requested() {
        return None;
    }
    let mut future = pin!(future);
    let mut stop = pin!(shutdown.requested());
    std::future::poll_fn(|cx| match future.as_mut().poll(cx) {
        Poll::Ready(output) => Poll::Ready(Some(output)),
        Poll::Pending => stop.as_mut().poll(cx).map(|()| None),
    })
    .await
}
