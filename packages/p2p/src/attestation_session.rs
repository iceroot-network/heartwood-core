//! The forger's attestation session: from a loopback source only, declaring at most
//! [`MAX_DECLARED`] keys, at most one (a newer session replaces the older with a warning), the
//! long poll `getStatusToSign` answered with each new status or `Unchanged` after 10 s, every
//! signature of `postStatusSignatures` verified against the offer it names, and the attested set:
//! the keys whose signatures verified, in declared order, duplicates kept. A session ends with
//! its socket, with its replacement, or when no poll has been pending for 15 s; its attested set
//! is then empty.
//!
//! An offer holds the status text, the keys to sign it with (the declared keys that are
//! registered delegates, in declared order) and the session's challenge, 32 random bytes drawn
//! when the session begins. A key is attested only when the session also signs the challenge
//! with it ([`heartwood_p2p_wire::status::session_proof`]): the served status's signatures are
//! public, so a signature of the status alone does not show that the session holds the key. An
//! offer without such a key is never sent; its status is served with no attestation at once. The
//! status server waits for an offer's signatures ([`AttestationSession::signatures`]); after one
//! wait that ran out, the same offer is served unsigned without another.
//!
//! A harness build records each offer and the attachment of its signatures when they happen
//! ([`NodeServices::attestation_step`]), whether or not a getStatus waits for them.

use std::collections::HashSet;
use std::net::IpAddr;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use heartwood_crypto::crypto::hash::sha256;
use heartwood_crypto::utils::hex;
use heartwood_crypto::{PublicKeyBytes, Signature};
use heartwood_kernel::{Clock, debug, warning};
use heartwood_p2p_wire::internal::{StatusSignatures, StatusToSignRequest, StatusToSignResponse};
use heartwood_p2p_wire::status::{Attestation, session_proof, verify_attestation};
use tokio::sync::watch;
use tokio::time::{Instant, timeout_at};

use crate::node_services::{AttestationRecord, AttestationStep, AttestedKeys, NodeServices};
use crate::server::seams::{ServerRefusal, SessionEvents, SessionId};

/// How long a poll waits for a new status before it answers `Unchanged`.
pub const LONG_POLL: Duration = Duration::from_secs(10);
/// How long a session lasts without a pending poll.
pub const IDLE_END_MS: i64 = 15_000;
/// The most keys a session declares: a key file holds a few dozen.
pub const MAX_DECLARED: usize = 256;

/// What the status server gets for an offer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Signing {
    /// No forger's session: the status is served unsigned, as the reference serves it without a
    /// live forger.
    NoSession,
    /// The verified attestations, in the offered order (none when no declared key is a
    /// registered delegate).
    Signed(Vec<Attestation>),
    /// The signatures did not come within the wait, or the status changed.
    Unsigned,
}

/// One offer of a status to the session's forger.
struct Offer {
    number: u64,
    status: String,
    /// The SHA-256 of the status text, in hex, which names the offer.
    name: String,
    /// The status's slot.
    slot: i64,
    keys: Vec<PublicKeyBytes>,
    signatures: Option<Vec<Attestation>>,
    /// A wait for the signatures ran out.
    waited: bool,
}

/// The forger's session.
struct Session {
    id: SessionId,
    ip: IpAddr,
    declared: Vec<PublicKeyBytes>,
    /// The challenge its keys sign, in hex.
    challenge: String,
    polls: usize,
    idle_since_ms: i64,
    /// The number of the last offer a poll answered.
    delivered: u64,
    offer: Option<Offer>,
    attested: Vec<PublicKeyBytes>,
}

#[derive(Default)]
struct Inner {
    /// The last status offered, kept for the next session.
    status: Option<String>,
    /// The last status's slot.
    slot: i64,
    session: Option<Session>,
    offers: u64,
}

/// The forger's attestation session.
pub struct AttestationSession {
    node: Arc<dyn NodeServices>,
    clock: Arc<dyn Clock>,
    inner: Mutex<Inner>,
    /// Counts the changes that a poll or a wait for signatures may be waiting for.
    changes: watch::Sender<u64>,
}

impl AttestationSession {
    /// A session over `node`'s registered delegates, on `clock`.
    pub fn new(node: Arc<dyn NodeServices>, clock: Arc<dyn Clock>) -> AttestationSession {
        AttestationSession {
            node,
            clock,
            inner: Mutex::default(),
            changes: watch::channel(0).0,
        }
    }

    /// Offer `status`, the canonical status text of the slot `slot`, to the session's forger.
    pub fn offer(&self, status: String, slot: i64) {
        let mut inner = self.lock();
        if inner.status.as_ref() == Some(&status) {
            return;
        }
        inner.status = Some(status);
        inner.slot = slot;
        let noted = self.new_offer(&mut inner);
        drop(inner);
        self.record(noted);
        self.changed();
    }

    /// getStatusToSign from `session` at `ip`.
    pub async fn status_to_sign(
        &self,
        session: SessionId,
        ip: IpAddr,
        request: StatusToSignRequest,
    ) -> Result<StatusToSignResponse, ServerRefusal> {
        if !ip.is_loopback() {
            let only = "Status attestations are given to a forger on the relay's host only";
            return Err(ServerRefusal::new(499, only));
        }
        if request.public_keys.len() > MAX_DECLARED {
            let many = format!("An attestation session declares at most {MAX_DECLARED} keys");
            return Err(ServerRefusal::new(499, &many));
        }
        let declared = (request.public_keys.iter())
            .map(|key| PublicKeyBytes::from_hex(key))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| ServerRefusal::new(400, "Validation failed"))?;
        let mut changes = self.changes.subscribe();
        self.attach(session, ip, declared);
        let _poll = Poll {
            attestation: self,
            session,
        };
        let deadline = after(LONG_POLL);
        loop {
            {
                let mut inner = self.lock();
                let Some(current) = inner.session.as_mut().filter(|s| s.id == session) else {
                    return Err(ServerRefusal::new(499, "The attestation session has ended"));
                };
                let delivered = current.delivered;
                let fresh = current
                    .offer
                    .as_ref()
                    .filter(|offer| offer.number > delivered);
                if let Some(offer) = fresh.filter(|offer| !offer.keys.is_empty()) {
                    let public_keys = offer.keys.iter().map(PublicKeyBytes::to_hex).collect();
                    let (status, challenge) = (offer.status.clone(), current.challenge.clone());
                    current.delivered = offer.number;
                    return Ok(StatusToSignResponse::Offer {
                        status,
                        public_keys,
                        challenge,
                    });
                }
            }
            if !matches!(timeout_at(deadline, changes.changed()).await, Ok(Ok(()))) {
                return Ok(StatusToSignResponse::Unchanged);
            }
        }
    }

    /// postStatusSignatures from `session`: the signatures of the offer they name, verified and
    /// attached in the offered order, each key's with its proof of the session's challenge;
    /// signatures of another offer, or of a session that is not the current one, are dropped.
    /// They are verified without the session's lock, and attached only if their offer is still
    /// the current one then.
    pub fn post_signatures(
        &self,
        session: SessionId,
        signatures: StatusSignatures,
    ) -> Result<(), ServerRefusal> {
        let named = {
            let mut inner = self.lock();
            self.expire(&mut inner);
            let Some(current) = inner.session.as_ref().filter(|s| s.id == session) else {
                debug!("Dropping the status signatures of an attestation session that has ended");
                return Ok(());
            };
            let delivered = current.delivered;
            let named =
                |offer: &&Offer| offer.name == signatures.offer && offer.number <= delivered;
            let offer = current.offer.as_ref().filter(named);
            let proof = session_proof(&current.challenge).unwrap_or_default();
            offer.map(|offer| {
                (
                    offer.number,
                    offer.status.clone(),
                    offer.keys.clone(),
                    proof,
                )
            })
        };
        let Some((number, status, keys, proof)) = named else {
            debug!("Dropping status signatures for a status that is no longer offered");
            return Ok(());
        };
        let (texts, proofs) = (&signatures.signatures, &signatures.proofs);
        let verified = (keys.iter().zip(texts).zip(proofs)).filter_map(|((key, text), proven)| {
            let signature = Signature::from_hex(text).ok();
            let valid = verify_attestation(status.as_bytes(), &key.to_hex(), text)
                && verify_attestation(&proof, &key.to_hex(), proven);
            if !valid {
                debug!(
                    "The status signature or proof of {} did not verify",
                    key.to_hex()
                );
            }
            signature.filter(|_| valid).map(|signature| Attestation {
                public_key: *key,
                signature,
            })
        });
        let verified: Vec<Attestation> = verified.collect();
        let mut inner = self.lock();
        let current = inner.session.as_mut().filter(|s| s.id == session);
        let Some(current) =
            current.filter(|s| s.offer.as_ref().is_some_and(|o| o.number == number))
        else {
            debug!("Dropping status signatures for a status that is no longer offered");
            return Ok(());
        };
        current.attested = verified.iter().map(|a| a.public_key).collect();
        let mut noted = Vec::new();
        if let Some(offer) = current.offer.as_mut() {
            let signed = AttestationStep::Signed {
                attestations: verified.len(),
            };
            noted.extend(self.noted(signed, offer));
            offer.signatures = Some(verified);
        }
        drop(inner);
        self.record(noted);
        self.changed();
        Ok(())
    }

    /// The signing of `status`, the status offered last, waiting for the forger's signatures at
    /// most `wait`.
    pub async fn signatures(&self, status: &str, wait: Duration) -> Signing {
        let mut changes = self.changes.subscribe();
        let deadline = after(wait);
        loop {
            {
                let mut inner = self.lock();
                self.expire(&mut inner);
                let Some(session) = inner.session.as_mut() else {
                    return Signing::NoSession;
                };
                let Some(offer) = session.offer.as_mut().filter(|o| o.status == status) else {
                    return Signing::Unsigned;
                };
                if let Some(signatures) = &offer.signatures {
                    return Signing::Signed(signatures.clone());
                }
                if offer.waited || Instant::now() >= deadline {
                    offer.waited = true;
                    return Signing::Unsigned;
                }
            }
            if let Ok(Err(_)) = timeout_at(deadline, changes.changed()).await {
                return Signing::Unsigned;
            }
        }
    }

    /// Whether a forger's session is attached.
    pub fn is_attached(&self) -> bool {
        let mut inner = self.lock();
        self.expire(&mut inner);
        inner.session.is_some()
    }

    /// The delegates on this node: the attested keys that are active in the round of the tip
    /// plus one, in declared order, duplicates kept (the reference's `getDelegatesOnThisNode`);
    /// none when the round cannot be read.
    pub fn on_this_node(&self) -> Vec<PublicKeyBytes> {
        let attested = self.attested();
        if attested.is_empty() {
            return attested;
        }
        let active = self.node.active_keys_next_round().unwrap_or_default();
        attested
            .into_iter()
            .filter(|key| active.contains(key))
            .collect()
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn changed(&self) {
        self.changes
            .send_modify(|count| *count = count.wrapping_add(1));
    }

    /// Make `session` from `ip` the current session, declaring `declared`, and count its poll.
    fn attach(&self, session: SessionId, ip: IpAddr, declared: Vec<PublicKeyBytes>) {
        let mut inner = self.lock();
        self.expire(&mut inner);
        let now = self.clock.now_ms();
        let same = inner.session.as_ref().is_some_and(|s| s.id == session);
        if let (false, Some(old)) = (same, &inner.session) {
            warning!(
                "A new attestation session from {ip} replaces the session from {}",
                old.ip
            );
        }
        let stays = inner
            .session
            .as_ref()
            .is_some_and(|s| same && s.declared == declared);
        let mut noted = Vec::new();
        if !stays {
            let mut seen = HashSet::new();
            let repeated = declared.iter().filter(|key| !seen.insert(**key)).count();
            if repeated > 0 {
                warning!(
                    "The attestation session from {ip} declares {repeated} keys again; each copy \
                     counts as a delegate on this node"
                );
            }
            let polls = if same {
                inner.session.as_ref().map_or(0, |s| s.polls)
            } else {
                0
            };
            inner.session = Some(Session {
                id: session,
                ip,
                declared,
                challenge: hex::encode(&rand::random::<[u8; 32]>()),
                polls,
                idle_since_ms: now,
                delivered: 0,
                offer: None,
                attested: Vec::new(),
            });
            noted = self.new_offer(&mut inner);
        }
        if let Some(current) = inner.session.as_mut() {
            current.polls = current.polls.saturating_add(1);
        }
        drop(inner);
        self.record(noted);
        self.changed();
    }

    /// Offer the last status to the current session, with its keys; the records of the offer,
    /// and of its signatures when no key is to sign it.
    fn new_offer(&self, inner: &mut Inner) -> Vec<AttestationRecord> {
        let (Some(status), Some(session)) = (inner.status.clone(), inner.session.as_mut()) else {
            return Vec::new();
        };
        let chain = self.node.chain();
        let keys: Vec<PublicKeyBytes> = (session.declared.iter())
            .filter(|key| chain.delegate_by_key(key).is_some())
            .copied()
            .collect();
        inner.offers = inner.offers.wrapping_add(1);
        let offer = session.offer.insert(Offer {
            number: inner.offers,
            name: hex::encode(&sha256(status.as_bytes())),
            status,
            slot: inner.slot,
            signatures: keys.is_empty().then(Vec::new),
            keys,
            waited: false,
        });
        let mut noted: Vec<AttestationRecord> = self
            .noted(AttestationStep::Offered, offer)
            .into_iter()
            .collect();
        if offer.signatures.is_some() {
            let none = AttestationStep::Signed { attestations: 0 };
            noted.extend(self.noted(none, offer));
        }
        noted
    }

    /// The record of `step` of `offer`, now: in a harness build only.
    fn noted(&self, step: AttestationStep, offer: &Offer) -> Option<AttestationRecord> {
        cfg!(feature = "difftest").then(|| AttestationRecord {
            at_ms: self.clock.now_ms(),
            step,
            status: offer.name.clone(),
            slot: offer.slot,
        })
    }

    /// Hand `records` to the node's attestation record, outside the session's lock.
    fn record(&self, records: Vec<AttestationRecord>) {
        for record in &records {
            self.node.attestation_step(record);
        }
    }

    /// End the session when no poll has been pending for 15 s.
    fn expire(&self, inner: &mut Inner) {
        let now = self.clock.now_ms();
        let idle = |s: &Session| s.polls == 0 && now.saturating_sub(s.idle_since_ms) >= IDLE_END_MS;
        if let Some(session) = inner.session.take_if(|s| idle(s)) {
            debug!(
                "The attestation session from {} ended without a poll",
                session.ip
            );
        }
    }
}

/// The moment `wait` from now.
fn after(wait: Duration) -> Instant {
    let now = Instant::now();
    now.checked_add(wait).unwrap_or(now)
}

/// A pending poll, counted while it lasts.
struct Poll<'a> {
    attestation: &'a AttestationSession,
    session: SessionId,
}

impl Drop for Poll<'_> {
    fn drop(&mut self) {
        let now = self.attestation.clock.now_ms();
        let mut inner = self.attestation.lock();
        if let Some(session) = inner.session.as_mut().filter(|s| s.id == self.session) {
            session.polls = session.polls.saturating_sub(1);
            if session.polls == 0 {
                session.idle_since_ms = now;
            }
        }
    }
}

impl SessionEvents for AttestationSession {
    fn opened(&self, _session: SessionId, _ip: IpAddr) {}

    fn closed(&self, session: SessionId, ip: IpAddr) {
        let ended = self.lock().session.take_if(|s| s.id == session);
        if ended.is_some() {
            debug!("The attestation session from {ip} ended with its socket");
            self.changed();
        }
    }
}

impl AttestedKeys for AttestationSession {
    fn attested(&self) -> Vec<PublicKeyBytes> {
        let mut inner = self.lock();
        self.expire(&mut inner);
        inner
            .session
            .as_ref()
            .map(|session| session.attested.clone())
            .unwrap_or_default()
    }
}

/// The delegates on this node as the network monitor's fork health check reads them: the
/// session's [`AttestationSession::on_this_node`].
pub struct DelegatesOnThisNode(pub Arc<AttestationSession>);

impl AttestedKeys for DelegatesOnThisNode {
    fn attested(&self) -> Vec<PublicKeyBytes> {
        self.0.on_this_node()
    }
}
