//! The forger's loop, as the reference's `ForgerService` runs it, driven by its relay.
//!
//! - **Boot.** The delegates are deduplicated by public key in file order. The first round is
//!   loaded; when no relay answers, "Waiting for a responsive host" is logged and the check runs
//!   again after 2 s.
//! - **The slot check** (`checkSlot`), at every slot: load the round (at the first load the active
//!   and inactive delegate lines, `forger.started` and "Forger Manager started"; later, the lines of
//!   delegates that entered or left the active positions). Before the epoch, wait out the slot. When
//!   the slot's forger is not one of ours: if the next one is, log "Delegate ... is due to forge in
//!   ..." and ask the relay to sync; wait until the slot's end by the forger's own clock. When it is
//!   ours: reset the error counter, start the slot's attempts, and check again at the slot's end.
//!   A relay error goes to the graceful error log ("Waiting for relay to become ready" once while
//!   the relay answers "App is not ready"); any other error sends `forger.failed`; the next check
//!   is in 2 s. A computed wait of zero or less waits [`crate::deadlines::POLL_FLOOR`].
//! - **An attempt** (`forgeNewBlock`): the debug delay; the network state (logged by the relay on
//!   the first attempt); the round's slot; on the first attempt only, the pool's transactions; the
//!   block's inputs; the time left; the relay's status and slots; the two early stops with the
//!   "already forged on another node" line; the height line; the quorum's reading; then, with at
//!   least 2,000 ms left in the round's slot and the delegate still its forger, the block from the
//!   signer ([`BlockSigner`], which signs a slot once and re-sends it after), "Delegate ... forged a
//!   new block ...", `postBlock` and the events; else the two "Failed to forge new block" lines. Any
//!   error marks the attempt errored. After 1,000 ms the round is loaded again, and while fewer
//!   than `blockTime - 1` attempts in a row errored the next attempt runs.
//! - **The quorum's reading** (`isForgingAllowed`): unknown, cold start and below the minimum
//!   refuse with the reference's lines; the distinct over-height headers are counted; an
//!   over-height header by the delegate's own key whose signature verifies refuses ("already forged
//!   a block"); a quorum that cannot forge refuses with the debug dump.
//! - **Shutdown.** Scheduling stops; an in-flight `postBlock` runs to its deadline; the sessions
//!   close.

use std::sync::Arc;

use heartwood_crypto::{Milestones, PublicKeyBytes};
use heartwood_kernel::Clock;
use heartwood_kernel::errors::BootError;
use heartwood_p2p_wire::internal::{CurrentRound, NetworkState};
use tokio::sync::Mutex;

use crate::options::Options;
use crate::seams::{BlockSigner, RelayLink, ShutdownSignal};

/// The margin the forger keeps before a slot's end: a block is not sent with less time left.
pub const MINIMUM_MS: i64 = 2_000;
/// The wait after a failed slot check.
pub const RETRY_MS: u64 = 2_000;
/// The wait between two attempts in a slot.
pub const ATTEMPT_PAUSE_MS: u64 = 1_000;

/// What a forger service is made of.
pub struct ForgerParts<L, S> {
    /// The link to the relays.
    pub link: Arc<L>,
    /// The block signer.
    pub signer: S,
    /// The delegates' public keys, in the key file's order; the service deduplicates them.
    pub delegates: Vec<PublicKeyBytes>,
    /// The component's options (the debug delay).
    pub options: Options,
    /// The forger's own milestones: its epoch and block times.
    pub milestones: Milestones,
    /// The wall clock.
    pub clock: Arc<dyn Clock>,
}

/// The forger's loop.
pub struct ForgerService<L, S> {
    #[expect(dead_code, reason = "the loop reads it")]
    link: Arc<L>,
    #[expect(dead_code, reason = "the loop reads it")]
    signer: Mutex<S>,
    #[expect(dead_code, reason = "the loop reads it")]
    delegates: Vec<PublicKeyBytes>,
    #[expect(dead_code, reason = "the loop reads it")]
    options: Options,
    #[expect(dead_code, reason = "the loop reads it")]
    milestones: Milestones,
    #[expect(dead_code, reason = "the loop reads it")]
    clock: Arc<dyn Clock>,
}

impl<L, S> ForgerService<L, S> {
    /// The service over `parts`.
    pub fn new(parts: ForgerParts<L, S>) -> ForgerService<L, S> {
        let ForgerParts {
            link,
            signer,
            delegates,
            options,
            milestones,
            clock,
        } = parts;
        ForgerService {
            link,
            signer: Mutex::new(signer),
            delegates,
            options,
            milestones,
            clock,
        }
    }
}

impl<L, S> ForgerService<L, S>
where
    L: RelayLink + 'static,
    S: BlockSigner + 'static,
{
    /// Boot: deduplicate the delegates and load the first round (the reference's `boot`). A relay
    /// that does not answer is no boot failure: the loop waits for it.
    pub async fn boot(&mut self) -> Result<(), BootError> {
        todo!("the forger's boot")
    }

    /// Run the slot loop until `shutdown` is asked for, then let an in-flight `postBlock` finish up
    /// to its deadline.
    pub async fn run(&self, shutdown: ShutdownSignal) {
        let _ = shutdown;
        todo!("the slot loop")
    }

    /// Whether `delegate` may forge on the network state `state` (the reference's
    /// `isForgingAllowed`), logging its reason when not.
    pub fn is_forging_allowed(&self, state: &NetworkState, delegate: &PublicKeyBytes) -> bool {
        let _ = (state, delegate);
        todo!("isForgingAllowed")
    }

    /// The round last loaded (the reference's `getRound`).
    pub async fn round(&self) -> Option<CurrentRound> {
        todo!("the loaded round")
    }

    /// Stop scheduling and close the relay sessions (the reference's `dispose`).
    pub async fn dispose(&self) {
        todo!("the forger's dispose")
    }
}
