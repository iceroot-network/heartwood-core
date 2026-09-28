//! The seams between the forger's parts: each trait is implemented by one part and called by
//! another, so every part can be tested against fakes of its neighbours.
//!
//! | Trait | Implemented by | Called by |
//! |---|---|---|
//! | [`RelayLink`] | [`crate::client::Client`] (a scripted relay in tests) | the slot loop |
//! | [`AttestLink`] | [`crate::client::Client`] (a scripted relay in tests) | the attester |
//! | [`BlockSigner`] | [`crate::signer::Signer`] (a recording fake in tests) | the slot loop |
//!
//! The link's methods return `Send` futures, as the component's lifecycle requires; the forger
//! runs them on one current-thread runtime. The small types the parts share are here too.

use std::future::Future;
use std::time::Duration;

use heartwood_crypto::{BlockId, PublicKeyBytes, Transaction};
use heartwood_kernel::Event;
use heartwood_p2p_wire::internal::{
    CurrentRound, NetworkState, StatusSignatures, StatusToSignResponse,
};
use heartwood_p2p_wire::status::ReceivedStatus;
use serde_json::{Value, json};
use tokio::sync::watch;
use tokio::time::Instant;

use crate::errors::{RelayError, SignError};

/// When a relay call gives up, on tokio's clock (which the tests pause). A call past its
/// deadline fails as a [`RelayError::Communication`], as the reference's own failures count.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Deadline(pub Instant);

impl Deadline {
    /// The deadline `timeout` from now (now itself if the sum overflowed, which no call's timeout
    /// comes near).
    pub fn after(timeout: Duration) -> Deadline {
        let now = Instant::now();
        Deadline(now.checked_add(timeout).unwrap_or(now))
    }

    /// The time left before the deadline: zero when it is now or past.
    pub fn remaining(&self) -> Duration {
        self.0.saturating_duration_since(Instant::now())
    }
}

/// The pool's transactions for a block, as `getUnconfirmedTransactions` answers the forger.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ForgingTransactions {
    /// The number of transactions in the relay's pool.
    pub pool_size: u32,
    /// The collated transactions' bytes, in the collator's order.
    pub transactions: Vec<Vec<u8>>,
}

/// An event the forger sends its relay (`emitEvent`): the reference's four forger events.
#[derive(Debug, Clone, PartialEq)]
pub enum ForgerEvent {
    /// `forger.started`, once the first round is loaded: `{activeDelegates: [public key...]}`,
    /// every delegate of the forger.
    Started {
        /// The forger's delegates' public keys, in hex.
        active_delegates: Vec<String>,
    },
    /// `forger.failed`, when a slot check fails with an error that is not a relay's:
    /// `{error: message}`.
    Failed {
        /// The error's message.
        error: String,
    },
    /// `block.forged`, after a block was posted: the block's header data.
    BlockForged {
        /// The header data, as `block.getHeader()` writes it.
        header: Value,
    },
    /// `transaction.forged`, after a block was posted, once per transaction: its data.
    TransactionForged {
        /// The transaction's data, as the reference writes it.
        transaction: Value,
    },
}

impl ForgerEvent {
    /// The event's name, as the relay dispatches it.
    pub fn name(&self) -> &'static str {
        let event = match self {
            ForgerEvent::Started { .. } => Event::ForgerStarted,
            ForgerEvent::Failed { .. } => Event::ForgerFailed,
            ForgerEvent::BlockForged { .. } => Event::BlockForged,
            ForgerEvent::TransactionForged { .. } => Event::TransactionForged,
        };
        event.name()
    }

    /// The event's body.
    pub fn body(&self) -> Value {
        match self {
            ForgerEvent::Started { active_delegates } => {
                json!({ "activeDelegates": active_delegates })
            }
            ForgerEvent::Failed { error } => json!({ "error": error }),
            ForgerEvent::BlockForged { header } => header.clone(),
            ForgerEvent::TransactionForged { transaction } => transaction.clone(),
        }
    }
}

/// The forger's calls to its selected relay: the reference's client, with a deadline on every
/// call ([`crate::deadlines`]).
///
/// `get_round`, `get_slot_number` and `sync_with_network` select a host first; the other calls
/// reuse the host last selected.
pub trait RelayLink: Send + Sync {
    /// `getCurrentRound`.
    fn get_round(&self) -> impl Future<Output = Result<CurrentRound, RelayError>> + Send;

    /// `getSlotNumber`: the slot of `timestamp` (seconds after the epoch), or of the relay's now.
    fn get_slot_number(
        &self,
        timestamp: Option<i64>,
        deadline: Deadline,
    ) -> impl Future<Output = Result<i64, RelayError>> + Send;

    /// `getNetworkState`, read as `NetworkState.parse` reads it; `log` on a slot's first attempt.
    fn get_network_state(
        &self,
        log: bool,
        deadline: Deadline,
    ) -> impl Future<Output = Result<NetworkState, RelayError>> + Send;

    /// `getStatus`, with the forger's headers.
    fn get_status(
        &self,
        deadline: Deadline,
    ) -> impl Future<Output = Result<ReceivedStatus, RelayError>> + Send;

    /// `getUnconfirmedTransactions`, with the forger's headers; `None` for an empty answer.
    fn get_transactions(
        &self,
        deadline: Deadline,
    ) -> impl Future<Output = Result<Option<ForgingTransactions>, RelayError>> + Send;

    /// `postBlock` of `bytes`, a block with its transactions; a failure is logged ("Broadcast block
    /// failed: ..."), not returned.
    fn broadcast_block(&self, bytes: &[u8], deadline: Deadline) -> impl Future<Output = ()> + Send;

    /// `syncBlockchain`: a failed call is logged ("Could not sync check: ..."); a host selection
    /// that finds no ready host is returned, as it leaves the reference's slot check.
    fn sync_with_network(&self) -> impl Future<Output = Result<(), RelayError>> + Send;

    /// `emitEvent`, sent only when some host's address text contains `127.0.0.1` or `::1` (the
    /// reference's substring test), to the selected host; otherwise, and on a failure, an error
    /// line is logged.
    fn emit_event(&self, event: ForgerEvent) -> impl Future<Output = ()> + Send;

    /// Close every session to the relays and stop reconnecting (the reference's `dispose`): the
    /// relays' statuses become unsigned.
    fn dispose(&self);
}

/// The attestation routes, on the selected host's session.
pub trait AttestLink: Send + Sync {
    /// `getStatusToSign`: the long poll for a status to attest, with the forger's declared keys
    /// (file order, duplicates kept).
    fn status_to_sign(
        &self,
        public_keys: &[String],
    ) -> impl Future<Output = Result<StatusToSignResponse, RelayError>> + Send;

    /// `postStatusSignatures`: the attestations of one offer.
    fn post_signatures(
        &self,
        signatures: StatusSignatures,
    ) -> impl Future<Output = Result<(), RelayError>> + Send;
}

/// The inputs of a block, as the relay's round and network state give them.
#[derive(Debug, Clone, Copy)]
pub struct ForgeInputs<'a> {
    /// The transactions, in the collator's order.
    pub transactions: &'a [Transaction],
    /// The previous block's id: the network state's last block id.
    pub previous_id: BlockId,
    /// The previous block's height: the network state's node height.
    pub previous_height: u32,
    /// The round's block timestamp, seconds after the epoch.
    pub timestamp: u32,
    /// The round's reward.
    pub reward: u64,
}

/// A block a delegate signed for a slot, as the signing record keeps it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signed {
    /// The block's height.
    pub height: u32,
    /// The block's slot.
    pub slot: i64,
    /// The block's id.
    pub id: BlockId,
    /// The block with its transactions, as `postBlock` sends it.
    pub bytes: Vec<u8>,
}

/// What the signer gave for a slot: a block signed now, or the one signed for the slot before.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignedOutcome {
    /// A new block, recorded before it is returned.
    New(Signed),
    /// The block already recorded for this key and slot, to be sent again as it is.
    Resent(Signed),
}

impl SignedOutcome {
    /// The block to send.
    pub fn signed(&self) -> &Signed {
        match self {
            SignedOutcome::New(signed) | SignedOutcome::Resent(signed) => signed,
        }
    }
}

/// Signs a delegate's block for a slot, once: the key, the signing record's decision, the block
/// and its signature behind one call.
///
/// `slot` is the slot of the block's own timestamp (`inputs.timestamp`), by the forger's
/// milestones, and `own_slot` the slot of the forger's own clock: a slot more than one away from
/// it is refused and nothing is recorded. A slot below the key's recorded slot is refused; the
/// recorded slot gives back its recorded block; a later slot is signed and recorded before it is
/// returned.
pub trait BlockSigner: Send {
    /// The block of `key`'s delegate for `slot`, from `inputs`.
    fn sign_or_resend(
        &mut self,
        key: &PublicKeyBytes,
        inputs: &ForgeInputs<'_>,
        own_slot: i64,
        slot: i64,
    ) -> Result<SignedOutcome, SignError>;
}

/// Asks the forger's tasks to stop: the slot loop stops scheduling, an in-flight `postBlock` runs
/// to its deadline, and the attester ends its poll.
#[derive(Debug)]
pub struct ShutdownTrigger(watch::Sender<bool>);

/// What the forger's tasks watch for a shutdown. Clones watch the same trigger.
#[derive(Debug, Clone)]
pub struct ShutdownSignal(watch::Receiver<bool>);

impl ShutdownSignal {
    /// A trigger and the signal it sets.
    pub fn new() -> (ShutdownTrigger, ShutdownSignal) {
        let (sender, receiver) = watch::channel(false);
        (ShutdownTrigger(sender), ShutdownSignal(receiver))
    }

    /// Whether a shutdown was asked for.
    pub fn is_requested(&self) -> bool {
        *self.0.borrow()
    }

    /// Wait until a shutdown is asked for, or its trigger is dropped.
    pub async fn requested(&mut self) {
        let _ = self.0.wait_for(|requested| *requested).await;
    }
}

impl ShutdownTrigger {
    /// Ask for the shutdown.
    pub fn trigger(&self) {
        self.0.send_replace(true);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_have_the_reference_names_and_bodies() {
        let started = ForgerEvent::Started {
            active_delegates: vec!["02aa".into()],
        };
        assert_eq!(started.name(), "forger.started");
        assert_eq!(started.body(), json!({"activeDelegates": ["02aa"]}));
        let failed = ForgerEvent::Failed {
            error: "boom".into(),
        };
        assert_eq!(failed.name(), "forger.failed");
        assert_eq!(failed.body(), json!({"error": "boom"}));
        let block = ForgerEvent::BlockForged {
            header: json!({"height": 2}),
        };
        assert_eq!(
            (block.name(), block.body()),
            ("block.forged", json!({"height": 2}))
        );
        let transaction = ForgerEvent::TransactionForged {
            transaction: json!({"id": "ff"}),
        };
        assert_eq!(transaction.name(), "transaction.forged");
    }

    #[tokio::test(start_paused = true)]
    async fn a_shutdown_reaches_every_clone_and_a_deadline_runs_out() {
        let (trigger, signal) = ShutdownSignal::new();
        let mut clone = signal.clone();
        assert!(!signal.is_requested());
        let deadline = Deadline::after(Duration::from_secs(2));
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert_eq!(deadline.remaining(), Duration::from_millis(1_500));
        trigger.trigger();
        clone.requested().await;
        assert!(signal.is_requested());
        tokio::time::sleep(Duration::from_secs(2)).await;
        assert_eq!(deadline.remaining(), Duration::ZERO);
    }
}
