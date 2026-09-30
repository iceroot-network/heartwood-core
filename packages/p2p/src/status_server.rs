//! The served status: the header of the tip, the slot info, the reference's peer configuration
//! and the attestations, cached by the reference's key (the tip's id, `forgingAllowed`,
//! `currentSlot`). The key is looked at on a timer at each new tip, slot boundary and half-slot
//! flip, and a changed status is offered to the forger's attestation session at once; getStatus
//! waits for the signatures at most the attestation wait, then serves the header unsigned and
//! does not cache it.
//!
//! A status served without a forger's session is cached unsigned, as the reference caches it
//! without a live forger; once a session attaches, the next getStatus of the same key waits for
//! its signatures instead of serving that cache.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use heartwood_crypto::BlockId;
use heartwood_crypto::crypto::slots::{self, SlotInfo};
use heartwood_kernel::Clock;
use heartwood_p2p_wire::Proto;
use heartwood_p2p_wire::status::{self, PeerConfig, SignedState, StatusHeaderData};
use tokio::sync::watch;

use crate::attestation_session::{AttestationSession, Signing};
use crate::node_services::{AttestationStep, NodeServices};
use crate::server::ATTESTATION_WAIT;
use crate::server::seams::ServerRefusal;

/// How often the timer looks at the tip between the slot's changes.
pub const TIP_POLL: Duration = Duration::from_millis(100);

/// The reference's cache key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Key {
    id: BlockId,
    forging_allowed: bool,
    slot: i64,
}

/// A served status kept for its key.
struct Cached {
    key: Key,
    bytes: Vec<u8>,
    /// Whether a forger's session signed it (possibly with no key).
    attested: bool,
}

/// The status of the tip at one moment.
struct Current {
    key: Key,
    state: SignedState,
    config: PeerConfig,
    /// The canonical status text: what a delegate signs.
    text: String,
}

/// The served status and its cache.
pub struct StatusServer {
    node: Arc<dyn NodeServices>,
    attestation: Arc<AttestationSession>,
    clock: Arc<dyn Clock>,
    wait: Duration,
    cache: Mutex<Option<Cached>>,
    offered: Mutex<Option<Key>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl StatusServer {
    /// The status of `node`, attested through `attestation`, on `clock`.
    pub fn new(
        node: Arc<dyn NodeServices>,
        attestation: Arc<AttestationSession>,
        clock: Arc<dyn Clock>,
    ) -> StatusServer {
        StatusServer {
            node,
            attestation,
            clock,
            wait: ATTESTATION_WAIT,
            cache: Mutex::new(None),
            offered: Mutex::new(None),
        }
    }

    /// The same status server, waiting at most `wait` for a status's signatures (the server's
    /// `attestation_wait`) instead of [`ATTESTATION_WAIT`].
    pub fn with_wait(self, wait: Duration) -> StatusServer {
        StatusServer { wait, ..self }
    }

    /// The getStatus answer, encoded.
    pub async fn get_status(&self) -> Result<Vec<u8>, ServerRefusal> {
        let key = self.key(&self.slot());
        let cached = lock(&self.cache).as_ref().and_then(|cached| {
            let fresh = cached.attested || !self.attestation.is_attached();
            (cached.key == key && fresh).then(|| cached.bytes.clone())
        });
        if let Some(bytes) = cached {
            return Ok(bytes);
        }
        let current = self.offer_current();
        let signing = self.attestation.signatures(&current.text, self.wait).await;
        let (attestations, keep, attested) = match signing {
            Signing::NoSession => (Vec::new(), true, false),
            Signing::Signed(attestations) => (attestations, true, true),
            Signing::Unsigned => (Vec::new(), false, false),
        };
        let bytes = status::response(&current.state, &current.config, &attestations).encode();
        if keep {
            let (key, bytes) = (current.key, bytes.clone());
            *lock(&self.cache) = Some(Cached {
                key,
                bytes,
                attested,
            });
        }
        Ok(bytes)
    }

    /// Look at the cache key, and offer a changed status to the forger's session.
    pub fn refresh(&self) {
        let key = self.key(&self.slot());
        if *lock(&self.offered) != Some(key) {
            self.offer_current();
        }
    }

    /// The tip's status now, offered to the forger's session when its key is not the one offered
    /// last. The status is read and offered under one lock, so an older status never replaces a
    /// newer one.
    fn offer_current(&self) -> Current {
        let mut offered = lock(&self.offered);
        let current = self.current();
        if *offered != Some(current.key) {
            *offered = Some(current.key);
            self.attestation.offer(current.text.clone());
        }
        current
    }

    /// The attestation record's hook at `step` of the status `current`: the node's record in a
    /// harness build (`difftest`), nothing in any other build.
    #[allow(
        dead_code,
        reason = "called at the attestation record's three points: an offer, the signatures' attachment and a status served unsigned"
    )]
    fn attestation_step(&self, step: AttestationStep, current: &Current) {
        #[cfg(feature = "difftest")]
        self.node
            .attestation_step(&crate::node_services::AttestationRecord {
                at_ms: self.clock.now_ms(),
                step,
                status: heartwood_crypto::utils::hex::encode(
                    &heartwood_crypto::crypto::hash::sha256(current.text.as_bytes()),
                ),
                slot: current.key.slot,
            });
        #[cfg(not(feature = "difftest"))]
        let _ = (step, current);
    }

    /// Refresh at each new tip, slot boundary and half-slot flip until `shutdown` turns true.
    pub async fn run(&self, mut shutdown: watch::Receiver<bool>) {
        let mut queue = self.node.queue_state();
        let mut queue_open = true;
        while !*shutdown.borrow() {
            self.refresh();
            let pause = self.until_next_change().min(TIP_POLL);
            let queue_changed = async {
                if !queue_open || queue.changed().await.is_err() {
                    queue_open = false;
                    std::future::pending::<()>().await;
                }
            };
            tokio::select! {
                () = tokio::time::sleep(pause) => {}
                () = queue_changed => {}
                changed = shutdown.changed() => if changed.is_err() { return },
            }
        }
    }

    /// The slot at the clock's time.
    fn slot(&self) -> SlotInfo {
        let chain = self.node.chain();
        let first = chain.milestones().at(1);
        let epoch = slots::epoch_unix_ms(first.epoch()).unwrap_or_default();
        let now = slots::epoch_time(self.clock.now_ms(), epoch);
        slots::slot_info(now, first.block_time())
    }

    /// The time until the next slot boundary or half-slot flip.
    fn until_next_change(&self) -> Duration {
        let chain = self.node.chain();
        let first = chain.milestones().at(1);
        let epoch = slots::epoch_unix_ms(first.epoch()).unwrap_or_default();
        let block_ms = i64::try_from(first.block_time().max(1)).unwrap_or(i64::MAX / 1_000);
        let (block_ms, half_ms) = (
            block_ms.saturating_mul(1_000),
            (block_ms / 2).saturating_mul(1_000),
        );
        let into = self
            .clock
            .now_ms()
            .saturating_sub(epoch)
            .rem_euclid(block_ms);
        let next = if into < half_ms { half_ms } else { block_ms };
        Duration::from_millis(u64::try_from(next.saturating_sub(into)).unwrap_or(1))
    }

    fn key(&self, slot: &SlotInfo) -> Key {
        Key {
            id: *self.node.chain().last_block().id(),
            forging_allowed: slot.forging_status,
            slot: slot.slot_number,
        }
    }

    /// The tip's status now.
    fn current(&self) -> Current {
        let slot = self.slot();
        let tip = self.node.chain().last_block();
        let state = SignedState {
            height: tip.height(),
            forging_allowed: slot.forging_status,
            current_slot: slot.slot_number,
            header: StatusHeaderData::from_block(&tip),
        };
        let config = self.node.peer_config();
        let text = String::from_utf8_lossy(&status::signing_preimage(&state, &config)).into_owned();
        let key = Key {
            id: *tip.id(),
            forging_allowed: slot.forging_status,
            slot: slot.slot_number,
        };
        Current {
            key,
            state,
            config,
            text,
        }
    }
}
