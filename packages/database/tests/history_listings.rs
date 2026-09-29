//! The history readers of the block, missed-block and chain routes over a small store: the block
//! criteria (the id as a prefix, ranges, names), the Core Transfer join, the missed-block
//! criteria, the productivity aggregate on the edges of its window, the fee statistics with ties
//! at one half, the burned totals over ranges, and the integer rounding the reference's SQL does.

mod common;

use heartwood_crypto::TransactionType;
use heartwood_database::{
    BlockCriteria, BlockFilter, CountFlag, Database, Direction, MissedBlockCriteria,
    MissedBlockFilter, MissedRow, Numeric, Order, Page, Readers, TransactionRow, avg_int8,
    hundredths, round_div, round2,
};
use serde_json::json;

use common::{block_id, block_row, open, transaction_row};

const ALL: Page = Page {
    offset: 0,
    limit: 100,
};

/// The block generators by height: the genesis block has none, `carol` and `erin` have no
/// registration.
const NAMES: [Option<&str>; 8] = [
    None,
    Some("alice"),
    Some("alice"),
    Some("bob"),
    Some("alice"),
    Some("bob"),
    Some("carol"),
    Some("alice"),
];

/// Transactions of the store: height, type group, type, fee, the asset and the top-level amount.
fn transactions() -> Vec<(u32, u32, u16, u64, Option<serde_json::Value>, u64)> {
    let registration = |name: &str| Some(json!({"delegate": {"username": name}}));
    let transfers = |amounts: &[&str]| {
        let items: Vec<_> = amounts.iter().map(|a| json!({"amount": a})).collect();
        Some(json!({"transfers": items}))
    };
    vec![
        (1, 1, 6, 0, transfers(&["1000", "500"]), 0),
        (2, 1, 2, 10, registration("alice"), 0),
        (2, 1, 2, 10, registration("bob"), 0),
        (2, 1, 2, 10, registration("dave"), 0),
        (2, 1, 2, 10, registration("frank"), 0),
        (3, 1, 6, 1, transfers(&["10", "20"]), 0),
        (4, 1, 6, 2, transfers(&["5"]), 0),
        (4, 2, 0, 3, None, 700),
        (5, 2, 0, 4, None, 300),
        (6, 1, 6, 2, transfers(&["7"]), 0),
        (8, 1, 6, 4, transfers(&["1"]), 0),
    ]
}

fn missed() -> Vec<MissedRow> {
    let row = |timestamp, height, username: &str| MissedRow {
        timestamp,
        height,
        username: username.into(),
    };
    vec![
        row(350, 4, "bob"),
        row(450, 5, "bob"),
        row(550, 6, "dave"),
        row(650, 7, "erin"),
    ]
}

/// A store of eight blocks, one per 100 seconds, with `transactions()` and `missed()`.
fn store(name: &str) -> (Database, Readers) {
    let (mut database, settings) = open(name);
    let all = transactions();
    for height in 1..=8u32 {
        let rows: Vec<TransactionRow> = all
            .iter()
            .filter(|spec| spec.0 == height)
            .enumerate()
            .map(|(sequence, spec)| {
                let sequence = u32::try_from(sequence).unwrap();
                let mut row = transaction_row(height, sequence);
                row.timestamp = height * 100;
                (row.type_group, row.kind) = (spec.1, spec.2);
                (row.fee, row.burned_fee, row.amount) = (spec.3, spec.3 / 2, spec.5);
                row.asset = spec.4.as_ref().map(ToString::to_string);
                row.recipients = Vec::new();
                if height == 1 {
                    row.sender_public_key = block_row(1, 0).generator_public_key;
                }
                row
            })
            .collect();
        let mut block = block_row(height, u32::try_from(rows.len()).unwrap());
        block.timestamp = height * 100;
        block.reward = u64::from(height) * 10;
        block.username = NAMES[usize::try_from(height - 1).unwrap()].map(str::to_owned);
        let mut chunk = database.begin().unwrap();
        chunk.save_block(&block, &rows).unwrap();
        chunk.commit().unwrap();
    }
    let mut chunk = database.begin().unwrap();
    chunk.save_missed(&missed()).unwrap();
    chunk.commit().unwrap();
    let readers = Readers::open(&settings, 1).unwrap();
    (database, readers)
}

fn order(property: &str, direction: Direction) -> Vec<Order> {
    vec![Order {
        property: property.into(),
        direction,
    }]
}

fn heights(
    readers: &Readers,
    criteria: BlockCriteria,
    order: &[Order],
    page: Page,
) -> (Vec<u32>, u64) {
    let listing = readers
        .get()
        .blocks(
            &BlockFilter::of(criteria),
            order,
            page,
            false,
            CountFlag::Exact,
        )
        .unwrap();
    (
        listing.rows.iter().map(|b| b.row.height).collect(),
        listing.total_count,
    )
}

#[test]
fn the_block_id_is_a_prefix() {
    let (_database, readers) = store("listings-id");
    let by_id = |prefix: &str| BlockCriteria {
        id: vec![prefix.into()],
        ..BlockCriteria::default()
    };
    let up = order("height", Direction::Asc);
    // The ids of the test chain start with the height in eight hex digits.
    assert_eq!(heights(&readers, by_id("00000003"), &up, ALL), (vec![3], 1));
    assert_eq!(heights(&readers, by_id("0000000"), &up, ALL).1, 8);
    assert_eq!(heights(&readers, by_id("0000000_"), &up, ALL).1, 8);
    assert_eq!(heights(&readers, by_id("000000%3"), &up, ALL), (vec![3], 1));
    assert_eq!(heights(&readers, by_id("00000003%"), &up, ALL).0, [3]);
    let whole = block_id(5).to_hex();
    assert_eq!(heights(&readers, by_id(&whole), &up, ALL).0, [5]);
    // Two ids are two alternatives.
    let two = BlockCriteria {
        id: vec!["00000002".into(), "00000007".into()],
        ..BlockCriteria::default()
    };
    assert_eq!(heights(&readers, two, &up, ALL).0, [2, 7]);
}

#[test]
fn block_criteria_combine_and_orders_and_pages_hold() {
    let (_database, readers) = store("listings-criteria");
    let criteria = BlockCriteria {
        height: vec![Numeric::Range {
            from: Some(2),
            to: Some(7),
        }],
        username: vec!["alice".into(), "bob".into()],
        ..BlockCriteria::default()
    };
    let down = order("height", Direction::Desc);
    assert_eq!(
        heights(&readers, criteria.clone(), &down, ALL),
        (vec![6, 5, 4, 3, 2], 5)
    );
    let page = Page {
        offset: 1,
        limit: 2,
    };
    assert_eq!(
        heights(&readers, criteria.clone(), &down, page),
        (vec![5, 4], 5)
    );
    // Ties are broken by height in the direction of the last key.
    let by_name = order("username", Direction::Desc);
    assert_eq!(
        heights(&readers, criteria, &by_name, ALL).0,
        [6, 4, 5, 3, 2]
    );
    // The names' nulls come last ascending and first descending.
    let all = BlockCriteria::default();
    let up = order("username", Direction::Asc);
    assert_eq!(heights(&readers, all.clone(), &up, ALL).0[7], 1);
    assert_eq!(heights(&readers, all, &by_name, ALL).0[0], 1);
    // An order on something that is no column finds nothing, and keeps the flag it was given.
    let nothing = readers
        .get()
        .blocks(
            &BlockFilter::default(),
            &order("nope", Direction::Asc),
            ALL,
            false,
            CountFlag::Estimate,
        )
        .unwrap();
    assert!(nothing.rows.is_empty() && nothing.total_count_is_estimate);
}

#[test]
fn the_core_transfer_join_sums_each_blocks_payments() {
    let (_database, readers) = store("listings-transfers");
    let listing = |transfers| {
        readers
            .get()
            .blocks(
                &BlockFilter::default(),
                &order("height", Direction::Asc),
                ALL,
                transfers,
                CountFlag::Exact,
            )
            .unwrap()
    };
    let with: Vec<_> = listing(true).rows.iter().map(|b| b.transferred).collect();
    let expected = [1500, 0, 30, 5, 0, 7, 0, 1].map(Some);
    assert_eq!(with, expected);
    assert!(listing(false).rows.iter().all(|b| b.transferred.is_none()));
}

#[test]
fn missed_block_criteria_and_orders() {
    let (_database, readers) = store("listings-missed");
    let listed = |criteria: MissedBlockCriteria, order: &[Order]| {
        let listing = readers
            .get()
            .missed_blocks(&MissedBlockFilter::of(criteria), order, ALL)
            .unwrap();
        (
            listing.rows.iter().map(|r| r.height).collect::<Vec<_>>(),
            listing.total_count,
        )
    };
    let up = order("timestamp", Direction::Asc);
    let bob = MissedBlockCriteria {
        username: vec!["bob".into()],
        ..MissedBlockCriteria::default()
    };
    assert_eq!(listed(bob, &up), (vec![4, 5], 2));
    let ranged = MissedBlockCriteria {
        timestamp: vec![Numeric::Range {
            from: Some(450),
            to: Some(650),
        }],
        height: vec![Numeric::Range {
            from: None,
            to: Some(6),
        }],
        ..MissedBlockCriteria::default()
    };
    assert_eq!(
        listed(ranged, &order("timestamp", Direction::Desc)),
        (vec![6, 5], 2)
    );
    // Without a criterion every slot matches, and ties of height fall to the timestamp.
    let none = MissedBlockCriteria::default();
    assert_eq!(
        listed(none.clone(), &order("height", Direction::Desc)).0,
        [7, 6, 5, 4]
    );
    assert_eq!(listed(none, &order("nope", Direction::Asc)), (vec![], 0));
}

#[test]
fn productivity_counts_each_side_from_the_window_start() {
    let (_database, readers) = store("listings-productivity");
    let rows = |since| -> Vec<(String, Option<u64>, Option<u64>)> {
        readers
            .get()
            .productivity(since)
            .unwrap()
            .into_iter()
            .map(|row| (row.username, row.produced, row.missed))
            .collect()
    };
    let name = |n: &str, p, m| (n.to_owned(), p, m);
    // Registered names only, in name order; a name with neither side has nothing.
    assert_eq!(
        rows(200),
        [
            name("alice", Some(4), None),
            name("bob", Some(2), Some(2)),
            name("dave", None, Some(1)),
            name("frank", None, None),
        ]
    );
    // A block stamped exactly at the start counts; one second later it does not.
    assert_eq!(rows(300)[0], name("alice", Some(3), None));
    assert_eq!(rows(301)[0], name("alice", Some(2), None));
    // The same for a missed slot.
    assert_eq!(rows(450)[1], name("bob", Some(1), Some(1)));
    assert_eq!(rows(451)[1], name("bob", Some(1), None));
    // The genesis block has no name: while the window reaches it the reference's division fails.
    assert!(readers.get().productivity(100).is_err());
    assert!(readers.get().productivity(101).is_ok());
}

#[test]
fn productivity_rounds_half_up_on_exact_quotients() {
    assert_eq!(hundredths(4, 0), Some(10_000));
    assert_eq!(hundredths(0, 5), Some(0));
    assert_eq!(hundredths(0, 0), None);
    // 3 of 4,000 is 0.075 percent, which ROUND writes as 0.08; a float division gives 0.07.
    assert_eq!(hundredths(3, 3_997), Some(8));
    assert_eq!(hundredths(1, 1), Some(5_000));
    assert_eq!(hundredths(1, 2), Some(3_333));
    assert_eq!(hundredths(2, 1), Some(6_667));
    assert_eq!(round2("0.075").as_deref(), Some("0.08"));
    assert_eq!(round2("-0.075").as_deref(), Some("-0.08"));
    assert_eq!(round2("2.675").as_deref(), Some("2.68"));
    assert_eq!(round2("0.004").as_deref(), Some("0.00"));
    assert_eq!(round2("-0.004").as_deref(), Some("0.00"));
    assert_eq!(round2("7").as_deref(), Some("7.00"));
    assert_eq!(round2("99.995").as_deref(), Some("100.00"));
    assert_eq!(round2("1e3"), None);
}

#[test]
fn averages_round_half_away_from_zero() {
    assert_eq!(avg_int8(3, 2), Some(2));
    assert_eq!(avg_int8(-3, 2), Some(-2));
    assert_eq!(avg_int8(5, 4), Some(1));
    assert_eq!(avg_int8(7, 4), Some(2));
    assert_eq!(avg_int8(0, 0), None);
    assert_eq!(round_div(1, 0), None);
    assert_eq!(round_div(-7, 2), Some(-4));
}

#[test]
fn fee_statistics_over_a_window_and_over_the_last_fees() {
    let (_database, readers) = store("listings-fees");
    let reader = readers.get();
    // Blocks 3 and 4 hold two transfers, of fees 1 and 2 (an average of 1.5, which is 2), and a
    // burn of fee 3.
    let rows = reader.fee_statistics(Some((300, 499))).unwrap();
    let stats: Vec<_> = rows
        .iter()
        .map(|r| (r.type_group, r.kind, r.avg, r.min, r.max, r.sum, r.burned))
        .collect();
    assert_eq!(stats, [(1, 6, 2, 1, 2, 3, 1), (2, 0, 3, 3, 3, 3, 1)]);
    // Blocks 4 and 5 hold burns of fees 3 and 4: 3.5 is 4.
    let burns = reader.fee_statistics(Some((400, 599))).unwrap();
    assert_eq!(burns.iter().find(|r| r.type_group == 2).unwrap().avg, 4);
    assert!(reader.fee_statistics(Some((900, 999))).unwrap().is_empty());
    // Without a window: one row per kept type, in the registry's order, zeros for a type with none.
    let last = reader.fee_statistics(None).unwrap();
    assert_eq!(last.len(), TransactionType::ALL.len());
    let of = |kind: TransactionType| {
        last.iter()
            .zip(TransactionType::ALL)
            .find(|(_, k)| *k == kind)
            .unwrap()
            .0
    };
    let transfer = of(TransactionType::Transfer);
    // Fees 0, 1, 2, 2, 4 average 1.8, which is 2; the burned sum covers every transfer.
    assert_eq!(
        (
            transfer.avg,
            transfer.min,
            transfer.max,
            transfer.sum,
            transfer.burned
        ),
        (2, 0, 4, 9, 4)
    );
    let signature = of(TransactionType::SecondSignature);
    assert_eq!(
        (
            signature.avg,
            signature.min,
            signature.max,
            signature.sum,
            signature.burned
        ),
        (0, 0, 0, 0, 0)
    );
}

#[test]
fn burned_totals_add_over_ranges() {
    let (_database, readers) = store("listings-burned");
    let reader = readers.get();
    let burned = |above, to| {
        let totals = reader.burned_totals(above, to).unwrap();
        (totals.fees, totals.transactions)
    };
    assert_eq!(burned(0, 8), (27, 1000));
    assert_eq!(burned(0, 4), (22, 700));
    assert_eq!(burned(4, 5), (2, 300));
    assert_eq!(burned(5, 8), (3, 0));
    assert_eq!(burned(8, 8), (0, 0));
    // A range is the sum of its parts, so a tip that grows adds only the new blocks.
    let (a, b) = (burned(0, 4), burned(4, 8));
    assert_eq!((a.0 + b.0, a.1 + b.1), burned(0, 8));
    // The supply's parts: the rewards of a range, and what the genesis block issued.
    assert_eq!(reader.rewards(0, 8).unwrap(), 360);
    assert_eq!(reader.rewards(3, 5).unwrap(), 90);
    assert_eq!(reader.issued().unwrap(), 1500);
}
