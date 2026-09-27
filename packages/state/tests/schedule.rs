//! The forging schedule: the shuffle against the golden vectors of class V09-shuffle, round
//! starts and their reverts, missed slots, and the reward of a block in its round.

use heartwood_crypto::crypto::hash::sha256;
use heartwood_crypto::utils::hex;
use heartwood_crypto::{
    Address, BlockHeader, KeyPair, Milestones, Network, PublicKey, PublicKeyBytes, Signature,
};
use heartwood_kernel::RoundInfo;
use heartwood_state::errors::{RewardError, RoundError};
use heartwood_state::round_state::{MissedBlock, RoundDelegate, RoundReward, shuffle_delegates};
use heartwood_state::{Delegate, Resignation, RoundState, WalletRepository};
use serde_json::{Value, json};

const NETWORK: u8 = 90;
const BLOCK_TIME: u32 = 8;

const V09: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/vectors/V09-shuffle.jsonl"
));

/// The records of a vector file, after its meta record.
fn records(text: &str, class: &str) -> Vec<Value> {
    let mut lines = text
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("a JSON record"));
    let meta = lines.next().expect("a meta record");
    assert_eq!(meta["op"], "meta");
    assert_eq!(meta["class"], class);
    let records: Vec<Value> = lines.collect();
    assert_eq!(meta["records"], records.len());
    records
}

fn order(round: u64, count: u64) -> Vec<u64> {
    let input: Vec<u64> = (0..count).collect();
    shuffle_delegates(round, &input)
}

#[test]
fn v09_shuffle() {
    let mut rejection_rounds = 0;
    for record in records(V09, "V09-shuffle") {
        let input = &record["input"];
        let output = &record["output"];
        let count = input["count"].as_u64().unwrap();
        let label = format!("V09 {}", record["name"]);
        if let Some(round) = input["round"].as_u64() {
            assert_eq!(output["order"], json!(order(round, count)), "{label}");
            rejection_rounds += usize::from(label.contains("rejected draw"));
            continue;
        }
        let (from, to) = (
            input["from"].as_u64().unwrap(),
            input["to"].as_u64().unwrap(),
        );
        let mut text = String::new();
        for round in from..=to {
            let line: Vec<String> = order(round, count).iter().map(u64::to_string).collect();
            text.push_str(&line.join(","));
            text.push('\n');
        }
        assert_eq!(output["rounds"], to - from + 1, "{label}");
        assert_eq!(
            output["sha256"],
            hex::encode(&sha256(text.as_bytes())),
            "{label}"
        );
    }
    assert_eq!(rejection_rounds, 10);
}

fn network() -> Network {
    Network {
        name: "devnet".into(),
        message_prefix: "devnet message:\n".into(),
        address_character: Some("d".into()),
        bip32_public: 70_617_039,
        bip32_private: 70_615_956,
        pub_key_hash: NETWORK,
        nethash: "0".repeat(64),
        wif: 252,
        slip44: 1,
        token: "dROOT".into(),
        symbol: "dRT".into(),
        explorer: String::new(),
    }
}

/// Milestones with rounds of `active_delegates` and the reward rules `rewards` (keys of the
/// milestone at height 1).
fn milestones(active_delegates: u64, rewards: Value) -> Milestones {
    let mut milestone = json!({
        "height": 1,
        "activeDelegates": active_delegates,
        "blockTime": BLOCK_TIME,
        "block": { "version": 0, "maxTransactions": 150, "maxPayload": 2_097_152 },
        "epoch": "2026-01-01T00:00:00.000Z",
        "reward": 0,
        "burn": { "feeBasisPoints": 9000, "txAmount": 2_000_000 },
        "donations": {},
        "dynamicReward": { "enabled": false },
        "transfer": { "maximum": 256, "minimum": 1 },
    });
    if let (Value::Object(target), Value::Object(changes)) = (&mut milestone, rewards) {
        target.extend(changes);
    }
    Milestones::load(&json!([milestone]).to_string(), &network()).expect("the milestones load")
}

/// Delegates `d0`, `d1`, ... with keys, vote balances from `vote_balances` (so `d0` ranks
/// first when the balances fall).
fn delegates(vote_balances: &[u64]) -> (WalletRepository, Vec<Address>) {
    let mut wallets = WalletRepository::new(NETWORK);
    let mut addresses = Vec::new();
    for (i, &vote_balance) in vote_balances.iter().enumerate() {
        let public_key = KeyPair::from_passphrase(&format!("schedule {i}"))
            .unwrap()
            .public_key()
            .clone();
        let address = Address::from_public_key(&public_key, NETWORK);
        wallets.bind_public_key(&address, public_key);
        wallets
            .register_delegate(&address, format!("d{i}"))
            .unwrap()
            .vote_balance = vote_balance;
        addresses.push(address);
    }
    (wallets, addresses)
}

fn delegate<'a>(wallets: &'a WalletRepository, username: &str) -> &'a Delegate {
    wallets
        .find_by_username(username)
        .unwrap()
        .delegate()
        .unwrap()
}

fn ranks(wallets: &WalletRepository) -> Vec<(String, Option<u32>)> {
    wallets
        .delegates()
        .filter_map(|wallet| wallet.delegate())
        .map(|delegate| (delegate.username().to_owned(), delegate.rank))
        .collect()
}

fn names(delegates: &[RoundDelegate]) -> Vec<&str> {
    delegates.iter().map(|d| d.username.as_str()).collect()
}

fn round_one(max_delegates: u64) -> RoundInfo {
    RoundInfo {
        round: 1,
        round_height: 1,
        next_round: 1,
        max_delegates,
    }
}

#[test]
fn a_round_starts_with_the_ranking_and_its_shuffle() {
    let (mut wallets, _) = delegates(&[10, 50, 30, 40, 20]);
    let round = RoundState::start(&mut wallets, round_one(3)).unwrap();
    assert_eq!(round.info(), &round_one(3));
    assert_eq!(names(round.round_delegates()), ["d1", "d3", "d2"]);
    assert_eq!(
        round
            .round_delegates()
            .iter()
            .map(|d| d.vote_balance)
            .collect::<Vec<_>>(),
        [50, 40, 30]
    );
    assert_eq!(
        round.forging_delegates(),
        shuffle_delegates(1, round.round_delegates())
    );
    assert!(round.blocks().is_empty());
    assert_eq!(
        ranks(&wallets),
        [
            ("d0".to_owned(), Some(5)),
            ("d1".to_owned(), Some(1)),
            ("d2".to_owned(), Some(3)),
            ("d3".to_owned(), Some(2)),
            ("d4".to_owned(), Some(4)),
        ]
    );
}

#[test]
fn a_refused_round_start_leaves_every_rank_as_it_was() {
    let (mut wallets, addresses) = delegates(&[10, 20, 30]);
    RoundState::start(&mut wallets, round_one(3)).unwrap();
    let before = wallets.clone();
    wallets
        .wallet_mut(&addresses[0])
        .unwrap()
        .delegate_mut()
        .unwrap()
        .resigned = Some(Resignation::Permanent);
    let resigned = wallets.clone();
    assert_eq!(
        RoundState::start(&mut wallets, round_one(3)),
        Err(RoundError::NotEnoughDelegates {
            required: 3,
            ranked: 2
        })
    );
    assert_eq!(wallets, resigned);
    assert_eq!(ranks(&wallets), ranks(&before));
}

#[test]
fn a_round_ends_starts_the_next_one_and_reverts_exactly() {
    let milestones = milestones(3, json!({}));
    let (mut wallets, addresses) = delegates(&[10, 50, 30, 40, 20]);
    let mut round = RoundState::start(&mut wallets, round_one(3)).unwrap();
    let first = round.clone();
    let first_wallets = wallets.clone();

    // Heights 2 and 3 are in round 1; the block at 3 ends it.
    assert_eq!(
        round.apply_block(&mut wallets, 2, "d1", &milestones),
        Ok(None)
    );
    assert_eq!(round.blocks(), ["d1"]);
    let after_two = round.clone();

    // The ranking changes during the round: d0 overtakes every other delegate, and d3 resigns.
    let delegate = wallets
        .wallet_mut(&addresses[0])
        .unwrap()
        .delegate_mut()
        .unwrap();
    delegate.vote_balance = 100;
    wallets
        .wallet_mut(&addresses[3])
        .unwrap()
        .delegate_mut()
        .unwrap()
        .resigned = Some(Resignation::Temporary);
    let changed_wallets = wallets.clone();

    let undo = round
        .apply_block(&mut wallets, 3, "d3", &milestones)
        .unwrap()
        .expect("the block at 3 ends round 1");
    assert_eq!(undo.previous.blocks(), ["d1", "d3"]);
    assert_eq!(
        undo.ranks,
        wallets
            .delegates()
            .map(|w| (
                *w.address(),
                first_wallets
                    .find_by_address(w.address())
                    .unwrap()
                    .delegate()
                    .unwrap()
                    .rank
            ))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        round.info(),
        &RoundInfo {
            round: 2,
            round_height: 4,
            next_round: 2,
            max_delegates: 3
        }
    );
    assert_eq!(names(round.round_delegates()), ["d0", "d1", "d2"]);
    assert_eq!(
        round.forging_delegates(),
        shuffle_delegates(2, round.round_delegates())
    );
    assert!(round.blocks().is_empty());
    assert_eq!(delegate_rank(&wallets, "d3"), None);
    assert_eq!(delegate_rank(&wallets, "d0"), Some(1));

    // Reverting the block at 3 restores round 1 with its blocks up to 2, and every rank of its
    // start; the vote balances and the resignation are the transactions' to revert.
    round.revert_block(&mut wallets, 3, Some(undo)).unwrap();
    assert_eq!(round, after_two);
    assert_eq!(wallets, changed_wallets);
    assert_eq!(ranks(&wallets), ranks(&first_wallets));

    round.revert_block(&mut wallets, 2, None).unwrap();
    assert_eq!(round, first);
    assert_eq!(
        round.revert_block(&mut wallets, 1, None),
        Err(RoundError::NotLastBlock { height: 1 })
    );
    assert_eq!(round, first);
}

fn delegate_rank(wallets: &WalletRepository, username: &str) -> Option<u32> {
    delegate(wallets, username).rank
}

#[test]
fn a_revert_must_name_the_last_block() {
    let milestones = milestones(3, json!({}));
    let (mut wallets, _) = delegates(&[10, 20, 30]);
    let mut round = RoundState::start(&mut wallets, round_one(3)).unwrap();
    round
        .apply_block(&mut wallets, 2, "d0", &milestones)
        .unwrap();
    let before = round.clone();
    for height in [1, 3, 4] {
        assert_eq!(
            round.revert_block(&mut wallets, height, None),
            Err(RoundError::NotLastBlock { height })
        );
        assert_eq!(round, before);
    }
    let undo = round
        .apply_block(&mut wallets, 3, "d1", &milestones)
        .unwrap();
    assert!(undo.is_some());
    let started = round.clone();
    // The undo of a round start belongs to the block that ended the round.
    assert_eq!(
        round.revert_block(&mut wallets, 2, undo.clone()),
        Err(RoundError::NotLastBlock { height: 2 })
    );
    assert_eq!(round, started);
    round.revert_block(&mut wallets, 3, undo).unwrap();
    assert_eq!(round, before);
}

#[test]
fn a_refused_round_start_leaves_the_round_as_it_was() {
    let milestones = milestones(3, json!({}));
    let (mut wallets, addresses) = delegates(&[10, 20, 30]);
    let mut round = RoundState::start(&mut wallets, round_one(3)).unwrap();
    round
        .apply_block(&mut wallets, 2, "d0", &milestones)
        .unwrap();
    wallets
        .wallet_mut(&addresses[1])
        .unwrap()
        .delegate_mut()
        .unwrap()
        .resigned = Some(Resignation::Temporary);
    let (before, before_wallets) = (round.clone(), wallets.clone());
    assert_eq!(
        round.apply_block(&mut wallets, 3, "d2", &milestones),
        Err(RoundError::NotEnoughDelegates {
            required: 3,
            ranked: 2
        })
    );
    assert_eq!(round, before);
    assert_eq!(wallets, before_wallets);
}

#[test]
fn every_block_ends_a_round_of_one() {
    let milestones = milestones(1, json!({}));
    let (mut wallets, _) = delegates(&[10, 20]);
    let info = RoundInfo {
        round: 2,
        round_height: 2,
        next_round: 3,
        max_delegates: 1,
    };
    let mut round = RoundState::start(&mut wallets, info).unwrap();
    let genesis = round.clone();
    let undo = round
        .apply_block(&mut wallets, 2, "d1", &milestones)
        .unwrap()
        .expect("every block ends a round of one");
    assert_eq!(round.info().round, 3);
    assert_eq!(round.info().round_height, 3);
    round.revert_block(&mut wallets, 2, Some(undo)).unwrap();
    assert_eq!(round, genesis);
}

fn header(height: u32, timestamp: u32) -> BlockHeader {
    BlockHeader {
        version: 0,
        timestamp,
        height,
        previous_block: [0; 32],
        number_of_transactions: 0,
        total_amount: 0,
        total_fee: 0,
        reward: 0,
        payload_length: 0,
        payload_hash: [0; 32],
        generator_public_key: PublicKeyBytes::from_array([2; 33]),
        block_signature: Signature::from_array([0; 64]),
    }
}

#[test]
fn missed_slots_name_the_scheduled_delegates() {
    let milestones = milestones(3, json!({}));
    let (mut wallets, _) = delegates(&[10, 20, 30]);
    let round = RoundState::start(&mut wallets, round_one(3)).unwrap();
    let forging = names(round.forging_delegates());
    let missed = |last: BlockHeader, next: BlockHeader| {
        round.detect_missed_blocks(&last, &next, &milestones)
    };

    // The next slot, or the same one, misses nothing.
    assert!(missed(header(5, 800), header(6, 808)).is_empty());
    assert!(missed(header(5, 800), header(6, 807)).is_empty());
    assert!(missed(header(5, 800), header(6, 790)).is_empty());
    // Nothing is missed after the genesis block.
    assert!(missed(header(1, 0), header(2, 800)).is_empty());

    // Slot 100 forged, slots 101 to 104 missed, slot 105 forged (timestamps anywhere in their
    // slots); the forging list repeats.
    let rows = missed(header(5, 803), header(6, 846));
    let expected: Vec<MissedBlock> = (101_i64..=104)
        .map(|slot| MissedBlock {
            timestamp: slot * i64::from(BLOCK_TIME),
            height: 6,
            username: forging[usize::try_from(slot).unwrap() % 3].to_owned(),
        })
        .collect();
    assert_eq!(rows, expected);
}

#[test]
fn the_reward_of_a_block_in_its_round() {
    let table = json!({
        "reward": 7,
        "dynamicReward": { "enabled": true, "ranks": { "1": 100, "2": 90, "3": 80 }, "secondaryReward": 5 }
    });
    let dynamic = milestones(3, table);
    let fixed = milestones(3, json!({ "reward": 7 }));
    let (mut wallets, addresses) = delegates(&[30, 20, 10]);
    let mut round = RoundState::start(&mut wallets, round_one(3)).unwrap();
    let reward = |round: &RoundState, wallets: &WalletRepository, username: &str, milestones| {
        round.reward_for_block_in_round(2, delegate(wallets, username), milestones)
    };
    let fresh = |reward| {
        Ok(RoundReward {
            already_forged: false,
            reward,
        })
    };

    assert_eq!(reward(&round, &wallets, "d0", &dynamic), fresh(100));
    assert_eq!(reward(&round, &wallets, "d2", &dynamic), fresh(80));
    assert_eq!(reward(&round, &wallets, "d1", &fixed), fresh(7));

    round.apply_block(&mut wallets, 2, "d1", &dynamic).unwrap();
    assert_eq!(
        reward(&round, &wallets, "d1", &dynamic),
        Ok(RoundReward {
            already_forged: true,
            reward: 5
        })
    );
    // Without the dynamic reward, forging twice changes nothing.
    assert_eq!(reward(&round, &wallets, "d1", &fixed), fresh(7));

    // A delegate without a rank: only the rank table has no answer.
    for address in &addresses {
        wallets
            .wallet_mut(address)
            .unwrap()
            .delegate_mut()
            .unwrap()
            .rank = None;
    }
    assert_eq!(
        reward(&round, &wallets, "d0", &dynamic),
        Err(RewardError::NoRank {
            username: "d0".into()
        })
    );
    assert_eq!(reward(&round, &wallets, "d0", &fixed), fresh(7));
    assert_eq!(
        reward(&round, &wallets, "d1", &dynamic),
        Ok(RoundReward {
            already_forged: true,
            reward: 5
        })
    );
    // A rank the table does not list.
    wallets
        .wallet_mut(&addresses[0])
        .unwrap()
        .delegate_mut()
        .unwrap()
        .rank = Some(4);
    assert!(matches!(
        reward(&round, &wallets, "d0", &dynamic),
        Err(RewardError::Reward(_))
    ));
}

#[test]
fn round_rows_carry_the_keys() {
    let (mut wallets, _) = delegates(&[10, 20]);
    let round = RoundState::start(&mut wallets, round_one(2)).unwrap();
    for row in round.round_delegates() {
        let wallet = wallets.find_by_username(&row.username).unwrap();
        assert_eq!(
            Some(row.public_key),
            wallet.public_key().map(PublicKey::to_compressed)
        );
    }
}
