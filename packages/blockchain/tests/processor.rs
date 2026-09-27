//! The block processor on the committed one-node chain, with fakes of the stored ids and of the
//! pool: every result the reference's processor can reach, which handler decides it, and what each
//! handler changes (the state, the last downloaded block, the forked block, the wake-up time and
//! the pool).
//!
//! Blocks are built on the chain's tip and signed with the keys the chain's public seed derives,
//! as the synthetic cases of the harness build them.

mod common;

use std::collections::BTreeSet;
use std::sync::{Arc, LazyLock};

use common::Chain;
use heartwood_blockchain::ChainState;
use heartwood_blockchain::processor::handlers::Refusal;
use heartwood_blockchain::processor::{
    self, BlockProcessorResult, ForgedIds, Handler, PoolSteps, ProcessContext, Traced,
    UnchainedStatus,
};
use heartwood_crypto::blocks::{BlockHeader, factory, serialiser};
use heartwood_crypto::crypto::hash::sha256;
use heartwood_crypto::crypto::sig::{self, SchemeId, Signature, SigningDomain};
use heartwood_crypto::crypto::slots;
use heartwood_crypto::genesis::DerivedKey;
use heartwood_crypto::genesis::derive::role_key;
use heartwood_crypto::transactions::builders::TransactionBuilder;
use heartwood_crypto::transactions::types::{ResignationType, TransferItem, Votes};
use heartwood_crypto::{Aux, Block, KeyPair, Milestones, Params, Transaction, TransactionId};
use heartwood_kernel::{ChainLink, ManualClock};
use heartwood_state::State;
use heartwood_transactions::Handlers;

/// The devnet tooling's key-derivation tag and the chain's public seed.
const TAG: &str = "solar-devnet-v1/";
const SEED: &str = "solar-devnet";
/// The aux of every signature here.
const AUX: [u8; 32] = [0x5a; 32];
/// A fee above every floor of the devnet.
const FEE: u64 = 20_000_000;

/// The committed chain, the keys its seed derives, and the state at its tip.
struct Fixture {
    chain: Chain,
    keys: Vec<DerivedKey>,
    tip: State,
    epoch_ms: i64,
}

static FIXTURE: LazyLock<Fixture> = LazyLock::new(|| {
    let chain = Chain::load();
    let milestones = chain.milestones();
    let mut tip = State::from_genesis(&chain.genesis(), milestones, &Handlers).unwrap();
    for block in &chain.blocks {
        tip.apply_block(block, milestones, &Handlers).unwrap();
    }
    let network = chain.files.network.pub_key_hash;
    let mut roles: Vec<String> = vec!["genesis-1".into(), "genesis-2".into()];
    roles.extend((1..=53).map(|k| format!("delegate/{k}")));
    let keys = roles
        .iter()
        .map(|role| role_key(TAG, role, SEED, network).unwrap())
        .collect();
    let epoch_ms = slots::epoch_unix_ms(milestones.at(1).epoch()).unwrap();
    Fixture {
        chain,
        keys,
        tip,
        epoch_ms,
    }
});

fn milestones() -> &'static Milestones {
    FIXTURE.chain.milestones()
}

fn key(role: &str) -> &'static DerivedKey {
    FIXTURE.keys.iter().find(|k| k.role == role).unwrap()
}

fn key_of(public_key: &heartwood_crypto::PublicKeyBytes) -> Option<&'static KeyPair> {
    FIXTURE
        .keys
        .iter()
        .find(|k| &k.keys.public_key().to_compressed() == public_key)
        .map(|k| &k.keys)
}

/// Stored ids: a set the test fills.
#[derive(Default)]
struct Forged(BTreeSet<TransactionId>);

impl ForgedIds for Forged {
    fn is_stored(&self, id: &TransactionId) -> bool {
        self.0.contains(id)
    }
}

/// The pool: the ids it was asked to remove, in order.
#[derive(Default)]
struct Pool(Vec<TransactionId>);

impl PoolSteps for Pool {
    fn remove_forged(&mut self, transaction: &Transaction, _params: &Params) {
        self.0.push(*transaction.id());
    }
}

/// A node at the chain's tip, started, with its last downloaded block at the tip.
struct Node {
    state: State,
    chain: ChainState,
    forged: Forged,
    pending: Vec<Block>,
    pool: Pool,
    clock: ManualClock,
}

impl Node {
    fn at_tip() -> Node {
        let state = FIXTURE.tip.clone();
        let mut chain = ChainState::at(Arc::new(state.last_block().clone()));
        chain.started = true;
        Node {
            state,
            chain,
            forged: Forged::default(),
            pending: Vec::new(),
            pool: Pool::default(),
            clock: ManualClock::new(0),
        }
    }

    fn tip(&self) -> &Block {
        self.state.last_block()
    }

    fn params(&self) -> &'static Params {
        milestones().at(self.state.height() + 1)
    }

    /// Process `block` with the clock at the start of its slot.
    fn process(&mut self, block: &Block) -> Traced {
        let now = FIXTURE.epoch_ms + i64::from(block.header().timestamp) * 1_000;
        self.process_at(block, now)
    }

    /// Process `block` with the clock at its slot and `name` as its delegate name.
    fn process_named(&mut self, block: &Block, name: Option<&str>) -> Traced {
        let now = FIXTURE.epoch_ms + i64::from(block.header().timestamp) * 1_000;
        self.clock.set(now);
        let mut ctx = ProcessContext {
            state: &mut self.state,
            handlers: &Handlers,
            milestones: milestones(),
            chain: &mut self.chain,
            forged: &self.forged,
            pending: &self.pending,
            pool: &mut self.pool,
            clock: &self.clock,
        };
        processor::trace_named(&mut ctx, block, name)
    }

    fn process_at(&mut self, block: &Block, now_ms: i64) -> Traced {
        self.clock.set(now_ms);
        let mut ctx = ProcessContext {
            state: &mut self.state,
            handlers: &Handlers,
            milestones: milestones(),
            chain: &mut self.chain,
            forged: &self.forged,
            pending: &self.pending,
            pool: &mut self.pool,
            clock: &self.clock,
        };
        processor::trace(&mut ctx, block)
    }

    fn nonce(&self, key: &DerivedKey) -> u64 {
        self.state
            .wallets()
            .find_by_address(&key.address)
            .map_or(0, heartwood_state::Wallet::nonce)
    }

    fn build(&self, builder: TransactionBuilder, key: &DerivedKey, nonce: u64) -> Transaction {
        let params = self.params();
        builder
            .network(params.network())
            .nonce(nonce)
            .fee(FEE)
            .build(&key.keys, Aux::fixed(AUX), params)
            .unwrap()
    }

    /// A transfer of `amount` from `from` to `to`, with the sender's next nonce plus `skip`.
    fn transfer(&self, from: &DerivedKey, to: &DerivedKey, amount: u64, skip: u64) -> Transaction {
        let item = TransferItem {
            amount,
            recipient: to.address,
        };
        let nonce = self.nonce(from) + 1 + skip;
        self.build(TransactionBuilder::transfer(vec![item]), from, nonce)
    }

    /// A vote of `from` for `username`, with the sender's next nonce plus `skip`.
    fn vote(&self, from: &DerivedKey, username: &str, skip: u64) -> Transaction {
        let votes: Votes = [(username.to_owned(), 10_000)].into_iter().collect();
        let nonce = self.nonce(from) + 1 + skip;
        self.build(TransactionBuilder::vote(votes), from, nonce)
    }

    /// The next block (see [`Forge`]).
    fn forge(&self, forge: Forge) -> Block {
        forge.build(&self.state)
    }

    /// Forge and accept the next block of the scheduled delegate, with `transactions`.
    fn advance(&mut self, transactions: Vec<Transaction>) -> Block {
        let block = self.forge(Forge {
            transactions,
            ..Forge::default()
        });
        let traced = self.process(&block);
        assert_eq!(
            traced.processed.result,
            BlockProcessorResult::Accepted,
            "{traced:?}"
        );
        block
    }
}

/// A block to build on a state: by default the next block, in the first slot after the tip
/// whose delegate's key the seed derives, by that delegate, with its reward.
#[derive(Default)]
struct Forge {
    transactions: Vec<Transaction>,
    /// Slots to leave out after the tip.
    gap: u32,
    /// Sign with this key instead of the scheduled delegate's.
    signer: Option<&'static KeyPair>,
    height: Option<u32>,
    previous: Option<[u8; 32]>,
    timestamp: Option<u32>,
    reward: Option<u64>,
    version: Option<u32>,
    payload_length: Option<u32>,
    /// Sign, then change a byte of the signature.
    bad_signature: bool,
}

impl Forge {
    fn build(self, state: &State) -> Block {
        let ms = milestones();
        let last = state.last_block().header();
        let block_time = u32::try_from(ms.at(1).block_time()).unwrap();
        let last_slot = last.timestamp / block_time;
        let n = u32::try_from(ms.at(1).active_delegates()).unwrap();
        let (slot, scheduled) = (last_slot + 1 + self.gap..last_slot + 200)
            .find_map(|slot| {
                let forger = state.forging_delegates().get((slot % n) as usize)?;
                key_of(&forger.public_key).map(|keys| (slot, (forger.public_key, keys)))
            })
            .unwrap();
        let height = self.height.unwrap_or(last.height + 1);
        let reward = self.reward.unwrap_or_else(|| {
            state
                .reward_for_block_in_round(height, &scheduled.0, ms)
                .map_or(0, |r| r.reward)
        });
        let signer = self.signer.unwrap_or(scheduled.1);
        let ids: Vec<u8> = self
            .transactions
            .iter()
            .flat_map(|t| t.id().as_bytes().to_vec())
            .collect();
        let count = u32::try_from(self.transactions.len()).unwrap();
        let mut header = BlockHeader {
            version: self.version.unwrap_or(0),
            timestamp: self.timestamp.unwrap_or(slot * block_time),
            height,
            previous_block: self.previous.unwrap_or(*state.last_block().id().as_bytes()),
            number_of_transactions: count,
            total_amount: self.transactions.iter().map(Transaction::amount).sum(),
            total_fee: self.transactions.iter().map(Transaction::fee).sum(),
            reward,
            payload_length: self.payload_length.unwrap_or(count * 32),
            payload_hash: sha256(&ids),
            generator_public_key: signer.public_key().to_compressed(),
            block_signature: Signature::from_array([0; 64]),
        };
        header.block_signature = sig::sign(
            SchemeId::Secp256k1Bip340,
            SigningDomain::Block,
            &serialiser::signing_hash(&header),
            signer.secret_key(),
            Aux::fixed(AUX),
        )
        .unwrap();
        if self.bad_signature {
            let mut bytes = *header.block_signature.as_bytes();
            bytes[63] ^= 1;
            header.block_signature = Signature::from_array(bytes);
        }
        factory::from_data(header, &self.transactions, ms).unwrap()
    }
}

fn refused(refusal: Refusal) -> Handler {
    Handler::Refused(refusal)
}

/// The last downloaded block, set away from the tip before a refusal, is back at the tip.
fn assert_reset(node: &Node) {
    assert_eq!(node.chain.last_downloaded, Some(ChainLink::of(node.tip())));
}

/// A link below the tip, to see a reset.
fn lower_link(node: &Node) -> Option<ChainLink> {
    Some(ChainLink::of(node.state.genesis_block()))
}

#[test]
fn the_scheduled_next_block_is_accepted() {
    let mut node = Node::at_tip();
    let tip = node.state.height();
    let block = node.forge(Forge::default());
    let traced = node.process(&block);
    assert_eq!(traced.handler, Handler::Accept);
    let processed = traced.processed;
    assert_eq!(processed.result, BlockProcessorResult::Accepted);
    assert_eq!(processed.unchained, None);
    assert!(processed.applied.unwrap().missed.is_empty());
    assert_eq!(node.state.height(), tip + 1);
    assert_eq!(node.chain.last_block().map(|b| b.id()), Some(block.id()));
    assert_eq!(node.chain.last_downloaded, Some(ChainLink::of(&block)));
    let now = FIXTURE.epoch_ms + i64::from(block.header().timestamp) * 1_000;
    assert_eq!(node.chain.last_accepted_ms, Some(now));
    assert!(node.pool.0.is_empty());
}

#[test]
fn an_accepted_block_removes_its_transactions_from_the_pool_in_order() {
    let mut node = Node::at_tip();
    let first = node.transfer(key("genesis-1"), key("genesis-2"), 1_000, 0);
    let second = node.transfer(key("genesis-1"), key("genesis-2"), 2_000, 1);
    let third = node.vote(key("genesis-2"), "genesis_3", 0);
    let ids = vec![*first.id(), *second.id(), *third.id()];
    let block = node.advance(vec![first, second, third]);
    assert_eq!(node.pool.0, ids);
    assert_eq!(node.state.last_block(), &block);
}

#[test]
fn missed_slots_before_an_accepted_block_come_with_its_apply() {
    let mut node = Node::at_tip();
    let block = node.forge(Forge {
        gap: 2,
        ..Forge::default()
    });
    let processed = node.process(&block).processed;
    assert_eq!(processed.result, BlockProcessorResult::Accepted);
    let missed = processed.applied.unwrap().missed;
    assert!(missed.len() >= 2, "{missed:?}");
    assert!(missed.iter().all(|m| m.height == block.height()));
}

#[test]
fn the_wake_up_time_moves_only_once_the_blockchain_has_started() {
    let mut node = Node::at_tip();
    node.chain.started = false;
    node.advance(Vec::new());
    assert_eq!(node.chain.last_accepted_ms, None);
}

#[test]
fn the_last_downloaded_block_moves_up_only_from_below() {
    let mut node = Node::at_tip();
    let ahead = node.forge(Forge {
        height: Some(node.state.height() + 5),
        ..Forge::default()
    });
    node.chain.last_downloaded = Some(ChainLink::of(&ahead));
    node.advance(Vec::new());
    assert_eq!(node.chain.last_downloaded, Some(ChainLink::of(&ahead)));
    node.chain.last_downloaded = None;
    node.advance(Vec::new());
    assert_eq!(node.chain.last_downloaded, None);
}

#[test]
fn an_accepted_block_at_the_forked_height_clears_the_forked_block() {
    let mut node = Node::at_tip();
    let next = node.forge(Forge::default());
    let later = node.forge(Forge {
        height: Some(node.state.height() + 2),
        ..Forge::default()
    });
    node.chain.forked_block = Some(Arc::new(later.clone()));
    node.process(&next);
    assert_eq!(node.chain.forked_block.as_deref(), Some(&later));
    let block = node.advance(Vec::new());
    assert_eq!(block.height(), later.height());
    assert_eq!(node.chain.forked_block, None);
}

#[test]
fn a_block_that_fails_its_verification_is_rejected() {
    let mut node = Node::at_tip();
    let cases = [
        Forge {
            bad_signature: true,
            ..Forge::default()
        },
        Forge {
            version: Some(1),
            ..Forge::default()
        },
        Forge {
            payload_length: Some(32),
            ..Forge::default()
        },
    ];
    for forge in cases {
        let block = node.forge(forge);
        node.chain.last_downloaded = lower_link(&node);
        let traced = node.process(&block);
        assert_eq!(traced.handler, refused(Refusal::VerificationFailed));
        assert_eq!(traced.processed.result, BlockProcessorResult::Rejected);
        assert_reset(&node);
    }
    // A timestamp more than one block time after the clock.
    let block = node.forge(Forge::default());
    let early = FIXTURE.epoch_ms + (i64::from(block.header().timestamp) - 9) * 1_000;
    let traced = node.process_at(&block, early);
    assert_eq!(traced.handler, refused(Refusal::VerificationFailed));
    let at_limit = FIXTURE.epoch_ms + (i64::from(block.header().timestamp) - 8) * 1_000;
    assert_eq!(node.process_at(&block, at_limit).handler, Handler::Accept);
}

#[test]
fn nonces_must_follow_each_sender_s_wallet_nonce() {
    let mut node = Node::at_tip();
    let gap = node.transfer(key("genesis-1"), key("genesis-2"), 1_000, 1);
    let block = node.forge(Forge {
        transactions: vec![gap],
        ..Forge::default()
    });
    node.chain.last_downloaded = lower_link(&node);
    let traced = node.process(&block);
    assert_eq!(traced.handler, refused(Refusal::NonceOutOfOrder));
    assert_eq!(traced.processed.result, BlockProcessorResult::Rejected);
    assert_reset(&node);
    // The same nonce twice from one sender.
    let first = node.transfer(key("genesis-1"), key("genesis-2"), 1_000, 0);
    let again = node.transfer(key("genesis-1"), key("genesis-2"), 2_000, 0);
    let block = node.forge(Forge {
        transactions: vec![first, again],
        ..Forge::default()
    });
    assert_eq!(
        node.process(&block).handler,
        refused(Refusal::NonceOutOfOrder)
    );
}

#[test]
fn each_unchained_status_has_the_reference_s_outcome() {
    let tip_block = FIXTURE.tip.last_block().clone();
    let tip = tip_block.header().clone();
    let tip_key = key_of(&tip.generator_public_key).unwrap();
    let other_key = FIXTURE
        .keys
        .iter()
        .map(|k| &k.keys)
        .find(|k| k.public_key().to_compressed() != tip.generator_public_key)
        .unwrap();
    let at_tip = |signer, timestamp| Forge {
        signer: Some(signer),
        height: Some(tip.height),
        previous: Some(tip.previous_block),
        timestamp: Some(timestamp),
        reward: Some(tip.reward),
        ..Forge::default()
    };
    let node = Node::at_tip();
    let cases: Vec<(Block, UnchainedStatus, BlockProcessorResult)> = vec![
        (
            node.forge(Forge {
                height: Some(tip.height + 2),
                ..Forge::default()
            }),
            UnchainedStatus::NotReadyToAcceptNewHeight,
            BlockProcessorResult::Rejected,
        ),
        (
            FIXTURE.chain.block(tip.height - 1).clone(),
            UnchainedStatus::AlreadyInBlockchain,
            BlockProcessorResult::DiscardedButCanBeBroadcasted,
        ),
        (
            tip_block.clone(),
            UnchainedStatus::EqualToLastBlock,
            BlockProcessorResult::DiscardedButCanBeBroadcasted,
        ),
        (
            node.forge(at_tip(tip_key, tip.timestamp - 8)),
            UnchainedStatus::InvalidTimestamp,
            BlockProcessorResult::Rejected,
        ),
        // The tip ends a round: its slot's delegate is read from the round before.
        (
            node.forge(at_tip(tip_key, tip.timestamp + 1)),
            UnchainedStatus::DoubleForging,
            BlockProcessorResult::Rollback,
        ),
        (
            node.forge(at_tip(other_key, tip.timestamp + 1)),
            UnchainedStatus::GeneratorMismatch,
            BlockProcessorResult::Rejected,
        ),
        // The next height on another parent, by the slot's delegate.
        (
            node.forge(Forge {
                previous: Some([7; 32]),
                ..Forge::default()
            }),
            UnchainedStatus::DoubleForging,
            BlockProcessorResult::Rollback,
        ),
    ];
    for (block, status, result) in cases {
        let mut node = Node::at_tip();
        node.chain.last_downloaded = lower_link(&node);
        let traced = node.process(&block);
        assert_eq!(traced.handler, Handler::Unchained, "{status:?}");
        assert_eq!(traced.processed.unchained, Some(status));
        assert_eq!(traced.processed.result, result, "{status:?}");
        assert_eq!(traced.processed.applied, None);
        assert_reset(&node);
        assert_eq!(node.state, FIXTURE.tip);
    }
}

#[test]
fn an_invalid_generator_is_refused_after_the_chaining_test() {
    let mut node = Node::at_tip();
    let scheduled = node.forge(Forge::default());
    let other = FIXTURE
        .keys
        .iter()
        .filter(|k| k.role.starts_with("delegate/"))
        .map(|k| &k.keys)
        .find(|k| k.public_key().to_compressed() != scheduled.header().generator_public_key)
        .unwrap();
    for signer in [other, &key("genesis-1").keys] {
        let block = node.forge(Forge {
            signer: Some(signer),
            ..Forge::default()
        });
        node.chain.last_downloaded = lower_link(&node);
        let traced = node.process(&block);
        assert_eq!(traced.handler, refused(Refusal::InvalidGenerator));
        assert_eq!(traced.processed.result, BlockProcessorResult::Rejected);
        assert_reset(&node);
    }
}

/// A delegate resigned before a round starts has no rank in it. (A slot with no delegate in the
/// list cannot occur on a consistent state: a resignation that would leave fewer registered
/// delegates than a round has is refused, so every list is full.)
#[test]
fn a_delegate_without_a_rank_is_refused() {
    let mut node = Node::at_tip();
    let n = milestones().at(1).active_delegates() as usize;
    let standing = node
        .state
        .wallets()
        .delegates()
        .filter_map(heartwood_state::Wallet::delegate)
        .filter(|d| d.resigned.is_none())
        .count();
    assert!(standing > n, "the chain has a standby delegate");
    let resigning = key("delegate/1");
    let builder = TransactionBuilder::delegate_resignation(Some(ResignationType::PermanentResign));
    let resignation = node.build(builder, resigning, node.nonce(resigning) + 1);
    node.advance(vec![resignation]);
    let round = node.state.round().info().round;
    while node.state.round().info().round == round {
        node.advance(Vec::new());
    }
    let wallet = node.state.wallets().find_by_address(&resigning.address);
    assert_eq!(wallet.and_then(|w| w.delegate()).and_then(|d| d.rank), None);
    let unranked = node.forge(Forge {
        signer: Some(&resigning.keys),
        ..Forge::default()
    });
    node.chain.last_downloaded = lower_link(&node);
    let traced = node.process(&unranked);
    assert_eq!(traced.handler, refused(Refusal::InvalidGenerator));
    assert_reset(&node);
}

#[test]
fn a_reward_other_than_the_round_s_is_refused() {
    let mut node = Node::at_tip();
    let right = node.forge(Forge::default()).header().reward;
    for reward in [right + 1, right - 1, 0] {
        let block = node.forge(Forge {
            reward: Some(reward),
            ..Forge::default()
        });
        node.chain.last_downloaded = lower_link(&node);
        let traced = node.process(&block);
        assert_eq!(traced.handler, refused(Refusal::InvalidReward));
        assert_reset(&node);
    }
}

#[test]
fn two_transactions_of_a_unique_type_from_one_sender_are_refused() {
    let mut node = Node::at_tip();
    let from = key("genesis-1");
    let first = node.vote(from, "genesis_3", 0);
    let second = node.vote(from, "genesis_4", 1);
    let block = node.forge(Forge {
        transactions: vec![first.clone(), second],
        ..Forge::default()
    });
    node.chain.last_downloaded = lower_link(&node);
    let traced = node.process(&block);
    assert_eq!(traced.handler, refused(Refusal::ConflictingTransactions));
    assert_reset(&node);
    // One vote each from two senders is no conflict.
    let other = node.vote(key("genesis-2"), "genesis_4", 0);
    node.advance(vec![first, other]);
}

#[test]
fn a_stored_or_pending_transaction_is_refused() {
    let mut node = Node::at_tip();
    let transfer = node.transfer(key("genesis-1"), key("genesis-2"), 1_000, 0);
    let block = node.forge(Forge {
        transactions: vec![transfer.clone()],
        ..Forge::default()
    });
    node.forged.0.insert(*transfer.id());
    node.chain.last_downloaded = lower_link(&node);
    let traced = node.process(&block);
    assert_eq!(traced.handler, refused(Refusal::AlreadyForged));
    assert_reset(&node);
    node.forged.0.clear();
    node.pending.push(block.clone());
    assert_eq!(
        node.process(&block).handler,
        refused(Refusal::AlreadyForged)
    );
    node.pending.clear();
    assert_eq!(node.process(&block).handler, Handler::Accept);
}

#[test]
fn a_block_the_state_refuses_leaves_the_state_as_it_was() {
    for gap in [0, 3] {
        let mut node = Node::at_tip();
        let balance = node
            .state
            .wallets()
            .find_by_address(&key("genesis-1").address)
            .unwrap()
            .balance();
        let overspend = node.transfer(key("genesis-1"), key("genesis-2"), balance, 0);
        let block = node.forge(Forge {
            transactions: vec![overspend],
            gap,
            ..Forge::default()
        });
        node.chain.last_downloaded = lower_link(&node);
        let traced = node.process(&block);
        assert_eq!(traced.handler, Handler::Accept);
        assert_eq!(traced.processed.result, BlockProcessorResult::Rejected);
        assert_eq!(traced.processed.applied, None);
        assert_eq!(node.state, FIXTURE.tip);
        assert_eq!(
            node.chain.last_block().map(|b| b.height()),
            Some(node.state.height())
        );
        assert!(node.pool.0.is_empty());
        assert_reset(&node);
    }
}

/// A block whose generator named no delegate when it was enqueued (a delegate registered in the
/// same download) is refused as the reference refuses it, and chains as a mismatch when it
/// does not chain.
#[test]
fn a_block_without_its_delegate_name_has_an_invalid_generator() {
    let mut node = Node::at_tip();
    let block = node.forge(Forge::default());
    let traced = node.process_named(&block, None);
    assert_eq!(traced.handler, refused(Refusal::InvalidGenerator));
    let other_parent = node.forge(Forge {
        previous: Some([7; 32]),
        ..Forge::default()
    });
    let traced = node.process_named(&other_parent, None);
    assert_eq!(
        traced.processed.unchained,
        Some(UnchainedStatus::GeneratorMismatch)
    );
    let name = processor::delegate_name(&node.state, &block).map(str::to_owned);
    assert_eq!(
        node.process_named(&block, name.as_deref()).handler,
        Handler::Accept
    );
}

#[test]
fn the_round_of_a_height_reads_its_forging_list() {
    let state = &FIXTURE.tip;
    let tip = state.height();
    assert_eq!(
        processor::round_forgers(state, tip + 1, milestones()),
        state.forging_delegates()
    );
    // The tip ends a round: its list is the one before the round that follows.
    let ended = processor::round_forgers(state, tip, milestones());
    assert_eq!(ended.len(), state.forging_delegates().len());
    assert_ne!(ended, state.forging_delegates());
    let generator = FIXTURE.tip.last_block().header().generator_public_key;
    assert!(ended.iter().any(|d| d.public_key == generator));
    assert!(processor::round_forgers(state, tip + 60, milestones()).is_empty());
    assert!(!processor::round_forgers(state, 2, milestones()).is_empty());
}

#[test]
fn the_names_are_the_reference_s() {
    assert_eq!(
        BlockProcessorResult::DiscardedButCanBeBroadcasted.name(),
        "DiscardedButCanBeBroadcasted"
    );
    assert_eq!(
        UnchainedStatus::NotReadyToAcceptNewHeight.name(),
        "NotReadyToAcceptNewHeight"
    );
    assert_eq!(Handler::Accept.name(), "AcceptBlockHandler");
    assert_eq!(
        refused(Refusal::ConflictingTransactions).name(),
        "ConflictingTransactionsHandler"
    );
}
