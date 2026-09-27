//! Every row type is read back as it was written, amounts from 0 to `i64::MAX`; a value above
//! `i64::MAX` is refused before anything is written, and a stored value that does not fit its row
//! is a corrupt row.

mod common;

use common::{block_id, block_row, open, tamper, transaction_id, transaction_row};
use heartwood_crypto::BlockId;
use heartwood_database::{
    ChainMeta, DatabaseError, DelegateVersionRow, MilestoneEntryRow, MissedRow, RoundRow, StateRow,
    StoredBlock, UndoRow, WalletRow,
};

const MAX: u64 = i64::MAX as u64;

#[test]
fn blocks_and_transactions_are_read_back_as_written() {
    let (mut database, _) = open("rows-blocks");
    let mut blocks = Vec::new();
    for (height, amount) in [(1, 0), (2, 1), (3, MAX)] {
        let mut block = block_row(height, 2);
        block.total_amount = amount;
        block.total_fee = amount;
        block.burned_fee = amount;
        block.reward = amount;
        block.donations = format!(r#"{{"TDonee":"{amount}"}}"#);
        let mut transactions = vec![transaction_row(height, 0), transaction_row(height, 1)];
        transactions[0].amount = amount;
        transactions[0].fee = amount;
        transactions[0].burned_fee = amount;
        transactions[1].nonce = MAX - u64::from(height);
        transactions[1].memo = None;
        transactions[1].recipient_id = None;
        transactions[1].asset = Some(r#"{"votes":{"delegate":"100.00"}}"#.into());
        let mut chunk = database.begin().unwrap();
        chunk.save_block(&block, &transactions).unwrap();
        chunk.commit().unwrap();
        blocks.push((block, transactions));
    }

    let stored: Vec<StoredBlock> = blocks
        .iter()
        .map(|(block, transactions)| StoredBlock {
            row: block.clone(),
            transactions: transactions
                .iter()
                .map(|t| (t.id, t.serialised.clone()))
                .collect(),
        })
        .collect();
    assert_eq!(database.blocks(1, 3).unwrap(), stored);
    assert_eq!(database.blocks(2, 2).unwrap(), stored[1..2]);
    assert_eq!(database.blocks(4, 9).unwrap(), []);
    assert_eq!(database.last_block().unwrap().as_ref(), stored.last());
    assert_eq!(
        database.block_by_id(&block_id(2)).unwrap().as_ref(),
        stored.get(1)
    );
    assert_eq!(database.block_by_id(&block_id(7)).unwrap(), None);
    let top: Vec<StoredBlock> = stored.iter().rev().take(2).cloned().collect();
    assert_eq!(database.top_blocks(2).unwrap(), top);
    for (block, transactions) in &blocks {
        assert_eq!(&database.transaction_rows(&block.id).unwrap(), transactions);
    }

    let asked = [
        transaction_id(3, 1),
        transaction_id(9, 0),
        transaction_id(1, 0),
        transaction_id(3, 1),
    ];
    assert_eq!(
        database.forged_transaction_ids(&asked).unwrap(),
        [asked[0], asked[2], asked[3]]
    );
}

#[test]
fn transactions_are_read_by_sequence_not_by_insertion() {
    let (mut database, _) = open("rows-sequence");
    let transactions = [
        transaction_row(1, 2),
        transaction_row(1, 0),
        transaction_row(1, 1),
    ];
    let mut chunk = database.begin().unwrap();
    chunk.save_block(&block_row(1, 3), &transactions).unwrap();
    chunk.commit().unwrap();
    let ids: Vec<_> = database.blocks(1, 1).unwrap()[0]
        .transactions
        .iter()
        .map(|(id, _)| *id)
        .collect();
    assert_eq!(
        ids,
        [
            transaction_id(1, 0),
            transaction_id(1, 1),
            transaction_id(1, 2)
        ]
    );
}

#[test]
fn an_amount_above_i64_max_is_refused_and_nothing_is_written() {
    let (mut database, _) = open("rows-range");
    let over = MAX + 1;

    let mut block = block_row(1, 0);
    block.total_fee = over;
    let mut chunk = database.begin().unwrap();
    assert_eq!(
        chunk.save_block(&block, &[]),
        Err(DatabaseError::Range {
            column: "blocks.total_fee",
            value: u128::from(over),
        })
    );
    let mut transaction = transaction_row(1, 0);
    transaction.nonce = u64::MAX;
    assert_eq!(
        chunk.save_block(&block_row(1, 1), &[transaction]),
        Err(DatabaseError::Range {
            column: "transactions.nonce",
            value: u128::from(u64::MAX),
        })
    );
    let row = RoundRow {
        round: 1,
        public_key: "02".into(),
        balance: over,
    };
    assert!(matches!(
        chunk.save_round(&[row]),
        Err(DatabaseError::Range {
            column: "rounds.balance",
            ..
        })
    ));
    drop(chunk);
    assert_eq!(database.last_block().unwrap(), None);

    let row = DelegateVersionRow {
        public_key: "02".into(),
        round: over,
        version: "4.3.1".into(),
    };
    assert!(matches!(
        database.put_delegate_versions(&[row]),
        Err(DatabaseError::Range { .. })
    ));
    assert_eq!(database.delegate_versions().unwrap(), []);
}

#[test]
fn rounds_missed_rows_and_node_local_rows_are_read_back() {
    let (mut database, _) = open("rows-rounds");
    let rounds = [
        RoundRow {
            round: 1,
            public_key: "02aa".into(),
            balance: MAX,
        },
        RoundRow {
            round: 1,
            public_key: "02bb".into(),
            balance: 0,
        },
    ];
    let missed = [
        MissedRow {
            timestamp: -8,
            height: 2,
            username: "one".into(),
        },
        MissedRow {
            timestamp: 16,
            height: 3,
            username: "two".into(),
        },
    ];
    let mut chunk = database.begin().unwrap();
    chunk.save_round(&rounds).unwrap();
    chunk
        .save_missed(&[missed[1].clone(), missed[0].clone()])
        .unwrap();
    chunk.commit().unwrap();
    assert_eq!(database.round(1).unwrap(), rounds);
    assert_eq!(database.round(2).unwrap(), []);
    assert_eq!(database.missed_blocks().unwrap(), missed);

    // A row saved again replaces the stored one, as the reference's save does.
    let mut chunk = database.begin().unwrap();
    let changed = RoundRow {
        balance: 5,
        ..rounds[0].clone()
    };
    chunk.save_round(std::slice::from_ref(&changed)).unwrap();
    let late = MissedRow {
        username: "three".into(),
        ..missed[0].clone()
    };
    chunk.save_missed(std::slice::from_ref(&late)).unwrap();
    chunk.commit().unwrap();
    assert_eq!(database.round(1).unwrap(), [changed, rounds[1].clone()]);
    assert_eq!(database.missed_blocks().unwrap(), [late, missed[1].clone()]);

    let versions = [
        DelegateVersionRow {
            public_key: "03bb".into(),
            round: MAX,
            version: "4.3.1".into(),
        },
        DelegateVersionRow {
            public_key: "02aa".into(),
            round: 0,
            version: "4.3.0-next.1".into(),
        },
    ];
    database.put_delegate_versions(&versions).unwrap();
    assert_eq!(
        database.delegate_versions().unwrap(),
        [versions[1].clone(), versions[0].clone()]
    );
    database.put_delegate_versions(&versions[..1]).unwrap();
    assert_eq!(database.delegate_versions().unwrap(), versions[..1]);
}

#[test]
fn the_state_tables_meta_and_milestone_entries_are_read_back() {
    let (mut database, settings) = open("rows-state");
    assert_eq!(database.meta().unwrap(), None);
    assert_eq!(database.state_row().unwrap(), None);
    let meta = ChainMeta {
        schema_version: 1,
        nethash: "ab".repeat(32),
        genesis_id: block_id(1),
        state_format: 7,
    };
    let state = StateRow {
        height: 3,
        block_id: block_id(3),
        record: r#"{"format":1}"#.into(),
    };
    let entries = [
        MilestoneEntryRow {
            height: 1,
            digest: [0; 32],
        },
        MilestoneEntryRow {
            height: 40,
            digest: [0xff; 32],
        },
    ];

    let mut chunk = database.begin().unwrap();
    for height in 1..=3 {
        chunk.save_block(&block_row(height, 0), &[]).unwrap();
    }
    chunk.put_meta(&meta).unwrap();
    chunk.put_state(&state).unwrap();
    for (address, record) in [("TC", "c"), ("TA", "a"), ("TB", "b")] {
        chunk.put_wallet(address, record).unwrap();
    }
    chunk.put_undo(3, "three").unwrap();
    chunk.put_undo(2, "two").unwrap();
    chunk.put_milestone_entry(&entries[1]).unwrap();
    chunk.put_milestone_entry(&entries[0]).unwrap();
    chunk.commit().unwrap();

    assert_eq!(database.meta().unwrap(), Some(meta.clone()));
    assert_eq!(database.state_row().unwrap(), Some(state));
    assert_eq!(database.milestone_entries().unwrap(), entries);
    let wallet = |address: &str, record: &str| WalletRow {
        address: address.into(),
        record: record.into(),
    };
    assert_eq!(
        database.wallets().unwrap(),
        [wallet("TC", "c"), wallet("TA", "a"), wallet("TB", "b")]
    );
    let undo = |height, record: &str| UndoRow {
        height,
        record: record.into(),
    };
    assert_eq!(
        database.undo_records().unwrap(),
        [undo(2, "two"), undo(3, "three")]
    );

    // A changed wallet keeps its place; a deleted one leaves it; a new one goes last.
    let mut chunk = database.begin().unwrap();
    chunk.put_wallet("TC", "c2").unwrap();
    chunk.delete_wallet("TA").unwrap();
    chunk.delete_wallet("TZ").unwrap();
    chunk.put_wallet("TD", "d").unwrap();
    chunk.put_undo(3, "three again").unwrap();
    let state = StateRow {
        height: 2,
        block_id: block_id(2),
        record: "{}".into(),
    };
    chunk.put_state(&state).unwrap();
    chunk.commit().unwrap();
    assert_eq!(
        database.wallets().unwrap(),
        [wallet("TC", "c2"), wallet("TB", "b"), wallet("TD", "d")]
    );
    assert_eq!(database.state_row().unwrap(), Some(state));
    assert_eq!(database.undo_records().unwrap()[1], undo(3, "three again"));

    let mut chunk = database.begin().unwrap();
    chunk.delete_wallets().unwrap();
    chunk.commit().unwrap();
    assert_eq!(database.wallets().unwrap(), []);

    // A meta table with a key missing is a corrupt row, not an absent identity.
    tamper(&settings)
        .execute("DELETE FROM meta WHERE key = 'nethash'", [])
        .unwrap();
    assert!(matches!(
        database.meta(),
        Err(DatabaseError::CorruptRow { table: "meta", .. })
    ));
}

#[test]
fn a_stored_value_outside_its_row_type_is_a_corrupt_row() {
    let (mut database, settings) = open("rows-corrupt");
    let mut chunk = database.begin().unwrap();
    chunk
        .save_block(&block_row(1, 1), &[transaction_row(1, 0)])
        .unwrap();
    chunk.commit().unwrap();
    let corrupt =
        |table: &'static str, sql: &str, check: &dyn Fn() -> Result<(), DatabaseError>| {
            tamper(&settings).execute_batch(sql).unwrap();
            match check() {
                Err(DatabaseError::CorruptRow { table: found, .. }) => {
                    assert_eq!(found, table, "{sql}")
                }
                other => panic!("{sql}: {other:?}"),
            }
        };
    let blocks = || database.blocks(1, 1).map(drop);
    corrupt("blocks", "UPDATE blocks SET total_fee = -1", &blocks);
    corrupt(
        "blocks",
        "UPDATE blocks SET total_fee = 0, version = 4294967296",
        &blocks,
    );
    corrupt(
        "blocks",
        "UPDATE blocks SET version = 0, id = 'b1'",
        &blocks,
    );
    let id = block_id(1).to_hex();
    corrupt(
        "transactions",
        &format!("UPDATE blocks SET id = '{id}'; UPDATE transactions SET id = 'zz'"),
        &|| database.last_block().map(drop),
    );
    corrupt(
        "transactions",
        &format!(
            "UPDATE transactions SET id = '{}', version = 256",
            transaction_id(1, 0)
        ),
        &|| {
            database
                .transaction_rows(&BlockId::from_hex(&id).unwrap())
                .map(drop)
        },
    );
}
