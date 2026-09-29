//! The attester: the forger signs the statuses its relay offers, over the forger's own session.
//!
//! - It long-polls `getStatusToSign` on the selected host with its declared keys, and polls again
//!   after each answer; when the forger's selection moves, it polls the new host only. A selected
//!   host the forger does not reach over loopback attests nothing: it is not polled, and the
//!   selection is looked at again [`RETRY`] later.
//! - It signs only an offer whose text is at most [`MAX_STATUS`] bytes and exactly the canonical
//!   form of a status tree ([`heartwood_p2p_wire::status::parse_preimage`]), whose state's height
//!   is its header's, whose slot is within one of the forger's own slot, whose nethash is its own
//!   network's, and whose keys are its own, each offered at most as often as it is declared.
//! - It answers one legacy Schnorr signature per offered key, in the offer's order (a key declared
//!   and offered twice signs twice, as the reference signs its duplicates), naming the offer by
//!   the SHA-256 of its bytes, and with each key a signature of the session's challenge
//!   ([`heartwood_p2p_wire::status::session_proof`]), which shows the relay that this session
//!   holds the key.
//! - It polls again [`POLL_FLOOR`] after the poll's start at the earliest, whatever the answer,
//!   so that a relay answering at once, with `Unchanged` or with a fresh offer to sign, cannot make
//!   it spin.

use std::future::Future;
use std::pin::pin;
use std::sync::Arc;
use std::task::Poll;
use std::time::Duration;

use heartwood_crypto::crypto::hash::sha256;
use heartwood_crypto::crypto::slots;
use heartwood_crypto::utils::hex;
use heartwood_crypto::{Milestones, PublicKeyBytes};
use heartwood_kernel::{Clock, debug, warning};
use heartwood_p2p_wire::internal::{StatusSignatures, StatusToSignResponse};
use heartwood_p2p_wire::status::{attest, parse_preimage, session_proof};

use crate::errors::AttestError;
use crate::keys::DelegateKeys;
use crate::seams::{AttestLink, ShutdownSignal};

/// The wait before polling again after a failed poll.
pub const RETRY: Duration = Duration::from_secs(1);
/// The least time from the start of one poll to the next.
pub const POLL_FLOOR: Duration = Duration::from_millis(250);
/// The longest status text signed: a status is about 1 KiB.
pub const MAX_STATUS: usize = 16 * 1024;

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

    /// The signatures of an offer: `status`, the offered text, and the session's `challenge`,
    /// signed with each of `public_keys`, after the checks above.
    pub fn sign_offer(
        &self,
        status: &str,
        public_keys: &[String],
        challenge: &str,
    ) -> Result<StatusSignatures, AttestError> {
        let proof = session_proof(challenge).ok_or_else(|| AttestError::Challenge {
            challenge: challenge.chars().take(80).collect(),
        })?;
        if status.len() > MAX_STATUS {
            return Err(AttestError::Oversized {
                length: status.len(),
            });
        }
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
        // Every offered key is checked before any is signed: each one held, and none offered
        // more often than it is declared.
        let declared: Vec<(String, PublicKeyBytes)> = (self.keys.declared().into_iter())
            .map(|key| (key.to_hex(), key))
            .collect();
        let (mut unused, mut keys) = (declared.clone(), Vec::new());
        for offered in public_keys {
            let public_key = offered.clone();
            let Some(at) = unused.iter().position(|(hex, _)| hex == offered) else {
                return Err(match declared.iter().any(|(hex, _)| hex == offered) {
                    true => AttestError::Repeated { public_key },
                    false => AttestError::ForeignKey { public_key },
                });
            };
            let key = self.keys.key(&unused.swap_remove(at).1);
            keys.push(key.ok_or(AttestError::ForeignKey { public_key })?);
        }
        let sign = |text: &[u8]| {
            (keys.iter())
                .map(|key| Ok(attest(text, key)?.signature.to_hex()))
                .collect::<Result<Vec<_>, AttestError>>()
        };
        Ok(StatusSignatures {
            offer: offer_hash(status),
            signatures: sign(status.as_bytes())?,
            proofs: sign(&proof)?,
        })
    }

    /// Poll `link` for offers and answer them until `shutdown` is asked for.
    pub async fn run<L: AttestLink>(&self, link: &L, mut shutdown: ShutdownSignal) {
        let declared: Vec<String> = self.keys.declared().iter().map(|k| k.to_hex()).collect();
        loop {
            // A poll, and its offer checked and signed; after a failed poll, or none as the
            // selected host is remote, a pause.
            let floor = tokio::time::Instant::now().checked_add(POLL_FLOOR);
            let poll = async {
                if !link.selected_is_local() {
                    tokio::time::sleep(RETRY).await;
                    return None;
                }
                match link.status_to_sign(&declared).await {
                    Ok(StatusToSignResponse::Offer {
                        status,
                        public_keys,
                        challenge,
                    }) => Some(self.sign_offer(&status, &public_keys, &challenge)),
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
            let pause = floor.map(tokio::time::sleep_until);
            if let Some(pause) = pause
                && until(&mut shutdown, pause).await.is_none()
            {
                return;
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
