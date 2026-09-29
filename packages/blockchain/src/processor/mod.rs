//! The block processor: a block's checks in the reference's order, and each handler's outcome
//! and side effects.
//!
//! 1. The block's verification at the current time (its transactions strictly decoded by the
//!    rebuild): `Rejected`.
//! 2. A sender's nonces not consecutive from its wallet's nonce: `Rejected`.
//! 3. The generator: the delegate of the block's key, registered, ranked, and the delegate of the
//!    slot's index in the round's list.
//! 4. Not chained to the tip: the unchained statuses ([`unchained`]).
//! 5. An invalid generator: `Rejected`.
//! 6. A reward other than the round's reward for the block: `Rejected`.
//! 7. Two transactions of one unique type from one sender: `Rejected`.
//! 8. A transaction id already stored, or in a block of this job not stored yet: `Rejected`.
//! 9. The apply ([`handlers::accept`]): the state applies the block with its missed slots; the
//!    pool removes the forged transactions; `Accepted`. A refused apply leaves the state as it was:
//!    `Rejected`.
//!
//! Every refusing handler resets the last downloaded block; the unchained handler also asks for
//! the queue to be cleared. The processor runs on the node thread and never reads the wall clock
//! except through [`ProcessContext::clock`].
//!
//! The block's delegate name is the one the reference's `setBlockUsername` gave it when it was
//! enqueued ([`crate::work::IncomingBlock::set_username`]): the job passes it to
//! [`BlockProcess::process`], and [`trace_named`] judges the generator with it. A block whose
//! delegate registered in an earlier block of the same download has no name, so its generator is
//! refused until the block is downloaded again. [`trace`] takes the name the block's key has in
//! the state it is processed on ([`delegate_name`]), which is the enqueued name of every other
//! block.

pub mod handlers;
pub mod unchained;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use heartwood_crypto::TransactionType;
use heartwood_crypto::blocks::{Verification, VerifyFailure};
use heartwood_crypto::crypto::slots;
use heartwood_crypto::utils::js;
use heartwood_crypto::{Address, Block, Milestones, Params, Transaction, TransactionId};
use heartwood_kernel::{
    ChainLink, Clock, calculate_forging_info, calculate_round, is_block_chained,
};
use heartwood_state::State;
use heartwood_state::machine::Applied;
use heartwood_state::round_state::{RoundDelegate, RoundState};
use heartwood_transactions::Handlers;

use crate::chain_state::ChainState;
use crate::processor::handlers::Refusal;

/// What the processor made of a block, by the reference's names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockProcessorResult {
    /// Applied.
    Accepted,
    /// Not applied, and may still be broadcast: a block the node already has.
    DiscardedButCanBeBroadcasted,
    /// Refused.
    Rejected,
    /// A double-forged block of a round delegate: the node rolls back to recover.
    Rollback,
    /// Reverted after a partial apply; the reference's processor never returns it.
    Reverted,
    /// A revert that failed after a partial apply; unreachable with an atomic apply.
    Corrupted,
}

impl BlockProcessorResult {
    /// The reference's name of the result.
    pub const fn name(self) -> &'static str {
        match self {
            BlockProcessorResult::Accepted => "Accepted",
            BlockProcessorResult::DiscardedButCanBeBroadcasted => "DiscardedButCanBeBroadcasted",
            BlockProcessorResult::Rejected => "Rejected",
            BlockProcessorResult::Rollback => "Rollback",
            BlockProcessorResult::Reverted => "Reverted",
            BlockProcessorResult::Corrupted => "Corrupted",
        }
    }
}

/// Why a block does not chain to the tip, as the reference's unchained handler names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnchainedStatus {
    /// Higher than the tip plus one.
    NotReadyToAcceptNewHeight,
    /// Lower than the tip.
    AlreadyInBlockchain,
    /// The tip itself.
    EqualToLastBlock,
    /// At the tip's height plus one or the tip's, by a generator that is not the slot's delegate.
    GeneratorMismatch,
    /// A second block for the tip's slot by its scheduled delegate.
    DoubleForging,
    /// An older timestamp than the tip's.
    InvalidTimestamp,
}

impl UnchainedStatus {
    /// The reference's name of the status.
    pub const fn name(self) -> &'static str {
        match self {
            UnchainedStatus::NotReadyToAcceptNewHeight => "NotReadyToAcceptNewHeight",
            UnchainedStatus::AlreadyInBlockchain => "AlreadyInBlockchain",
            UnchainedStatus::EqualToLastBlock => "EqualToLastBlock",
            UnchainedStatus::GeneratorMismatch => "GeneratorMismatch",
            UnchainedStatus::DoubleForging => "DoubleForging",
            UnchainedStatus::InvalidTimestamp => "InvalidTimestamp",
        }
    }
}

/// The stored transaction ids the processor checks against: read once for all the job's
/// transactions before the job's chunk opens (the node thread is the only writer).
pub trait ForgedIds {
    /// Whether a stored block holds the transaction `id`.
    fn is_stored(&self, id: &TransactionId) -> bool;
}

/// The pool steps of an accepted block.
pub trait PoolSteps {
    /// Remove the forged `transaction` (with its sender's earlier transactions), under the rules
    /// `params`. A storage failure is recorded by the pool's side and stops the node after the
    /// job.
    fn remove_forged(&mut self, transaction: &Transaction, params: &Params);
}

/// What the processor reads and changes.
pub struct ProcessContext<'a> {
    /// The state at the tip.
    pub state: &'a mut State,
    /// The transaction handlers.
    pub handlers: &'a Handlers,
    /// The milestones.
    pub milestones: &'a Milestones,
    /// The chain state.
    pub chain: &'a mut ChainState,
    /// The stored transaction ids.
    pub forged: &'a dyn ForgedIds,
    /// The blocks accepted earlier in this job, not committed yet, in height order.
    pub pending: &'a [Arc<Block>],
    /// The pool.
    pub pool: &'a mut dyn PoolSteps,
    /// The clock of the block's verification and of the accept step.
    pub clock: &'a dyn Clock,
}

/// What processing a block did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Processed {
    /// The result.
    pub result: BlockProcessorResult,
    /// What the apply returned, when the block was applied.
    pub applied: Option<Applied>,
    /// The unchained status, when the block did not chain to the tip.
    pub unchained: Option<UnchainedStatus>,
    /// Whether the block was refused for a fault that no honest peer serves: a verification
    /// failure other than the timestamp, a wrong reward, conflicting transactions, a refused apply,
    /// and, of a block that chains to the tip, a nonce out of order, a generator that is not its
    /// slot's delegate or a key that is no registered delegate's (the shared state decides those,
    /// so no honest node accepted the block). A timestamp ahead of our clock, a transaction
    /// already forged, a block that does not chain and a generator whose name the enqueueing did
    /// not know (registered in an earlier block of the same download) are no fault.
    pub fault: bool,
}

/// Which of the reference's handlers decided a block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Handler {
    /// A refusal before the apply.
    Refused(Refusal),
    /// The unchained handler.
    Unchained,
    /// The accept handler: the apply, which accepted or refused the block.
    Accept,
}

impl Handler {
    /// The class name of the reference's handler.
    pub const fn name(self) -> &'static str {
        match self {
            Handler::Refused(refusal) => refusal.handler_name(),
            Handler::Unchained => "UnchainedHandler",
            Handler::Accept => "AcceptBlockHandler",
        }
    }
}

/// What processing a block did, and which handler decided it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Traced {
    /// What processing the block did.
    pub processed: Processed,
    /// The handler that decided it.
    pub handler: Handler,
}

/// Process `block` with the delegate name its key has in the state (see the module
/// documentation), and name the handler that decided it.
pub fn trace(ctx: &mut ProcessContext<'_>, block: &Block) -> Traced {
    let name = delegate_name(ctx.state, block).map(str::to_owned);
    trace_named(ctx, block, name.as_deref())
}

/// Process `block` (see the module documentation for the order of the checks) with `name` as the
/// block's delegate name: the name its generator key had when the block was enqueued, `None` when
/// the key named no delegate then; and name the handler that decided it.
pub fn trace_named(ctx: &mut ProcessContext<'_>, block: &Block, name: Option<&str>) -> Traced {
    // A block that chains to the tip is judged on the state it was built on, so a refusal by a
    // rule of that state is no honest node's block.
    let tip = ChainLink::of(ctx.state.last_block());
    let chained = is_block_chained(&tip, &ChainLink::of(block), ctx.milestones).is_ok();
    let refused = |ctx: &mut ProcessContext<'_>, refusal, fault: bool| Traced {
        processed: Processed {
            result: handlers::refuse(ctx, block, refusal),
            applied: None,
            unchained: None,
            fault: fault || (chained && delegate_name(ctx.state, block).is_none()),
        },
        handler: Handler::Refused(refusal),
    };
    if let Some(fault) = unverified(ctx, block) {
        return refused(ctx, Refusal::VerificationFailed, fault);
    }
    if has_nonce_out_of_order(ctx.state, ctx.milestones, block) {
        return refused(ctx, Refusal::NonceOutOfOrder, chained);
    }
    let generator_valid = is_valid_generator(ctx.state, ctx.milestones, block, name);
    if is_block_chained(&tip, &ChainLink::of(block), ctx.milestones).is_err() {
        let (result, status) = unchained::handle(ctx, block, name, generator_valid);
        return Traced {
            processed: Processed {
                result,
                applied: None,
                unchained: Some(status),
                fault: false,
            },
            handler: Handler::Unchained,
        };
    }
    // Chained: the round's list on the shared state names the slot's delegate. Only a block whose
    // delegate the enqueueing could not name (registered in an earlier block of the same
    // download) is refused as an honest peer's.
    if !generator_valid {
        return refused(ctx, Refusal::InvalidGenerator, name.is_some());
    }
    if !is_valid_reward(ctx.state, ctx.milestones, block) {
        return refused(ctx, Refusal::InvalidReward, true);
    }
    if has_conflicting_transactions(ctx.milestones, block) {
        return refused(ctx, Refusal::ConflictingTransactions, true);
    }
    if has_forged_transactions(ctx, block) {
        return refused(ctx, Refusal::AlreadyForged, false);
    }
    Traced {
        processed: handlers::accept(ctx, block),
        handler: Handler::Accept,
    }
}

/// The processor as the job calls it: [`BlockProcessor`] in a node, a scripted fake in the
/// runtime's tests.
pub trait BlockProcess: Send + Sync {
    /// Process `block`, whose delegate name is `name`: the one it got when it was enqueued (see
    /// [`trace_named`]).
    fn process(&self, ctx: &mut ProcessContext<'_>, block: &Block, name: Option<&str>)
    -> Processed;
}

/// The block processor.
#[derive(Debug, Clone, Copy, Default)]
pub struct BlockProcessor;

impl BlockProcess for BlockProcessor {
    fn process(
        &self,
        ctx: &mut ProcessContext<'_>,
        block: &Block,
        name: Option<&str>,
    ) -> Processed {
        trace_named(ctx, block, name).processed
    }
}

/// The delegate name of the block's generator key, as the reference's `setBlockUsername` gives it
/// to a block of version 0: the name of the registered delegate that has bound the key.
pub fn delegate_name<'s>(state: &'s State, block: &Block) -> Option<&'s str> {
    if block.header().version != 0 {
        return None;
    }
    state
        .wallets()
        .find_by_public_key(&block.header().generator_public_key)
        .and_then(heartwood_state::Wallet::delegate)
        .map(heartwood_state::Delegate::username)
}

/// Whether `block`, judged without the clock, is one that no honest peer serves: a rule other
/// than the timestamp fails, or it chains to the tip and its generator is not the delegate of its
/// slot in its round's list, which the shared state decides. For a block discarded before it is
/// processed because its slot is in the future by our clock: the reference discards it unjudged,
/// so a peer needed no key to be asked for it again and again ([`crate::process_blocks_job`]).
pub fn no_delegate_forged(state: &State, milestones: &Milestones, block: &Block) -> bool {
    let header = block.header();
    let failures = block.verify(milestones, i64::MAX).failures;
    if failures.iter().any(|f| *f != VerifyFailure::Timestamp) {
        return true;
    }
    let tip = ChainLink::of(state.last_block());
    if is_block_chained(&tip, &ChainLink::of(block), milestones).is_err() {
        return false;
    }
    let forgers = round_forgers(state, header.height, milestones);
    let info = calculate_forging_info(i64::from(header.timestamp), header.height, milestones);
    let forger = usize::try_from(info.current_forger)
        .ok()
        .and_then(|i| forgers.get(i));
    forger.is_none_or(|forger| forger.public_key != header.generator_public_key)
}

/// The forging list of the round of `height`, as the reference's `getActiveDelegates` reads it:
/// the current forging list for the round of the next block, else the round's saved rows
/// shuffled for the round, which the undo record of the block that ended the round keeps. A
/// later round has no rows: empty.
pub fn round_forgers<'s>(
    state: &'s State,
    height: u32,
    milestones: &Milestones,
) -> &'s [RoundDelegate] {
    let round = calculate_round(height, milestones).round;
    let current = state.round();
    if round == current.info().round {
        return current.forging_delegates();
    }
    if round > current.info().round {
        return &[];
    }
    state
        .journal()
        .iter()
        .rev()
        .filter_map(|undo| undo.round.as_ref())
        .map(|undo| &undo.previous)
        .find(|previous| previous.info().round == round)
        .map_or(&[], RoundState::forging_delegates)
}

/// A height as the reference's `toLocaleString()` writes it in its log lines.
pub(crate) fn locale(height: u32) -> String {
    js::to_locale_integer(i128::from(height))
}

/// The time the block is verified at, in seconds after the network epoch; `None` when the epoch
/// does not read as a date, which the milestone loader never allows.
fn epoch_now(milestones: &Milestones, clock: &dyn Clock) -> Option<i64> {
    let epoch = slots::epoch_unix_ms(milestones.at(1).epoch())?;
    Some(slots::epoch_time(clock.now_ms(), epoch))
}

/// The block's verification at the current time: `None` when it passes, else whether a rule other
/// than the timestamp failed (the timestamp is judged by our clock). A time that cannot be read
/// refuses the block: its timestamp cannot be judged.
fn unverified(ctx: &ProcessContext<'_>, block: &Block) -> Option<bool> {
    let verification =
        epoch_now(ctx.milestones, ctx.clock).map(|now| block.verify(ctx.milestones, now));
    if verification.as_ref().is_some_and(Verification::is_verified) {
        return None;
    }
    heartwood_kernel::warning!(
        "Block {} ({}) disregarded because verification failed :scroll:",
        locale(block.height()),
        block.id()
    );
    let failures = verification.map(|v| v.failures).unwrap_or_default();
    let text: Vec<String> = failures.iter().map(ToString::to_string).collect();
    heartwood_kernel::warning!("{}", text.join("; "));
    let fault = failures.iter().any(|f| *f != VerifyFailure::Timestamp);
    Some(fault)
}

/// Whether a sender's nonces in the block are not consecutive from its wallet's nonce.
fn has_nonce_out_of_order(state: &State, milestones: &Milestones, block: &Block) -> bool {
    let params = milestones.at(block.height());
    let mut nonces: BTreeMap<Address, u64> = BTreeMap::new();
    for transaction in block.transactions() {
        let sender = transaction.data().sender_address(params);
        let preceding = *nonces.entry(sender).or_insert_with(|| {
            state
                .wallets()
                .find_by_address(&sender)
                .map_or(0, heartwood_state::Wallet::nonce)
        });
        let nonce = transaction.data().nonce;
        if preceding.checked_add(1) != Some(nonce) {
            heartwood_kernel::warning!(
                "Block {{ height: {}, id: {} }} not accepted: invalid nonce order for sender {}: preceding nonce: {preceding}, transaction {} has nonce {nonce} :bangbang:",
                locale(block.height()),
                block.id(),
                sender.to_base58(),
                transaction.id()
            );
            return true;
        }
        nonces.insert(sender, nonce);
    }
    false
}

/// Whether the block's generator may forge it: `name` names a registered delegate whose key is
/// the block's, with a rank, and the delegate of the block's slot in the forging list of its
/// round. A slot with no delegate in the list refuses the block.
fn is_valid_generator(
    state: &State,
    milestones: &Milestones,
    block: &Block,
    name: Option<&str>,
) -> bool {
    let header = block.header();
    let Some(username) = name else {
        return false;
    };
    let Some(wallet) = state.wallets().find_by_username(username) else {
        return false;
    };
    let key = wallet
        .public_key()
        .map(heartwood_crypto::PublicKey::as_bytes);
    if header.version == 0 && key != Some(header.generator_public_key.as_bytes().as_slice()) {
        return false;
    }
    if wallet
        .delegate()
        .is_none_or(|delegate| delegate.rank.is_none())
    {
        return false;
    }
    let forgers = round_forgers(state, header.height, milestones);
    let info = calculate_forging_info(i64::from(header.timestamp), header.height, milestones);
    let height = locale(header.height);
    match usize::try_from(info.current_forger)
        .ok()
        .and_then(|index| forgers.get(index))
    {
        None => {
            heartwood_kernel::warning!(
                "Could not decide if delegate {username} is allowed to forge block {height} :grey_question:"
            );
            false
        }
        Some(forger) if forger.username != username => {
            heartwood_kernel::warning!(
                "Delegate {username} is not allowed to forge in this slot, should be {} :-1:",
                forger.username
            );
            false
        }
        Some(_) => {
            heartwood_kernel::debug!("Delegate {username} is allowed to forge block {height} :+1:");
            true
        }
    }
}

/// Whether the block's reward is the reward of its generator in its round. The processor asks
/// only for a valid generator, which is a ranked delegate of an active rank, so the reward is
/// always defined; a reward that cannot be computed refuses the block.
fn is_valid_reward(state: &State, milestones: &Milestones, block: &Block) -> bool {
    let header = block.header();
    match state.reward_for_block_in_round(header.height, &header.generator_public_key, milestones) {
        Ok(reward) if reward.reward == header.reward => true,
        Ok(reward) => {
            heartwood_kernel::warning!(
                "Block rejected as reward was {}, should be {} :bangbang:",
                header.reward,
                reward.reward
            );
            false
        }
        Err(error) => {
            heartwood_kernel::warning!("Block rejected: its reward cannot be computed: {error}");
            false
        }
    }
}

/// The types of which a sender may send one per block, in the reference's registration order.
const UNIQUE_TYPES: [TransactionType; 4] = [
    TransactionType::SecondSignature,
    TransactionType::DelegateRegistration,
    TransactionType::DelegateResignation,
    TransactionType::Vote,
];

/// Whether the block holds two transactions of one unique type from one sender.
fn has_conflicting_transactions(milestones: &Milestones, block: &Block) -> bool {
    let params = milestones.at(block.height());
    for kind in UNIQUE_TYPES {
        let senders: Vec<Address> = block
            .transactions()
            .iter()
            .filter(|transaction| transaction.kind() == kind)
            .map(|transaction| transaction.data().sender_address(params))
            .collect();
        let distinct: BTreeSet<&Address> = senders.iter().collect();
        if distinct.len() != senders.len() {
            heartwood_kernel::warning!(
                "Block {} disregarded, because it contains multiple {} transactions from the same wallet :scroll:",
                locale(block.height()),
                kind.key()
            );
            return true;
        }
    }
    false
}

/// Whether a transaction of the block is stored, or in a block of this job that is not stored
/// yet.
fn has_forged_transactions(ctx: &ProcessContext<'_>, block: &Block) -> bool {
    let mut forged: Vec<TransactionId> = block
        .transaction_ids()
        .filter(|id| ctx.forged.is_stored(id))
        .copied()
        .collect();
    let ids: BTreeSet<&TransactionId> = block.transaction_ids().collect();
    for pending in ctx.pending {
        forged.extend(pending.transaction_ids().filter(|id| ids.contains(id)));
    }
    if forged.is_empty() {
        return false;
    }
    heartwood_kernel::warning!(
        "Block {} disregarded, because it contains already forged transactions :scroll:",
        locale(block.height())
    );
    let listed: Vec<String> = forged.iter().map(TransactionId::to_hex).collect();
    heartwood_kernel::debug!("{}", listed.join(", "));
    true
}

/// Set the last downloaded block to the tip, as the reference's `resetLastDownloadedBlock` does
/// with its last block.
pub(crate) fn reset_last_downloaded(state: &State, chain: &mut ChainState) {
    chain.last_downloaded = Some(ChainLink::of(state.last_block()));
}
