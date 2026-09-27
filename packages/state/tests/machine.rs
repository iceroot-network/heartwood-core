//! The state machine on chains built over the first devnet chain definition of the
//! `V10-genesis` vectors, with the transaction handlers of `heartwood-transactions`: applying
//! and reverting blocks of every kept transaction type restores every digest section; a refused
//! block leaves the state as it was; a revert across a round end restores the round and the
//! ranks of its start; missed slots and the reward of a second block in a round.

use std::collections::BTreeMap;

use heartwood_crypto::blocks::factory::{self, BlockDraft};
use heartwood_crypto::crypto::hash::sha256_concat;
use heartwood_crypto::identities::{Address, PublicKeyBytes};
use heartwood_crypto::transactions::TransactionBuilder;
use heartwood_crypto::transactions::types::{ResignationType, TransferItem, Votes};
use heartwood_crypto::{Aux, Block, KeyPair, Milestones, Network, Transaction};
use heartwood_state::State;
use heartwood_state::digest::{Sections, records};
use heartwood_state::errors::{ApplyError, RevertError, WalletError};
use heartwood_state::machine::Applied;
use heartwood_state::round_state::MissedBlock;
use heartwood_state::supply::check_supply;
use heartwood_transactions::{Handlers, minimum_fee};
use serde_json::Value;

const V10: &str = include_str!("../../crypto/tests/vectors/V10-genesis.jsonl");

/// A chain on the devnet genesis block, with the keys of its delegates and wallets by name:
/// the delegate names, the genesis wallets' roles (`team-placeholder-1`, ...) and the test
/// wallets' roles (`wallet/1`, ...).
struct Chain {
    milestones: Milestones,
    network: u8,
    state: State,
    keys: BTreeMap<String, KeyPair>,
}

impl Chain {
    fn new() -> Chain {
        Chain::with_milestones(|_| {})
    }

    /// A chain whose milestone file is the devnet's changed by `edit`. The genesis block stays
    /// valid while the milestone at height 1 keeps its rules.
    fn with_milestones(edit: impl FnOnce(&mut Vec<Value>)) -> Chain {
        let record: Value = V10
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .find(|record| record["op"] == "genesis.generate")
            .unwrap();
        let output = &record["output"];
        let file = |name: &str| output["files"][name].as_str().unwrap().to_owned();
        let network = Network::load(&file("crypto/network.json")).unwrap();
        let mut list: Vec<Value> = serde_json::from_str(&file("crypto/milestones.json")).unwrap();
        edit(&mut list);
        let milestones =
            Milestones::load(&serde_json::to_string(&list).unwrap(), &network).unwrap();
        let json: Value = serde_json::from_str(&file("crypto/genesisBlock.json")).unwrap();
        let genesis = factory::from_json(&json, &milestones).unwrap();
        let state = State::from_genesis(&genesis, &milestones, &Handlers).unwrap();
        let mut keys = BTreeMap::new();
        for (list, name) in [
            ("delegates", "username"),
            ("genesisWallets", "role"),
            ("testWallets", "role"),
        ] {
            for entry in output[list].as_array().unwrap() {
                let passphrase = entry["passphrase"].as_str().unwrap();
                keys.insert(
                    entry[name].as_str().unwrap().to_owned(),
                    KeyPair::from_passphrase(passphrase).unwrap(),
                );
            }
        }
        Chain {
            network: network.pub_key_hash,
            milestones,
            state,
            keys,
        }
    }

    fn keys(&self, name: &str) -> &KeyPair {
        &self.keys[name]
    }

    fn address(&self, name: &str) -> Address {
        Address::from_public_key(self.keys(name).public_key(), self.network)
    }

    /// The next nonce of `name`, `ahead` transactions after its last applied one.
    fn nonce(&self, name: &str, ahead: u64) -> u64 {
        let nonce = self
            .state
            .wallets()
            .find_by_address(&self.address(name))
            .map_or(0, |wallet| wallet.nonce());
        nonce + 1 + ahead
    }

    /// Sign `builder` for `name` at the next height, with at least the fee floor.
    fn sign(&self, name: &str, builder: TransactionBuilder) -> Transaction {
        let params = self.milestones.at(self.state.height() + 1);
        let keys = self.keys(name);
        let aux = Aux::fixed([0x5a; 32]);
        let transaction = builder.clone().build(keys, aux, params).unwrap();
        let floor = u64::try_from(minimum_fee(&transaction, params)).unwrap();
        if transaction.fee() >= floor {
            transaction
        } else {
            builder.fee(floor).build(keys, aux, params).unwrap()
        }
    }

    fn transfer(&self, from: &str, ahead: u64, to: &[(&str, u64)]) -> Transaction {
        let items = to
            .iter()
            .map(|&(name, amount)| TransferItem {
                amount,
                recipient: self.address(name),
            })
            .collect();
        let builder = TransactionBuilder::transfer(items).nonce(self.nonce(from, ahead));
        self.sign(from, builder)
    }

    fn vote(&self, from: &str, ahead: u64, votes: &[(&str, u16)]) -> Transaction {
        let votes: Votes = votes
            .iter()
            .map(|&(name, bps)| (name.to_owned(), bps))
            .collect();
        let builder = TransactionBuilder::vote(votes).nonce(self.nonce(from, ahead));
        self.sign(from, builder)
    }

    fn register(&self, from: &str, ahead: u64, username: &str) -> Transaction {
        let builder =
            TransactionBuilder::delegate_registration(username).nonce(self.nonce(from, ahead));
        self.sign(from, builder)
    }

    fn resign(&self, from: &str, ahead: u64, kind: ResignationType) -> Transaction {
        let builder =
            TransactionBuilder::delegate_resignation(Some(kind)).nonce(self.nonce(from, ahead));
        self.sign(from, builder)
    }

    fn burn(&self, from: &str, ahead: u64, amount: u64) -> Transaction {
        let builder = TransactionBuilder::burn(amount).nonce(self.nonce(from, ahead));
        self.sign(from, builder)
    }

    fn second_signature(&self, from: &str, ahead: u64, second: &str) -> Transaction {
        let key = self.keys(second).public_key().to_compressed();
        let builder = TransactionBuilder::second_signature(key).nonce(self.nonce(from, ahead));
        self.sign(from, builder)
    }

    /// The next block after `skipped` empty slots, forged by the delegate its slot schedules.
    fn block(&self, transactions: Vec<Transaction>, skipped: u32) -> Block {
        let (timestamp, forger) = self.schedule(skipped);
        self.block_by(transactions, timestamp, self.keys(&forger))
    }

    /// The timestamp of the next block after `skipped` empty slots, and the delegate its slot
    /// schedules.
    fn schedule(&self, skipped: u32) -> (u32, String) {
        let block_time = u32::try_from(self.milestones.at(1).block_time()).unwrap();
        let timestamp = self.state.last_block().header().timestamp + block_time * (1 + skipped);
        let forging = self.state.forging_delegates();
        let slot = usize::try_from(timestamp / block_time).unwrap();
        (timestamp, forging[slot % forging.len()].username.clone())
    }

    /// The next block at `timestamp`, forged with `keys`, with the reward the state gives it.
    fn block_by(&self, transactions: Vec<Transaction>, timestamp: u32, keys: &KeyPair) -> Block {
        let last = self.state.last_block();
        let height = last.height() + 1;
        let generator = keys.public_key().to_compressed();
        let reward = self
            .state
            .reward_for_block_in_round(height, &generator, &self.milestones)
            .map_or(0, |reward| reward.reward);
        let count = u32::try_from(transactions.len()).unwrap();
        let draft = BlockDraft {
            version: 0,
            timestamp,
            height,
            previous_block: *last.id().as_bytes(),
            number_of_transactions: count,
            total_amount: transactions.iter().map(Transaction::amount).sum(),
            total_fee: transactions.iter().map(Transaction::fee).sum(),
            reward,
            payload_length: 32 * count,
            payload_hash: sha256_concat(
                transactions
                    .iter()
                    .map(|transaction| transaction.id().as_bytes().as_slice()),
            ),
        };
        factory::make(
            draft,
            transactions,
            keys,
            Aux::fixed([0x42; 32]),
            &self.milestones,
        )
        .unwrap()
    }

    fn apply(&mut self, block: &Block) -> Applied {
        let applied = self
            .state
            .apply_block(block, &self.milestones, &Handlers)
            .unwrap();
        check_supply(&self.state).unwrap();
        applied
    }

    fn revert(&mut self) -> Block {
        let reverted = self
            .state
            .revert_block(&self.milestones, &Handlers)
            .unwrap();
        check_supply(&self.state).unwrap();
        reverted.block
    }

    fn sections(&self, missed: &[MissedBlock]) -> Sections {
        records(&self.state, &self.milestones, missed)
    }

    /// Every delegate's rank, by name.
    fn ranks(&self) -> BTreeMap<String, Option<u32>> {
        self.state
            .wallets()
            .delegates()
            .map(|wallet| {
                let delegate = wallet.delegate().unwrap();
                (delegate.username().to_owned(), delegate.rank)
            })
            .collect()
    }
}

/// The sections a revert must restore: all but the missed slots of the step.
fn settled(sections: &Sections) -> [&str; 4] {
    [
        &sections.block,
        &sections.supply,
        &sections.round,
        &sections.wallets,
    ]
}

const SXP: u64 = 100_000_000;

#[test]
fn apply_then_revert_restores_every_section() {
    let mut chain = Chain::new();
    let mut blocks = Vec::new();
    let mut forward = vec![chain.sections(&[])];

    // Transfers, one recipient twice.
    let block = chain.block(
        vec![chain.transfer(
            "team-placeholder-1",
            0,
            &[
                ("wallet/1", 5_000 * SXP),
                ("wallet/2", 100 * SXP),
                ("wallet/1", 1),
                ("wallet/3", 100 * SXP),
                ("wallet/4", 100 * SXP),
            ],
        )],
        0,
    );
    // Delegate registrations, a second key and a burn, after two missed slots.
    let applied = chain.apply(&block);
    forward.push(chain.sections(&applied.missed));
    blocks.push(block);
    let block = chain.block(
        vec![
            chain.register("wallet/1", 0, "carol"),
            chain.register("wallet/4", 0, "erin"),
            chain.second_signature("wallet/2", 0, "wallet/9"),
            chain.burn("team-placeholder-1", 0, 2_000_000),
        ],
        2,
    );
    let applied = chain.apply(&block);
    assert_eq!(applied.missed.len(), 2);
    forward.push(chain.sections(&applied.missed));
    blocks.push(block);
    // Votes (split, and for a new delegate), and resignations: 55 delegates allow two.
    let block = chain.block(
        vec![
            chain.vote("wallet/3", 0, &[("genesis_5", 6_000), ("carol", 4_000)]),
            chain.vote("wallet/1", 0, &[("carol", 10_000)]),
            chain.resign("genesis_7", 0, ResignationType::TemporaryResign),
            chain.resign("genesis_8", 0, ResignationType::PermanentResign),
        ],
        0,
    );
    let applied = chain.apply(&block);
    forward.push(chain.sections(&applied.missed));
    blocks.push(block);
    // A vote for a resigned delegate, and a vote that removes every vote.
    let block = chain.block(
        vec![
            chain.vote("team-placeholder-1", 0, &[("genesis_7", 10_000)]),
            chain.vote("wallet/3", 0, &[]),
        ],
        0,
    );
    let applied = chain.apply(&block);
    forward.push(chain.sections(&applied.missed));
    blocks.push(block);
    assert_eq!(chain.state.height(), 5);
    assert_eq!(chain.state.journal().len(), 4);

    // Back to the genesis block, one block at a time.
    for height in (1..5_usize).rev() {
        let reverted = chain.revert();
        assert_eq!(&reverted, &blocks[height - 1]);
        let sections = chain.sections(&[]);
        assert_eq!(
            settled(&sections),
            settled(&forward[height - 1]),
            "height {height}"
        );
    }
    assert_eq!(chain.sections(&[]), forward[0]);
    assert_eq!(
        chain.state.revert_block(&chain.milestones, &Handlers),
        Err(RevertError::Genesis)
    );

    // And forward again: the same states, the same missed slots.
    for (block, expected) in blocks.iter().zip(&forward[1..]) {
        let applied = chain.apply(block);
        assert_eq!(&chain.sections(&applied.missed), expected);
    }
}

#[test]
fn a_refused_block_leaves_the_state_unchanged() {
    let mut chain = Chain::new();
    let block = chain.block(
        vec![chain.transfer("team-placeholder-1", 0, &[("wallet/1", 10 * SXP)])],
        0,
    );
    chain.apply(&block);
    let before = chain.sections(&[]);
    let totals = *chain.state.totals();
    let unchanged = |chain: &Chain| {
        assert_eq!(chain.sections(&[]), before);
        assert_eq!(chain.state.height(), 2);
        assert_eq!(chain.state.journal().len(), 1);
        assert_eq!(chain.state.totals(), &totals);
    };

    // The second transaction overspends: the first is taken back.
    let block = chain.block(
        vec![
            chain.transfer("wallet/1", 0, &[("wallet/3", SXP)]),
            chain.transfer("wallet/1", 1, &[("wallet/3", 100 * SXP)]),
        ],
        3,
    );
    let error = chain
        .state
        .apply_block(&block, &chain.milestones, &Handlers)
        .unwrap_err();
    let ApplyError::Transaction {
        index, rejected, ..
    } = error
    else {
        panic!("{error:?}")
    };
    assert_eq!(index, 1);
    assert_eq!(rejected.class(), "InsufficientBalanceError");
    unchanged(&chain);

    // A first transaction from a wallet with no key and nothing to pay with leaves no key bound.
    let block = chain.block(vec![chain.transfer("wallet/5", 0, &[("wallet/3", 1)])], 0);
    let error = chain
        .state
        .apply_block(&block, &chain.milestones, &Handlers)
        .unwrap_err();
    assert!(
        matches!(error, ApplyError::Transaction { index: 0, .. }),
        "{error:?}"
    );
    unchanged(&chain);
    assert!(
        !before
            .wallets
            .contains(&chain.address("wallet/5").to_base58())
    );

    // A block by a key that is no delegate.
    let (timestamp, _) = chain.schedule(0);
    let block = chain.block_by(Vec::new(), timestamp, chain.keys("wallet/1"));
    assert_eq!(
        chain
            .state
            .apply_block(&block, &chain.milestones, &Handlers),
        Err(ApplyError::GeneratorNotDelegate {
            generator: chain.keys("wallet/1").public_key().to_compressed()
        })
    );
    unchanged(&chain);

    // A block at another height.
    assert_eq!(
        chain.state.apply_block(
            &chain.state.last_block().clone(),
            &chain.milestones,
            &Handlers
        ),
        Err(ApplyError::Height {
            expected: 3,
            height: 2
        })
    );
    unchanged(&chain);

    // The next good block still applies.
    let block = chain.block(vec![chain.transfer("wallet/1", 0, &[("wallet/3", SXP)])], 0);
    chain.apply(&block);
    assert_eq!(chain.state.height(), 3);
}

#[test]
fn a_block_is_checked_before_its_missed_slots_are_counted() {
    // Dated at the end of the timestamp range, the block would leave about 5 × 10^8 slots
    // missed. Its height and its generator are checked first, so it is refused at once, without
    // one row per slot (a regression aborts on the allocation or takes minutes).
    let mut chain = Chain::new();
    let block = chain.block(Vec::new(), 0);
    chain.apply(&block);
    let before = chain.sections(&[]);
    let far = u32::MAX - 7;
    let stranger = chain.block_by(Vec::new(), far, chain.keys("wallet/1"));
    assert_eq!(
        chain
            .state
            .apply_block(&stranger, &chain.milestones, &Handlers),
        Err(ApplyError::GeneratorNotDelegate {
            generator: chain.keys("wallet/1").public_key().to_compressed()
        })
    );
    assert_eq!(chain.sections(&[]), before);
}

#[test]
fn revert_across_a_round_end_restores_the_round_start_ranks() {
    let mut chain = Chain::new();
    let genesis_ranks = chain.ranks();
    // A large vote for genesis_40, and a delegate registered during round 1.
    let block = chain.block(
        vec![
            chain.vote("team-placeholder-1", 0, &[("genesis_40", 10_000)]),
            chain.transfer("genesis-1", 0, &[("wallet/1", 1_000 * SXP)]),
        ],
        0,
    );
    chain.apply(&block);
    let block = chain.block(vec![chain.register("wallet/1", 0, "dave")], 0);
    chain.apply(&block);
    // Ranks are the round-start snapshot: nothing moves before the round ends.
    assert_eq!(chain.ranks().get("dave"), Some(&None));
    let mut expected = genesis_ranks.clone();
    expected.insert("dave".into(), None);
    assert_eq!(chain.ranks(), expected);

    let mut before_end = None;
    while chain.state.height() < 53 {
        if chain.state.height() == 52 {
            before_end = Some((chain.sections(&[]), chain.ranks()));
        }
        let block = chain.block(Vec::new(), 0);
        let applied = chain.apply(&block);
        if chain.state.height() < 53 {
            assert_eq!(applied.round_started, None);
        } else {
            let started = applied.round_started.unwrap();
            assert_eq!(started.round, 2);
            assert_eq!(started.delegates.len(), 53);
            assert_eq!(started.delegates[0].username, "genesis_40");
            assert_eq!(started.delegates, chain.state.round().round_delegates());
        }
    }
    let (sections_52, ranks_52) = before_end.unwrap();
    let sections_53 = chain.sections(&[]);
    let ranks_53 = chain.ranks();
    assert_eq!(ranks_53["genesis_40"], Some(1));
    assert!(ranks_53["dave"].is_some());
    assert_eq!(chain.state.round().info().round, 2);
    assert!(chain.state.round().blocks().is_empty());

    // Revert the block that ended round 1: round 1 and its ranks come back.
    let reverted = chain
        .state
        .revert_block(&chain.milestones, &Handlers)
        .unwrap();
    assert_eq!(reverted.round_reverted, Some(2));
    assert_eq!(chain.ranks(), ranks_52);
    assert_eq!(settled(&chain.sections(&[])), settled(&sections_52));
    assert_eq!(chain.state.round().blocks().len(), 51);

    // Apply it again: the same round 2.
    chain.apply(&reverted.block);
    assert_eq!(chain.sections(&[]), sections_53);
    assert_eq!(chain.ranks(), ranks_53);

    // Revert through the round end and back to the genesis block, then forward again.
    let mut reverted = Vec::new();
    while chain.state.height() > 1 {
        reverted.push(chain.revert());
    }
    assert_eq!(chain.ranks(), genesis_ranks);
    for block in reverted.iter().rev() {
        chain.apply(block);
    }
    assert_eq!(chain.sections(&[]), sections_53);
    assert_eq!(chain.ranks(), ranks_53);
}

#[test]
fn missed_slots_and_the_reward_of_a_second_block_in_a_round() {
    let mut chain = Chain::new();
    let (_, first_forger) = chain.schedule(0);
    let block = chain.block(Vec::new(), 0);
    let applied = chain.apply(&block);
    assert!(
        applied.missed.is_empty(),
        "no missed slot after the genesis block"
    );
    let generator: PublicKeyBytes = block.header().generator_public_key;
    let first = block.header().reward;

    // A full turn of the forging list later, the same delegate forges again in round 1.
    let turn = u32::try_from(chain.state.forging_delegates().len()).unwrap() - 1;
    let (_, forger) = chain.schedule(turn);
    assert_eq!(forger, first_forger);
    let reward = chain
        .state
        .reward_for_block_in_round(3, &generator, &chain.milestones)
        .unwrap();
    assert!(reward.already_forged);
    let block = chain.block(Vec::new(), turn);
    assert_eq!(block.header().reward, reward.reward);
    let applied = chain.apply(&block);
    assert_eq!(applied.missed.len(), usize::try_from(turn).unwrap());
    let block_time = i64::try_from(chain.milestones.at(1).block_time()).unwrap();
    let forging = chain.state.forging_delegates();
    for (i, missed) in applied.missed.iter().enumerate() {
        let slot = 2 + i;
        assert_eq!(missed.height, 3);
        assert_eq!(missed.timestamp, block_time * i64::try_from(slot).unwrap());
        assert_eq!(missed.username, forging[slot % forging.len()].username);
    }
    let delegate = chain
        .state
        .wallets()
        .find_by_public_key(&generator)
        .unwrap()
        .delegate()
        .unwrap();
    assert_eq!(delegate.produced_blocks, 2);
    assert_eq!(delegate.forged_rewards, first + reward.reward);
    assert_eq!(delegate.last_block, Some(*block.id()));
    assert_eq!(
        chain.state.round().blocks(),
        [first_forger.clone(), first_forger]
    );
    assert_eq!(
        chain.state.totals().rewards,
        u128::from(first + reward.reward),
        "every reward is counted, donations included"
    );
}

#[test]
fn the_supply_totals_never_refuse_a_block_and_a_balance_stops_at_64_bits() {
    // From height 2, a flat reward of 9 × 10^18 per block (a block's reward is at most 2^63 − 1):
    // three blocks reward more than 2^64 − 1 in total, and a forger's third block takes its
    // forged rewards (and its balance) past 2^64 − 1, which the reference's big numbers hold.
    let reward = 9_000_000_000_000_000_000_u64;
    let mut chain = Chain::with_milestones(|list| {
        list.push(serde_json::json!({
            "height": 2,
            "reward": reward.to_string(),
            "dynamicReward": {"enabled": false},
        }));
    });
    let first = chain.block(Vec::new(), 0);
    assert_eq!(first.header().reward, reward);
    chain.apply(&first);
    let forger = chain
        .state
        .wallets()
        .find_by_public_key(&first.header().generator_public_key)
        .and_then(|wallet| wallet.delegate())
        .unwrap()
        .username()
        .to_owned();
    for _ in 0..2 {
        let block = chain.block(Vec::new(), 0);
        chain.apply(&block);
    }
    assert!(chain.state.totals().rewards > u128::from(u64::MAX));

    // The first forger's next blocks, until its values would pass 2^64 − 1.
    for turn in 1.. {
        assert!(turn <= 3, "the forger's values never reached 64 bits");
        let skipped = (0..200)
            .find(|skipped| chain.schedule(*skipped).1 == forger)
            .unwrap();
        let block = chain.block(Vec::new(), skipped);
        let before = chain.sections(&[]);
        let totals = *chain.state.totals();
        match chain
            .state
            .apply_block(&block, &chain.milestones, &Handlers)
        {
            Ok(_) => check_supply(&chain.state).unwrap(),
            Err(error) => {
                assert!(
                    matches!(
                        error,
                        ApplyError::Wallet(
                            WalletError::BalanceOverflow { .. } | WalletError::DelegateRange { .. }
                        )
                    ),
                    "{error:?}"
                );
                assert_eq!(chain.sections(&[]), before);
                assert_eq!(chain.state.totals(), &totals);
                assert!(turn >= 2);
                break;
            }
        }
    }
}
