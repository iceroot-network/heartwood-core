//! The store-backed productivity and totals: the window of the reference's UTC midnight, a table
//! kept for its tip, totals that grow with the tip and are summed again after a revert or at a
//! start.

mod history_fixture;

use std::sync::Arc;

use heartwood_api::productivity::window_start;
use heartwood_api::{Productivity, StoreProductivity};
use heartwood_crypto::blocks::factory::{self, BlockDraft};
use heartwood_crypto::crypto::hash::sha256_concat;
use heartwood_crypto::crypto::slots;
use heartwood_crypto::{Aux, Block, BlockId, KeyPair, TransactionId};
use heartwood_database::{BlockRow, Database, MissedRow, Readers, Settings, TransactionRow};
use serde_json::json;

const DAY: u32 = 86_400;

/// A chain in a scratch store, and what it takes to extend or revert it.
struct Chain {
    database: Database,
    settings: Settings,
    epoch: String,
    /// Seconds after the epoch of a UTC midnight three days on: the tests' base time.
    base: u32,
    blocks: Vec<Block>,
}

/// What one block holds in the store.
struct Spec {
    /// Seconds after the base time.
    at: i64,
    name: Option<&'static str>,
    /// The generator's key, which sets the id.
    key: &'static str,
    /// The delegate names its transactions register.
    registers: &'static [&'static str],
    /// A missed slot at this many seconds after the base time.
    missed: Option<(i64, &'static str)>,
    /// The reward its row records.
    reward: u64,
}

fn spec(at: i64, name: &'static str) -> Spec {
    Spec {
        at,
        name: Some(name),
        key: "one",
        registers: &[],
        missed: None,
        reward: 1000,
    }
}

impl Chain {
    fn new(name: &str) -> Chain {
        let files = history_fixture::files();
        let epoch = files.milestones.at(1).epoch().to_owned();
        let epoch_seconds = slots::epoch_unix_ms(&epoch).unwrap().div_euclid(1000);
        let offset = u32::try_from(epoch_seconds.rem_euclid(i64::from(DAY))).unwrap();
        let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("productivity-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let settings = Settings::in_data_dir(&dir);
        Chain {
            database: Database::open(&settings).unwrap(),
            settings,
            epoch,
            base: 3 * DAY - offset,
            blocks: Vec::new(),
        }
    }

    fn at(&self, seconds: i64) -> u32 {
        u32::try_from(i64::from(self.base) + seconds).unwrap()
    }

    /// Make and store the block at the next height.
    fn push(&mut self, spec: &Spec) -> Block {
        let height = u32::try_from(self.blocks.len()).unwrap() + 1;
        let previous = self.blocks.last().map_or([0; 32], |b| *b.id().as_bytes());
        let keys = KeyPair::from_passphrase(spec.key).unwrap();
        let draft = BlockDraft {
            version: 0,
            timestamp: self.at(spec.at),
            height,
            previous_block: previous,
            number_of_transactions: 0,
            total_amount: 0,
            total_fee: 0,
            reward: 0,
            payload_length: 0,
            payload_hash: sha256_concat([]),
        };
        let milestones = history_fixture::files().milestones;
        let block =
            factory::make(draft, Vec::new(), &keys, Aux::fixed([7; 32]), &milestones).unwrap();
        self.store(&block, spec);
        self.blocks.push(block.clone());
        block
    }

    fn store(&mut self, block: &Block, spec: &Spec) {
        let header = block.header();
        let height = block.height();
        let row = BlockRow {
            id: *block.id(),
            height,
            version: 0,
            timestamp: header.timestamp,
            previous_block: Some(BlockId::from_array(header.previous_block)),
            number_of_transactions: 0,
            total_amount: 0,
            total_fee: 0,
            burned_fee: 0,
            reward: spec.reward,
            donations: "{}".into(),
            payload_length: 0,
            payload_hash: "00".repeat(32),
            generator_public_key: header.generator_public_key.to_hex(),
            username: spec.name.map(str::to_owned),
            block_signature: header.block_signature.to_hex(),
        };
        let transaction = |sequence: u32, group: u32, kind: u16, amount, fee, asset| {
            let mut bytes = [0x77; 32];
            bytes[..8]
                .copy_from_slice(&(u64::from(height) << 32 | u64::from(sequence)).to_be_bytes());
            TransactionRow {
                id: TransactionId::from_array(bytes),
                version: 3,
                block_id: *block.id(),
                block_height: height,
                sequence,
                timestamp: header.timestamp,
                nonce: u64::from(height) * 100 + u64::from(sequence),
                sender_id: "TSender".into(),
                sender_public_key: if height == 1 {
                    header.generator_public_key.to_hex()
                } else {
                    format!("03{}", "11".repeat(32))
                },
                recipient_id: None,
                kind,
                type_group: group,
                memo: None,
                amount,
                fee,
                burned_fee: fee / 2,
                serialised: vec![0xee],
                asset,
                recipients: Vec::new(),
            }
        };
        // Every block burns 100 times its height at a fee of twice its height; the first also
        // issues 1500, and registrations come with the blocks that name them.
        let mut rows = vec![transaction(
            0,
            2,
            0,
            100 * u64::from(height),
            2 * u64::from(height),
            None,
        )];
        if height == 1 {
            let items = json!({"transfers": [{"amount": "1000"}, {"amount": "500"}]});
            rows.push(transaction(1, 1, 6, 0, 0, Some(items.to_string())));
        }
        for (index, name) in spec.registers.iter().enumerate() {
            let asset = json!({"delegate": {"username": name}}).to_string();
            let sequence = u32::try_from(index).unwrap() + 2;
            rows.push(transaction(sequence, 1, 2, 0, 0, Some(asset)));
        }
        let missed = spec.missed.map(|(at, name)| (i64::from(self.at(at)), name));
        let mut chunk = self.database.begin().unwrap();
        chunk.save_block(&row, &rows).unwrap();
        if let Some((timestamp, name)) = missed {
            chunk
                .save_missed(&[MissedRow {
                    timestamp,
                    height,
                    username: name.into(),
                }])
                .unwrap();
        }
        chunk.commit().unwrap();
    }

    /// Take the blocks above `height` back out of the store and the chain.
    fn revert_to(&mut self, height: u32) {
        let mut chunk = self.database.begin().unwrap();
        chunk.delete_above(height, 1).unwrap();
        chunk.commit().unwrap();
        self.blocks.truncate(usize::try_from(height).unwrap());
    }

    fn productivity(&self, lookback: i64) -> StoreProductivity {
        let readers = Arc::new(Readers::open(&self.settings, 1).unwrap());
        StoreProductivity::new(readers, lookback).with_epoch(&self.epoch)
    }
}

fn table(productivity: &StoreProductivity, tip: &Block) -> Vec<(String, Option<u32>, Option<u32>)> {
    productivity
        .table(tip)
        .unwrap()
        .by_username
        .iter()
        .map(|(name, p)| (name.clone(), p.missed, p.productivity.map(|h| h.0)))
        .collect()
}

fn row(name: &str, missed: u32, hundredths: u32) -> (String, Option<u32>, Option<u32>) {
    (name.to_owned(), Some(missed), Some(hundredths))
}

/// A chain whose window, of 500 seconds before the tip's midnight (the base time), starts at a
/// block of alice's and a missed slot of bob's, with one block and one slot just before it.
fn windowed(name: &str) -> Chain {
    let mut chain = Chain::new(name);
    chain.push(&Spec {
        at: -2 * i64::from(DAY),
        name: None,
        registers: &[],
        ..spec(0, "")
    });
    chain.push(&Spec {
        registers: &["alice", "bob"],
        missed: Some((-501, "bob")),
        ..spec(-501, "alice")
    });
    chain.push(&Spec {
        missed: Some((-500, "bob")),
        ..spec(-500, "alice")
    });
    chain.push(&spec(100, "bob"));
    chain.push(&spec(200, "alice"));
    chain
}

#[test]
fn the_window_starts_at_the_midnight_of_the_tips_date() {
    let epoch = |text| slots::epoch_unix_ms(text).unwrap();
    let evening = epoch("2022-03-28T18:00:00.000Z");
    // The tip at 18:00 on the 28th: midnight is 18 hours before the epoch.
    assert_eq!(window_start(0, evening, 0), -64_800);
    assert_eq!(window_start(0, evening, 2_592_000), -64_800 - 2_592_000);
    // Up to 23:59:59 the date is the same; at 00:00:00 the next day's midnight is the start.
    assert_eq!(window_start(21_599, evening, 0), -64_800);
    assert_eq!(window_start(21_600, evening, 0), 21_600);
    assert_eq!(window_start(21_600 + DAY - 1, evening, 0), 21_600);
    // An epoch with milliseconds gives a fractional start, which counts from the next second.
    let fraction = epoch("2022-03-28T18:00:00.250Z");
    assert_eq!(window_start(0, fraction, 0), -64_800);
    assert_eq!(window_start(21_600, fraction, 0), 21_600);
    // An epoch at midnight starts the window at midnight of the tip's day count.
    let midnight = epoch("2026-01-01T00:00:00.000Z");
    assert_eq!(
        window_start(3 * DAY + 5, midnight, 100),
        i64::from(3 * DAY) - 100
    );
}

#[test]
fn the_window_counts_its_first_second_and_not_the_one_before() {
    let chain = windowed("window");
    let productivity = chain.productivity(500);
    let tip = chain.blocks.last().unwrap();
    assert_eq!(
        table(&productivity, tip),
        [row("alice", 0, 10_000), row("bob", 1, 5_000)]
    );
}

#[test]
fn a_table_is_kept_for_its_tip_and_made_again_for_another() {
    let mut chain = windowed("cache");
    let productivity = chain.productivity(500);
    let tip = chain.blocks.last().unwrap().clone();
    let first = productivity.table(&tip).unwrap();
    // The store grows, but the tip the routes ask about has not changed.
    chain.push(&Spec {
        missed: Some((250, "alice")),
        ..spec(300, "alice")
    });
    let again = productivity.table(&tip).unwrap();
    assert!(Arc::ptr_eq(&first, &again));
    // A new tip makes it again: alice has 3 blocks and a miss.
    let grown = chain.blocks.last().unwrap();
    assert_eq!(
        table(&productivity, grown),
        [row("alice", 1, 7_500), row("bob", 1, 5_000)]
    );
    // A revert changes the tip's id at the same height: bob forges block 5 instead.
    chain.revert_to(4);
    let forked = chain.push(&Spec {
        key: "two",
        ..spec(250, "bob")
    });
    assert_eq!(
        table(&productivity, &forked),
        [row("alice", 0, 10_000), row("bob", 1, 6_667)]
    );
    // A start (a new value over the same store) makes it from the store.
    let restarted = chain.productivity(500);
    assert_eq!(table(&restarted, &forked), table(&productivity, &forked));
}

#[test]
fn a_window_that_reaches_the_genesis_block_gives_the_last_table_or_none() {
    let chain = windowed("genesis");
    // The genesis block has no generator name: the reference's query fails, its error is
    // swallowed, and there is nothing to keep at a start.
    let productivity = chain.productivity(10 * i64::from(DAY));
    let tip = chain.blocks.last().unwrap();
    assert!(productivity.table(tip).unwrap().by_username.is_empty());
}

/// The totals of the first `count` blocks of the test chain: every block burns 100 times its
/// height at a fee of twice its height (half of it burned), rewards 1000, and the first issues
/// 1500.
fn expected(count: i128) -> (u128, u128, i128) {
    let sum = count * (count + 1) / 2;
    let (fees, burns) = (
        u128::try_from(sum).unwrap(),
        u128::try_from(100 * sum).unwrap(),
    );
    (fees, burns, 1500 + 1000 * count - sum - 100 * sum)
}

fn totals(productivity: &StoreProductivity, tip: &Block) -> (u128, u128, i128) {
    let totals = productivity.totals(tip).unwrap();
    (
        totals.burned_fees,
        totals.burned_transactions,
        totals.supply,
    )
}

/// A chain of `count` blocks, one an hour.
fn hourly(name: &str, count: i64) -> Chain {
    let mut chain = Chain::new(name);
    for height in 1..=count {
        chain.push(&spec(height * 3600, "alice"));
    }
    chain
}

#[test]
fn totals_are_summed_at_a_start_and_grow_with_the_tip() {
    let mut chain = hourly("totals", 5);
    let productivity = chain.productivity(500);
    assert_eq!(totals(&productivity, &chain.blocks[4]), expected(5));
    // The tip's own totals again, and an older tip's, which sums from the start.
    assert_eq!(totals(&productivity, &chain.blocks[4]), expected(5));
    assert_eq!(totals(&productivity, &chain.blocks[2]), expected(3));
    assert_eq!(totals(&productivity, &chain.blocks[4]), expected(5));
    // The stored rewards of blocks 3 to 5 double (their ids stay), and two blocks follow. The
    // sums that kept their earlier blocks show the old rewards of those three; sums made again
    // show the new.
    let saved = chain.blocks[2..].to_vec();
    chain.revert_to(2);
    for block in saved {
        chain.blocks.push(block.clone());
        chain.store(
            &block,
            &Spec {
                reward: 2000,
                ..spec(0, "alice")
            },
        );
    }
    chain.push(&spec(6 * 3600, "alice"));
    chain.push(&spec(7 * 3600, "alice"));
    let (fees, burns, supply) = expected(7);
    assert_eq!(
        totals(&productivity, &chain.blocks[6]),
        (fees, burns, supply)
    );
    assert_eq!(
        totals(&chain.productivity(500), &chain.blocks[6]),
        (fees, burns, supply + 3000)
    );
}

#[test]
fn totals_are_summed_again_after_a_revert() {
    let mut chain = hourly("totals-revert", 5);
    let productivity = chain.productivity(500);
    assert_eq!(totals(&productivity, &chain.blocks[4]), expected(5));
    // Blocks 4 and 5 are taken back, and other blocks 4 to 6 (another generator's) follow: the
    // block at the cached height is no longer the cached one, so the sums start again.
    chain.revert_to(3);
    for height in 4..=6 {
        chain.push(&Spec {
            key: "two",
            ..spec(height * 3600 + 1, "bob")
        });
    }
    assert_eq!(totals(&productivity, &chain.blocks[5]), expected(6));
    // A tip below the cached height is summed again too.
    assert_eq!(totals(&productivity, &chain.blocks[3]), expected(4));
}
