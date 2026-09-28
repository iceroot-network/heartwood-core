//! The forger's loop, as the reference's `ForgerService` runs it, driven by its relay.
//!
//! - **Boot.** The delegates are deduplicated by public key in file order. The first round is
//!   loaded; when no relay answers, "Waiting for a responsive host" is logged and the check runs
//!   after 2 s, else at once.
//! - **The slot check** (`checkSlot`), at every slot: load the round (at the first load the active
//!   and inactive delegate lines, `forger.started` and "Forger Manager started"; later, the lines of
//!   delegates that entered or left the active positions). Before the epoch, wait out the slot. When
//!   the slot's forger is not one of ours: if the next one is, log "Delegate ... is due to forge in
//!   ..." and ask the relay to sync; wait until the slot's end by the forger's own clock. When it is
//!   ours: reset the error counter, start the slot's attempts on their own task, and check again at
//!   the slot's end. A relay error goes to the graceful error log ("Waiting for relay to become
//!   ready" once while the relay answers "App is not ready"); any other error sends
//!   `forger.failed`; the next check is in 2 s. A computed wait of zero or less waits
//!   [`POLL_FLOOR`] where the reference polls again at once.
//! - **An attempt** (`forgeNewBlock`): the debug delay; the network state (logged by the relay on
//!   the first attempt); the round's slot; on the first attempt only, the pool's transactions; the
//!   block's inputs, from which the block must be made (unsigned: a block the reference cannot
//!   make errors its attempt here); the time left; the relay's status and slots; the two early
//!   stops with the "already forged on another node" line; the height line; the quorum's reading;
//!   then, with at least 2,000 ms left in the round's slot and the delegate still its forger, the
//!   block from the signer ([`BlockSigner`], which signs a slot once and re-sends it after),
//!   "Delegate ... forged a new block ...", `postBlock` and the events; else the two "Failed to
//!   forge new block" lines. Any error marks the attempt errored. After 1,000 ms the round is loaded again, and while fewer
//!   than `blockTime - 1` attempts in a row errored the next attempt runs. The reference signs
//!   before its checks; Heartwood signs the same bytes after them, through the signing record.
//! - **The quorum's reading** (`isForgingAllowed`): unknown, cold start and below the minimum
//!   refuse with the reference's lines; the distinct over-height headers are counted; an
//!   over-height header by the delegate's own key whose signature verifies refuses ("already forged
//!   a block"; the reference verifies the header as a whole block, which only an empty block
//!   passes); a quorum that cannot forge refuses with the debug dump.
//! - **Shutdown.** Scheduling stops and the relay calls in flight are dropped, but a `postBlock` in
//!   flight runs to its deadline; then the sessions close.

use std::collections::HashMap;
use std::future::{Future, poll_fn};
use std::pin::pin;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use heartwood_crypto::blocks::factory;
use heartwood_crypto::crypto::slots;
use heartwood_crypto::utils::js;
use heartwood_crypto::{Block, BlockId, Milestones, PublicKeyBytes, Transaction};
use heartwood_kernel::Clock;
use heartwood_kernel::errors::BootError;
use heartwood_kernel::utils::pluralise::pluralise;
use heartwood_kernel::{debug, info, warning};
use heartwood_p2p_wire::RouteId;
use heartwood_p2p_wire::internal::{
    CurrentRound, DelegateWallet, InternalBody, NetworkState, NetworkStateStatus,
};
use serde_json::Value;
use tokio::task::JoinSet;

use crate::deadlines::{self, POLL_FLOOR};
use crate::delegate::Delegate;
use crate::errors::RelayError;
use crate::options::Options;
use crate::seams::{BlockSigner, Deadline, ForgeInputs, ForgerEvent, RelayLink, ShutdownSignal};

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

/// How one attempt of a slot ended ([`ForgerService::forge_attempt`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttemptEnd {
    /// The block was signed and posted.
    Forged,
    /// The slot's last block is already in the current slot, or the delegate is no longer the
    /// slot's forger: the slot's attempts end.
    Stopped,
    /// The quorum's reading refused.
    Refused,
    /// Less than 2,000 ms were left, or the relay was already in the next slot.
    Late,
    /// The block would be in the slot of the relay's last block.
    SameSlot,
    /// A relay call failed, or the relay's answers could not make a block.
    Errored,
}

/// The forger's loop.
pub struct ForgerService<L, S> {
    inner: Arc<Inner<L, S>>,
}

struct Inner<L, S> {
    link: Arc<L>,
    signer: Mutex<S>,
    /// The delegates, deduplicated, with their keys in hex as the relay writes them.
    delegates: Vec<(PublicKeyBytes, String)>,
    delay: Duration,
    milestones: Milestones,
    clock: Arc<dyn Clock>,
    epoch_ms: i64,
    /// The block time (also in milliseconds) and the round length, which the milestones fix from
    /// height 1.
    block_time: u64,
    slot_ms: i64,
    active_delegates: u32,
    state: Mutex<State>,
}

/// What the reference's service keeps between its checks and attempts.
#[derive(Default)]
struct State {
    round: Option<CurrentRound>,
    /// Usernames by public key: of the round's active delegates, and of every delegate.
    usernames: HashMap<String, String>,
    all_usernames: HashMap<String, String>,
    active: Vec<String>,
    inactive: Vec<String>,
    initialised: bool,
    error_count: u64,
    log_app_ready: bool,
    /// The slot's transactions, fetched by its first attempt.
    transactions: Vec<Transaction>,
    first_check: Duration,
}

/// Why a check or an attempt failed: a relay's error, logged gracefully, or any other, whose
/// message a failed check sends as `forger.failed`.
enum Failure {
    Relay(RelayError),
    Other(String),
}

impl From<RelayError> for Failure {
    fn from(error: RelayError) -> Failure {
        Failure::Relay(error)
    }
}

/// What the checks of an attempt concluded: an end, or a block to sign and post.
enum Prepared {
    Ended(AttemptEnd),
    Ready(Ready),
}

/// A block's inputs, checked for the slot of the round.
struct Ready {
    pretty: String,
    transactions: Vec<Transaction>,
    previous_id: BlockId,
    previous_height: u32,
    timestamp: u32,
    reward: u64,
}

/// The reference's `TypeError` where it reads a forger of a round that has none there.
fn no_forger() -> Failure {
    Failure::Other("Cannot read properties of undefined (reading 'publicKey')".into())
}

/// The reference's `assert.defined` failure.
fn undefined() -> Failure {
    Failure::Other(r#"Expected value which is "non-null and non-undefined""#.into())
}

impl<L, S> ForgerService<L, S> {
    /// The service over `parts`.
    pub fn new(parts: ForgerParts<L, S>) -> ForgerService<L, S> {
        let mut delegates: Vec<(PublicKeyBytes, String)> = Vec::new();
        for key in parts.delegates {
            if !delegates.iter().any(|(known, _)| *known == key) {
                delegates.push((key, key.to_hex()));
            }
        }
        let first = parts.milestones.at(1);
        let inner = Inner {
            epoch_ms: slots::epoch_unix_ms(first.epoch()).unwrap_or_default(),
            block_time: first.block_time(),
            slot_ms: i64::try_from(first.block_time().saturating_mul(1_000)).unwrap_or(i64::MAX),
            active_delegates: u32::try_from(first.active_delegates()).unwrap_or(u32::MAX),
            link: parts.link,
            signer: Mutex::new(parts.signer),
            delegates,
            delay: parts.options.delay,
            milestones: parts.milestones,
            clock: parts.clock,
            state: Mutex::new(State {
                log_app_ready: true,
                ..State::default()
            }),
        };
        ForgerService {
            inner: Arc::new(inner),
        }
    }
}

impl<L, S> ForgerService<L, S>
where
    L: RelayLink + 'static,
    S: BlockSigner + 'static,
{
    /// Boot: load the first round (the reference's `boot`). A relay that does not answer is no
    /// boot failure: the loop waits for it.
    pub async fn boot(&mut self) -> Result<(), BootError> {
        let first_check = match self.inner.load_round().await {
            Ok(_) => Duration::ZERO,
            Err(_) => {
                warning!("Waiting for a responsive host :hourglass_flowing_sand:");
                Duration::from_millis(RETRY_MS)
            }
        };
        self.inner.state().first_check = first_check;
        Ok(())
    }

    /// Run the slot loop until `shutdown` is asked for, then let an in-flight `postBlock` finish up
    /// to its deadline.
    pub async fn run(&self, shutdown: ShutdownSignal) {
        let (mut signal, mut attempts) = (shutdown.clone(), JoinSet::new());
        let mut wait = self.inner.state().first_check;
        while until(&mut signal, tokio::time::sleep(wait)).await.is_some() {
            while attempts.try_join_next().is_some() {}
            let check = self.inner.check_slot(&mut attempts, &shutdown);
            match until(&mut signal, check).await {
                Some(next) => wait = next,
                None => break,
            }
        }
        while attempts.join_next().await.is_some() {}
    }

    /// Whether `delegate` may forge on the network state `state` (the reference's
    /// `isForgingAllowed`), logging its reason when not.
    pub fn is_forging_allowed(&self, state: &NetworkState, delegate: &PublicKeyBytes) -> bool {
        self.inner.is_forging_allowed(state, &delegate.to_hex())
    }

    /// One attempt of `delegate`'s slot on `round`, without the pause and the reload after it: the
    /// loop's step, which the differential harness runs over a scripted relay.
    pub async fn forge_attempt(
        &self,
        delegate: &PublicKeyBytes,
        first_attempt: bool,
        round: &CurrentRound,
    ) -> AttemptEnd {
        let (_trigger, mut never) = ShutdownSignal::new();
        let key = (*delegate, delegate.to_hex());
        let end = self
            .inner
            .attempt(&key, first_attempt, round, &mut never)
            .await;
        end.unwrap_or(AttemptEnd::Errored)
    }

    /// The round last loaded (the reference's `getRound`).
    pub async fn round(&self) -> Option<CurrentRound> {
        self.inner.state().round.clone()
    }

    /// Stop scheduling and close the relay sessions (the reference's `dispose`). The loop of
    /// [`ForgerService::run`] has ended before, with its `postBlock` finished.
    pub async fn dispose(&self) {
        self.inner.link.dispose();
    }
}

impl<L, S> Inner<L, S>
where
    L: RelayLink + 'static,
    S: BlockSigner + 'static,
{
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn is_ours(&self, public_key: &str) -> Option<(PublicKeyBytes, String)> {
        (self.delegates.iter())
            .find(|(_, hex)| hex == public_key)
            .cloned()
    }

    /// The milliseconds left in the round's slot by the forger's clock (negative once it ended).
    fn time_left(&self, round: &CurrentRound) -> i64 {
        let start = self
            .epoch_ms
            .saturating_add(round.timestamp.saturating_mul(1_000));
        start
            .saturating_add(self.slot_ms)
            .saturating_sub(self.clock.now_ms())
    }

    /// The wait until the round's slot ends, at least [`POLL_FLOOR`].
    fn until_slot_end(&self, round: &CurrentRound) -> Duration {
        let left = u64::try_from(self.time_left(round)).unwrap_or_default();
        Duration::from_millis(left).max(POLL_FLOOR)
    }

    fn deadline(&self, route: RouteId) -> Deadline {
        let now = self.clock.now_ms();
        let next_end = deadlines::to_next_slot_end(now, self.epoch_ms, self.block_time);
        deadlines::deadline(route, next_end)
    }

    /// The reference's graceful error log.
    fn graceful(&self, error: &RelayError) {
        let text = error.to_string();
        if !text.contains("blockchain isn't ready") && !text.contains("App is not ready") {
            warning!("{text}");
        } else if std::mem::replace(&mut self.state().log_app_ready, false) {
            info!("Waiting for relay to become ready :hourglass_flowing_sand:");
        }
    }

    /// Send `event`, as the reference does, without waiting for the relay: its synchronous part
    /// (the local host rule and its line) runs now, the request after.
    fn emit(&self, event: ForgerEvent) {
        let link = self.link.clone();
        let mut sending = Box::pin(async move { link.emit_event(event).await });
        let mut context = Context::from_waker(Waker::noop());
        if sending.as_mut().poll(&mut context).is_pending() {
            tokio::spawn(sending);
        }
    }

    /// `loadRound`: the round, the usernames, and the active and inactive delegates with their
    /// lines.
    async fn load_round(&self) -> Result<CurrentRound, Failure> {
        let round = self.link.get_round().await?;
        let mut state = self.state();
        state.round = Some(round.clone());
        state.all_usernames = usernames(&round.all_delegates);
        state.usernames = usernames(&round.delegates);
        let keys = self.delegates.iter().map(|(_, hex)| hex.clone());
        let (active, inactive): (Vec<String>, Vec<String>) =
            keys.partition(|hex| state.usernames.contains_key(hex));
        let old_active = std::mem::replace(&mut state.active, active);
        let old_inactive = std::mem::replace(&mut state.inactive, inactive);
        let first = !std::mem::replace(&mut state.initialised, true);
        let named = |hex: &String| name(&state.usernames, hex);
        let registered = |hex: &String| {
            let known = state.all_usernames.get(hex).filter(|name| !name.is_empty());
            known.cloned()
        };
        let mut lines = Vec::new();
        if first {
            if !state.active.is_empty() {
                let names: Vec<String> = state.active.iter().map(named).collect();
                let count = pluralise("active delegate", len(&names), true);
                lines.push(format!("Loaded {count}: {}", names.join(", ")));
            }
            if self.delegates.len() > state.active.len() {
                let names: Vec<String> = (state.inactive.iter())
                    .map(|hex| {
                        let unregistered = || format!("unregistered delegate (public key: {hex})");
                        registered(hex).unwrap_or_else(unregistered)
                    })
                    .collect();
                let count = pluralise("inactive delegate", len(&names), true);
                lines.push(format!("Loaded {count}: {}", names.join(", ")));
            }
        } else {
            let entered: Vec<String> = (state.active.iter())
                .filter(|hex| !old_active.contains(hex))
                .map(named)
                .collect();
            let left: Vec<String> = (state.inactive.iter())
                .filter(|hex| !old_inactive.contains(hex))
                .map(|hex| registered(hex).unwrap_or_else(|| hex.clone()))
                .collect();
            let entering = [
                "is now in an active forging position",
                "are now in active forging positions",
            ];
            let leaving = [
                "is no longer in an active forging position",
                "are no longer in active forging positions",
            ];
            lines.extend(moved(&entered, entering, ":tada:"));
            lines.extend(moved(&left, leaving, ":cry:"));
        }
        drop(state);
        lines.iter().for_each(|line| info!("{line}"));
        if first {
            let active_delegates = self.delegates.iter().map(|(_, hex)| hex.clone()).collect();
            self.emit(ForgerEvent::Started { active_delegates });
            info!("Forger Manager started :hammer:");
        }
        Ok(round)
    }

    /// `checkSlot`, giving the wait before the next check.
    async fn check_slot(
        self: &Arc<Self>,
        attempts: &mut JoinSet<()>,
        shutdown: &ShutdownSignal,
    ) -> Duration {
        let checked = self.slot(attempts, shutdown).await;
        if let Err(Failure::Other(error)) = &checked {
            let error = error.clone();
            self.emit(ForgerEvent::Failed { error });
        }
        self.failed(&checked);
        checked.unwrap_or(Duration::from_millis(RETRY_MS))
    }

    async fn slot(
        self: &Arc<Self>,
        attempts: &mut JoinSet<()>,
        shutdown: &ShutdownSignal,
    ) -> Result<Duration, Failure> {
        let round = self.load_round().await?;
        if round.timestamp < 0 {
            return Ok(self.until_slot_end(&round));
        }
        let current = round.current_forger.as_ref().ok_or_else(no_forger)?;
        let Some(key) = self.is_ours(&current.public_key) else {
            let next = round.next_forger.as_ref().ok_or_else(no_forger)?;
            if self.is_ours(&next.public_key).is_some() {
                let username = name(&self.state().usernames, &next.public_key);
                let seconds = pluralise("second", i128::from(self.block_time), true);
                info!("Delegate {username} is due to forge in {seconds} from now :sparkles:");
                self.link.sync_with_network().await?;
            }
            return Ok(self.until_slot_end(&round));
        };
        self.state().error_count = 0;
        if !self.delay.is_zero() {
            tokio::time::sleep(self.delay).await;
        }
        attempts.spawn(self.clone().attempts(key, round.clone(), shutdown.clone()));
        self.state().log_app_ready = true;
        Ok(self.until_slot_end(&round))
    }

    /// The attempts of a slot (the reference's `forgeNewBlock` and its re-runs), the first one
    /// after the debug delay that the slot check waited.
    async fn attempts(
        self: Arc<Self>,
        key: (PublicKeyBytes, String),
        mut round: CurrentRound,
        mut shutdown: ShutdownSignal,
    ) {
        let mut first = true;
        loop {
            let Some(end) = self.attempt(&key, first, &round, &mut shutdown).await else {
                return;
            };
            if end == AttemptEnd::Stopped {
                return;
            }
            let pause = tokio::time::sleep(Duration::from_millis(ATTEMPT_PAUSE_MS));
            if shutdown.is_requested() || until(&mut shutdown, pause).await.is_none() {
                return;
            }
            let Some(reloaded) = until(&mut shutdown, self.load_round()).await else {
                return;
            };
            // Both failures are logged, as the reference's two catch blocks log them.
            let reload_failed = self.failed(&reloaded);
            let errored = end == AttemptEnd::Errored || reload_failed;
            let next = {
                let mut state = self.state();
                state.error_count = match errored {
                    true => state.error_count.saturating_add(1),
                    false => 0,
                };
                let again = state.error_count < self.block_time.saturating_sub(1);
                again.then(|| state.round.clone())
            };
            let Some(next) = next else {
                return;
            };
            round = next.unwrap_or(round);
            first = false;
            let delay = tokio::time::sleep(self.delay);
            if !self.delay.is_zero() && until(&mut shutdown, delay).await.is_none() {
                return;
            }
        }
    }

    /// One attempt to its end, its post never cut short: `None` when the shutdown stopped it.
    async fn attempt(
        &self,
        key: &(PublicKeyBytes, String),
        first: bool,
        round: &CurrentRound,
        shutdown: &mut ShutdownSignal,
    ) -> Option<AttemptEnd> {
        let end = match until(shutdown, self.prepare(key, first, round)).await? {
            Ok(Prepared::Ready(ready)) => self.forge(key, ready).await,
            Ok(Prepared::Ended(end)) => Ok(end),
            Err(failure) => Err(failure),
        };
        self.failed(&end);
        Some(end.unwrap_or(AttemptEnd::Errored))
    }

    /// Whether `result` failed, a relay's error logged gracefully.
    fn failed<T>(&self, result: &Result<T, Failure>) -> bool {
        if let Err(Failure::Relay(error)) = result {
            self.graceful(error);
        }
        result.is_err()
    }

    /// The checks of an attempt, on the relay's answers.
    async fn prepare(
        &self,
        key: &(PublicKeyBytes, String),
        first: bool,
        round: &CurrentRound,
    ) -> Result<Prepared, Failure> {
        let hex = key.1.as_str();
        let deadline = |route| self.deadline(route);
        let state = (self.link)
            .get_network_state(first, deadline(RouteId::GetNetworkState))
            .await?;
        let height = state.node_height.ok_or_else(undefined)?;
        let slot_of = |timestamp| {
            self.link
                .get_slot_number(timestamp, deadline(RouteId::GetSlotNumber))
        };
        let round_slot = slot_of(Some(round.timestamp)).await?;
        if first {
            let transactions = self.transactions(height).await?;
            self.state().transactions = transactions;
        }
        let transactions = self.state().transactions.clone();
        let previous_id = (state.last_block_id.as_deref())
            .and_then(|id| BlockId::from_hex(id).ok())
            .ok_or_else(undefined)?;
        let block = (u32::try_from(round.timestamp).ok())
            .zip(round.reward.parse::<u64>().ok())
            .ok_or_else(undefined)?;
        // The reference makes its block here; one it cannot make ends the attempt before the
        // checks. Heartwood signs only after them.
        let inputs = ForgeInputs {
            transactions: &transactions,
            previous_id,
            previous_height: height,
            timestamp: block.0,
            reward: block.1,
        };
        let made = Delegate::check(&key.0, &inputs, &self.milestones);
        made.map_err(|error| Failure::Other(error.to_string()))?;
        let time_left = self.time_left(round);
        let pretty = name(&self.state().usernames, hex);
        let status = self.link.get_status(deadline(RouteId::GetStatus)).await?;
        let header = (status.message.state.as_ref()).and_then(|state| state.header.as_ref());
        let current_slot = slot_of(None).await?;
        let header_time = header.and_then(|header| header.timestamp).map(i64::from);
        let last_block_slot = slot_of(header_time).await?;
        let is_current = || {
            let current = round.current_forger.as_ref().ok_or_else(no_forger)?;
            Ok::<bool, Failure>(current.public_key == hex)
        };
        if last_block_slot == current_slot || !is_current()? {
            let generator = header.and_then(|header| header.generator_public_key.as_deref());
            let by_state = state.last_generator.as_deref() == Some(hex)
                && state.last_slot_number == Some(round_slot);
            let by_status = generator == Some(hex) && last_block_slot == round_slot;
            if first && (by_state || by_status) {
                warning!(
                    "Not going to forge because delegate {pretty} has already forged on another node :octagonal_sign:"
                );
            }
            return Ok(Prepared::Ended(AttemptEnd::Stopped));
        }
        if height != round.last_block.height {
            let (ours, theirs) = (locale(height), locale(round.last_block.height));
            warning!(
                "The NetworkState height ({ours}) and round height ({theirs}) are out of sync. This indicates delayed blocks on the network :zzz:"
            );
        }
        if !self.is_forging_allowed(&state, hex) {
            return Ok(Prepared::Ended(AttemptEnd::Refused));
        }
        if time_left >= MINIMUM_MS && current_slot == round_slot && is_current()? {
            if slot_of(Some(round.timestamp)).await? == last_block_slot {
                return Ok(Prepared::Ended(AttemptEnd::SameSlot));
            }
            return Ok(Prepared::Ready(Ready {
                pretty,
                transactions,
                previous_id,
                previous_height: height,
                timestamp: block.0,
                reward: block.1,
            }));
        }
        if time_left > 0 {
            warning!(
                "Failed to forge new block by delegate {pretty}, because there were {time_left}ms left in the current slot (less than {MINIMUM_MS}ms) :bangbang:"
            );
        } else {
            warning!(
                "Failed to forge new block by delegate {pretty}, because already in next slot :bangbang:"
            );
        }
        Ok(Prepared::Ended(AttemptEnd::Late))
    }

    /// `getTransactionsForForging`: the pool's transactions, read as `fromBytesUnsafe` reads them.
    async fn transactions(&self, height: u32) -> Result<Vec<Transaction>, Failure> {
        let deadline = self.deadline(RouteId::GetUnconfirmedTransactions);
        let Some(answer) = self.link.get_transactions(deadline).await? else {
            warning!("Could not get unconfirmed transactions from pool :warning:");
            return Ok(Vec::new());
        };
        let params = self.milestones.at(height);
        let read = (answer.transactions.iter())
            .map(|bytes| Transaction::decode_trusted(bytes, params))
            .collect::<Result<Vec<_>, _>>();
        let transactions = read.map_err(|error| Failure::Other(error.to_string()))?;
        let received = pluralise("transaction", len(&transactions), true);
        let total = pluralise("transaction", i128::from(answer.pool_size), true);
        debug!("Received {received} from the pool containing {total} total :money_with_wings:");
        Ok(transactions)
    }

    /// Sign (or re-send) the slot's block, post it and send the events. The block's slot is the
    /// slot of its own timestamp, which the signing record is keyed by and checks against the
    /// forger's clock, whatever slot the relay answered for it. The post is never cut short by a
    /// shutdown: it runs to its deadline.
    async fn forge(
        &self,
        key: &(PublicKeyBytes, String),
        ready: Ready,
    ) -> Result<AttemptEnd, Failure> {
        let now = slots::epoch_time(self.clock.now_ms(), self.epoch_ms);
        let own_slot = slots::slot_number(now, self.block_time);
        let slot = slots::slot_number(i64::from(ready.timestamp), self.block_time);
        let inputs = ForgeInputs {
            transactions: &ready.transactions,
            previous_id: ready.previous_id,
            previous_height: ready.previous_height,
            timestamp: ready.timestamp,
            reward: ready.reward,
        };
        let outcome = {
            let mut signer = self.signer.lock().unwrap_or_else(PoisonError::into_inner);
            signer.sign_or_resend(&key.0, &inputs, own_slot, slot)
        };
        let signed = outcome.map_err(|error| Failure::Other(error.to_string()))?;
        let signed = signed.signed();
        let (pretty, height) = (&ready.pretty, locale(signed.height));
        let count = pluralise("transaction", len(&ready.transactions), true);
        info!("Delegate {pretty} forged a new block at height {height} with {count} :trident:");
        let deadline = self.deadline(RouteId::PostBlock);
        self.link.broadcast_block(&signed.bytes, deadline).await;
        if let Ok(block) = Block::decode_trusted(&signed.bytes, &self.milestones) {
            let mut header = block.to_json(&self.milestones);
            if let Value::Object(fields) = &mut header {
                fields.shift_remove("transactions");
            }
            self.emit(ForgerEvent::BlockForged { header });
        }
        let params = self.milestones.at(signed.height);
        for transaction in &ready.transactions {
            let transaction = transaction.to_json(params);
            self.emit(ForgerEvent::TransactionForged { transaction });
        }
        Ok(AttemptEnd::Forged)
    }

    /// `isForgingAllowed`, for the delegate whose key is `hex`.
    fn is_forging_allowed(&self, state: &NetworkState, hex: &str) -> bool {
        let refusal = match state.status {
            NetworkStateStatus::Unknown => "Failed to get network state from client",
            NetworkStateStatus::ColdStart => "Skipping slot because of cold start",
            NetworkStateStatus::BelowMinimumDelegates => {
                "Not peered with enough delegates to get quorum"
            }
            _ => "",
        };
        if !refusal.is_empty() {
            info!("{refusal}. Will not forge :bomb:");
            return false;
        }
        let headers: Vec<&Value> =
            (state.quorum_details.over_height_block_headers.values()).collect();
        let distinct = distinct(&headers);
        if !distinct.is_empty() {
            let detected = pluralise("distinct overheight block header", len(&distinct), true);
            info!("Detected {detected} :zzz:");
            for header in &headers {
                let generator = header.get("generatorPublicKey").and_then(Value::as_str);
                let verified = factory::from_json(header, &self.milestones)
                    .is_ok_and(|block| block.verify_signature());
                if generator == Some(hex) && verified {
                    let username = name(&self.state().usernames, hex);
                    warning!("Delegate {username} already forged a block. Will not forge :bomb:");
                    return false;
                }
            }
        }
        if !state.can_forge(self.active_delegates) {
            info!("Not enough quorum to forge next block. Will not forge :bomb:");
            debug!("Network State: {}", dump(state, distinct));
            return false;
        }
        true
    }
}

/// Run `work` until it ends, or until the shutdown is asked for (`None`).
async fn until<T>(shutdown: &mut ShutdownSignal, work: impl Future<Output = T>) -> Option<T> {
    let (mut work, mut stop) = (pin!(work), pin!(shutdown.requested()));
    poll_fn(|context| match work.as_mut().poll(context) {
        Poll::Ready(value) => Poll::Ready(Some(value)),
        Poll::Pending => stop.as_mut().poll(context).map(|()| None),
    })
    .await
}

/// A username as the reference reads it from its object of names: `undefined` when missing.
fn name(names: &HashMap<String, String>, hex: &str) -> String {
    (names.get(hex).cloned()).unwrap_or_else(|| "undefined".into())
}

/// The usernames of `wallets` by public key, the last one of a key kept.
fn usernames(wallets: &[DelegateWallet]) -> HashMap<String, String> {
    (wallets
        .iter()
        .map(|w| (w.public_key.clone(), w.username.clone())))
    .collect()
}

/// The line of delegates that entered or left the active positions: `[one, many]` with `emoji`.
fn moved(names: &[String], [one, many]: [&str; 2], emoji: &str) -> Option<String> {
    match names {
        [] => None,
        [name] => Some(format!("Delegate {name} {one} {emoji}")),
        _ => Some(format!("Delegates {} {many} {emoji}", and_joined(names))),
    }
}

fn len<T>(items: &[T]) -> i128 {
    i128::try_from(items.len()).unwrap_or(i128::MAX)
}

/// A count as `toLocaleString` writes it in English.
fn locale(count: u32) -> String {
    js::to_locale_integer(i128::from(count))
}

/// `names` joined with commas, the last comma replaced by " and", as the reference's pattern
/// replaces it.
fn and_joined(names: &[String]) -> String {
    let joined = names.join(", ");
    match joined.rsplit_once(',') {
        Some((before, after)) => format!("{before} and{after}"),
        None => joined,
    }
}

/// The over-height headers sorted by id, a header dropped when its id is the one before it (the
/// first one too when it has no id, as the reference's `lastSeen` starts unset).
fn distinct(headers: &[&Value]) -> Vec<Value> {
    let id = |header: &Value| header.get("id").and_then(Value::as_str).map(str::to_owned);
    let mut sorted = headers.to_vec();
    sorted.sort_by_key(|header| id(header));
    let mut last_seen = None;
    let mut kept = Vec::new();
    for header in sorted {
        let seen = id(header);
        if seen != last_seen {
            kept.push(header.clone());
        }
        last_seen = seen;
    }
    kept
}

/// The reference's `networkState.toJson()` after the distinct headers replaced the over-height
/// ones: the quorum, then the state without its status, with two spaces of indentation.
fn dump(state: &NetworkState, distinct: Vec<Value>) -> String {
    let mut json = state.to_json();
    let mut dumped = serde_json::Map::new();
    dumped.insert("quorum".into(), Value::from(state.quorum()));
    if let Value::Object(fields) = &mut json {
        fields.shift_remove("status");
        if let Some(details) = (fields.get_mut("quorumDetails")).and_then(Value::as_object_mut) {
            details.insert("overHeightBlockHeaders".into(), Value::Array(distinct));
        }
        dumped.extend(std::mem::take(fields));
    }
    js::stringify(&Value::Object(dumped), 2)
}
