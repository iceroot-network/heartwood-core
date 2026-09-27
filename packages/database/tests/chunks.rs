//! A chunk is one transaction: dropped, it leaves no row; a block's savepoint rolled back leaves
//! the chunk's other blocks. A revert deletes exactly the rows above the new tip. A reset empties
//! every table.

mod common;

use common::{ROUND, block_id, block_row, chain, open, round_rows, write_block};
use heartwood_database::{
    ChainMeta, Database, DelegateVersionRow, MilestoneEntryRow, MissedRow, RoundRow, Settings,
};

/// Every table, in the order of [`counts`].
const TABLES: [&str; 10] = [
    "meta",
    "blocks",
    "transactions",
    "rounds",
    "missed_blocks",
    "wallets",
    "state",
    "undo",
    "milestone_entries",
    "delegate_versions",
];

/// The number of rows of every table.
fn counts(settings: &Settings) -> Vec<(&'static str, i64)> {
    let connection = rusqlite::Connection::open(&settings.path).unwrap();
    TABLES
        .into_iter()
        .map(|table| {
            let count = connection
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap();
            (table, count)
        })
        .collect()
}

fn empty() -> Vec<(&'static str, i64)> {
    TABLES.into_iter().map(|table| (table, 0)).collect()
}

#[test]
fn a_dropped_chunk_leaves_no_row() {
    let (mut database, settings) = open("chunks-dropped");
    {
        let mut chunk = database.begin().unwrap();
        write_block(&mut chunk, 1, 2);
        write_block(&mut chunk, 2, 1);
        chunk.put_wallet("TA", "a").unwrap();
        chunk
            .put_meta(&ChainMeta {
                schema_version: 1,
                nethash: "00".repeat(32),
                genesis_id: block_id(1),
                state_format: 1,
            })
            .unwrap();
        chunk
            .put_milestone_entry(&MilestoneEntryRow {
                height: 1,
                digest: [1; 32],
            })
            .unwrap();
        // Dropped without a commit.
    }
    assert_eq!(counts(&settings), empty());

    // The connection is usable afterwards, and a reopened file holds nothing either.
    let mut chunk = database.begin().unwrap();
    write_block(&mut chunk, 1, 0);
    drop(chunk);
    drop(database);
    let database = Database::open(&settings).unwrap();
    assert_eq!(database.last_block().unwrap(), None);
    assert_eq!(counts(&settings), empty());
}

#[test]
fn a_rolled_back_savepoint_leaves_the_other_blocks_of_the_chunk() {
    let (mut database, settings) = open("chunks-savepoint");
    let mut chunk = database.begin().unwrap();
    for height in 1..=2 {
        let mut savepoint = chunk.savepoint().unwrap();
        write_block(&mut savepoint, height, 2);
        savepoint.commit().unwrap();
    }
    // Block 3 is refused halfway: explicitly rolled back.
    let mut savepoint = chunk.savepoint().unwrap();
    write_block(&mut savepoint, 3, 2);
    savepoint.put_wallet("TA", "refused").unwrap();
    savepoint.rollback().unwrap();
    // Block 3 again, refused by dropping its savepoint.
    {
        let mut savepoint = chunk.savepoint().unwrap();
        write_block(&mut savepoint, 3, 1);
    }
    // Block 3 accepted, with a nested savepoint rolled back inside it.
    let mut savepoint = chunk.savepoint().unwrap();
    write_block(&mut savepoint, 3, 0);
    {
        let mut inner = savepoint.savepoint().unwrap();
        inner.put_wallet("TB", "inner").unwrap();
    }
    savepoint.put_wallet("TC", "kept").unwrap();
    savepoint.commit().unwrap();
    chunk.commit().unwrap();

    let blocks = database.blocks(1, 9).unwrap();
    assert_eq!(
        blocks
            .iter()
            .map(|block| block.row.clone())
            .collect::<Vec<_>>(),
        [block_row(1, 2), block_row(2, 2), block_row(3, 0)]
    );
    assert_eq!(blocks[2].transactions, []);
    let wallets = database.wallets().unwrap();
    assert_eq!(wallets.len(), 1);
    assert_eq!(
        (wallets[0].address.as_str(), wallets[0].record.as_str()),
        ("TC", "kept")
    );
    assert_eq!(database.state_row().unwrap().unwrap().height, 3);
    assert!(database.integrity(&common::milestones()).unwrap().is_ok());
    assert_eq!(counts(&settings)[2], ("transactions", 4));
}

#[test]
fn delete_above_removes_exactly_the_rows_above_the_new_tip() {
    // Blocks 1 to 12: round rows of rounds 1, 2 (saved at 5) and 3 (saved at 10).
    let (mut database, settings) = chain("chunks-delete", 12);
    let mut chunk = database.begin().unwrap();
    for height in [1, 6, 11, 12] {
        chunk
            .put_milestone_entry(&MilestoneEntryRow {
                height,
                digest: [height as u8; 32],
            })
            .unwrap();
    }
    for (address, record) in [("TA", "a"), ("TB", "b")] {
        chunk.put_wallet(address, record).unwrap();
    }
    chunk.commit().unwrap();
    database
        .put_delegate_versions(&[DelegateVersionRow {
            public_key: "02aa".into(),
            round: 3,
            version: "4.3.1".into(),
        }])
        .unwrap();
    let before = counts(&settings);
    let wallets = database.wallets().unwrap();
    let state = database.state_row().unwrap();

    // Revert to height 8, in round 2.
    let mut chunk = database.begin().unwrap();
    chunk.delete_above(8, 2).unwrap();
    chunk.commit().unwrap();

    let heights: Vec<u32> = database
        .blocks(1, 99)
        .unwrap()
        .iter()
        .map(|block| block.row.height)
        .collect();
    assert_eq!(heights, (1..=8).collect::<Vec<_>>());
    let transactions: i64 = (1..=8).map(|height| i64::from(height % 3)).sum();
    assert_eq!(counts(&settings)[2], ("transactions", transactions));
    assert_eq!(
        database
            .undo_records()
            .unwrap()
            .iter()
            .map(|row| row.height)
            .collect::<Vec<_>>(),
        (2..=8).collect::<Vec<_>>()
    );
    assert_eq!(
        database
            .missed_blocks()
            .unwrap()
            .iter()
            .map(|row| row.height)
            .collect::<Vec<_>>(),
        (2..=8).collect::<Vec<_>>()
    );
    assert_eq!(
        database
            .milestone_entries()
            .unwrap()
            .iter()
            .map(|row| row.height)
            .collect::<Vec<_>>(),
        [1, 6]
    );
    assert_eq!(database.round(1).unwrap(), round_rows(1));
    assert_eq!(database.round(2).unwrap(), round_rows(2));
    assert_eq!(database.round(3).unwrap(), []);
    // Untouched: the wallets, the state row (the commit writes them), meta, the node-local rows.
    assert_eq!(database.wallets().unwrap(), wallets);
    assert_eq!(database.state_row().unwrap(), state);
    assert_eq!(database.delegate_versions().unwrap().len(), 1);
    let after = counts(&settings);
    for index in [0, 5, 6, 9] {
        assert_eq!(after[index], before[index]);
    }

    // A revert to a round's last block keeps that round's rows only: the round rows above the
    // round passed in go.
    let mut chunk = database.begin().unwrap();
    chunk.delete_above(5, 1).unwrap();
    chunk.commit().unwrap();
    assert_eq!(database.last_block().unwrap().unwrap().row.height, 5);
    assert_eq!(database.round(2).unwrap(), []);
    assert_eq!(database.round(1).unwrap(), round_rows(1));

    // Nothing above the tip: nothing changes.
    let before = counts(&settings);
    let mut chunk = database.begin().unwrap();
    chunk.delete_above(5, 1).unwrap();
    chunk.commit().unwrap();
    assert_eq!(counts(&settings), before);
}

#[test]
fn round_rows_are_read_by_balance_then_by_key_bytes() {
    let (mut database, _) = open("chunks-round-order");
    let row = |key: &str, balance| RoundRow {
        round: 7,
        public_key: key.into(),
        balance,
    };
    // Equal balances order by the key's bytes: digits before letters, shorter prefix first.
    let stored = [
        row("03a0", 50),
        row("02ff", 50),
        row("02a0", 50),
        row("0290", 50),
        row("029", 50),
        row("03", 90),
        row("02", 10),
    ];
    let mut chunk = database.begin().unwrap();
    chunk.save_round(&stored).unwrap();
    chunk.save_round(&[row("02", 20)]).unwrap();
    chunk.commit().unwrap();
    let keys: Vec<(String, u64)> = database
        .round(7)
        .unwrap()
        .into_iter()
        .map(|row| (row.public_key, row.balance))
        .collect();
    let expected = [
        ("03", 90),
        ("029", 50),
        ("0290", 50),
        ("02a0", 50),
        ("02ff", 50),
        ("03a0", 50),
        ("02", 20),
    ];
    assert_eq!(
        keys,
        expected
            .iter()
            .map(|(key, balance)| (key.to_string(), *balance))
            .collect::<Vec<_>>()
    );
}

#[test]
fn a_reset_empties_every_table() {
    let (mut database, settings) = chain("chunks-reset", 7);
    let mut chunk = database.begin().unwrap();
    chunk.put_wallet("TA", "a").unwrap();
    chunk
        .put_meta(&ChainMeta {
            schema_version: 1,
            nethash: "00".repeat(32),
            genesis_id: block_id(1),
            state_format: 1,
        })
        .unwrap();
    chunk
        .put_milestone_entry(&MilestoneEntryRow {
            height: 1,
            digest: [1; 32],
        })
        .unwrap();
    chunk
        .save_missed(&[MissedRow {
            timestamp: 1,
            height: 1,
            username: "x".into(),
        }])
        .unwrap();
    chunk.commit().unwrap();
    database
        .put_delegate_versions(&[DelegateVersionRow {
            public_key: "02".into(),
            round: 1,
            version: "1".into(),
        }])
        .unwrap();
    assert!(counts(&settings).iter().all(|(_, count)| *count > 0));

    database.reset().unwrap();
    assert_eq!(counts(&settings), empty());
    drop(database);

    // The file is still a database of schema version 1, ready for a first start.
    let mut database = Database::open(&settings).unwrap();
    let mut chunk = database.begin().unwrap();
    write_block(&mut chunk, 1, 1);
    chunk.commit().unwrap();
    assert_eq!(database.last_block().unwrap().unwrap().row.height, 1);
    assert_eq!(database.round(1).unwrap().len(), ROUND as usize);
}
