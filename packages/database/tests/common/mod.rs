//! Helpers shared by the integration tests: scratch databases, milestones with short rounds, and
//! rows of a small consistent chain.

#![allow(dead_code)]

use std::path::PathBuf;

use heartwood_crypto::{BlockId, Milestones, Network, TransactionId};
use heartwood_database::{
    BlockRow, Chunk, Database, MissedRow, RoundRow, Settings, StateRow, TransactionRow,
};

/// The delegates of a round in [`milestones`].
pub const ROUND: u32 = 5;

/// An empty directory for the test `name`, under Cargo's scratch directory for tests.
pub fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("heartwood-database")
        .join(format!("{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A new database for the test `name`, and its settings.
pub fn open(name: &str) -> (Database, Settings) {
    let settings = Settings::in_data_dir(&scratch(name));
    (Database::open(&settings).unwrap(), settings)
}

/// Milestones with rounds of [`ROUND`] delegates.
pub fn milestones() -> Milestones {
    let network = Network {
        name: "testnet".into(),
        message_prefix: "testnet message:\n".into(),
        address_character: Some("t".into()),
        bip32_public: 70_617_039,
        bip32_private: 70_615_956,
        pub_key_hash: 90,
        nethash: "0".repeat(64),
        wif: 252,
        slip44: 1,
        token: "TEST".into(),
        symbol: "tT".into(),
        explorer: String::new(),
    };
    let text = format!(
        r#"[{{"height": 1, "activeDelegates": {ROUND}, "blockTime": 8,
            "block": {{"version": 0, "maxTransactions": 150, "maxPayload": 2097152}},
            "epoch": "2026-01-01T00:00:00.000Z", "reward": 0,
            "burn": {{"feeBasisPoints": 9000, "txAmount": 2000000}}, "donations": {{}},
            "dynamicReward": {{"enabled": false}}, "transfer": {{"maximum": 256, "minimum": 1}}}}]"#
    );
    Milestones::load(&text, &network).unwrap()
}

/// 32 bytes that differ for each `(kind, a, b)`.
fn bytes(kind: u8, a: u32, b: u32) -> [u8; 32] {
    let mut out = [kind; 32];
    out[..4].copy_from_slice(&a.to_be_bytes());
    out[4..8].copy_from_slice(&b.to_be_bytes());
    out
}

/// The id of the test block at `height`.
pub fn block_id(height: u32) -> BlockId {
    BlockId::from_array(bytes(0xb1, height, 0))
}

/// The id of transaction `sequence` of the test block at `height`.
pub fn transaction_id(height: u32, sequence: u32) -> TransactionId {
    TransactionId::from_array(bytes(0x71, height, sequence))
}

/// The row of the test block at `height` with `transactions` transactions of amount 3 and fee 2
/// each.
pub fn block_row(height: u32, transactions: u32) -> BlockRow {
    BlockRow {
        id: block_id(height),
        height,
        version: 0,
        timestamp: height * 8,
        previous_block: Some(if height == 1 {
            BlockId::from_array([0; 32])
        } else {
            block_id(height - 1)
        }),
        number_of_transactions: transactions,
        total_amount: 3 * u64::from(transactions),
        total_fee: 2 * u64::from(transactions),
        burned_fee: u64::from(transactions),
        reward: 0,
        donations: "{}".into(),
        payload_length: 0,
        payload_hash: "ab".repeat(32),
        generator_public_key: format!("02{}", "cd".repeat(32)),
        username: (height > 1).then(|| format!("delegate{}", height % ROUND)),
        block_signature: "ef".repeat(64),
    }
}

/// Transaction `sequence` of the test block at `height`.
pub fn transaction_row(height: u32, sequence: u32) -> TransactionRow {
    TransactionRow {
        id: transaction_id(height, sequence),
        version: 3,
        block_id: block_id(height),
        block_height: height,
        sequence,
        timestamp: height * 8,
        nonce: u64::from(height) * 1000 + u64::from(sequence),
        sender_id: "TSender".into(),
        sender_public_key: format!("03{}", "11".repeat(32)),
        recipient_id: Some("TRecipient".into()),
        kind: 0,
        type_group: 1,
        memo: Some(b"memo".to_vec()),
        amount: 3,
        fee: 2,
        burned_fee: 1,
        serialised: vec![0xff, height as u8, sequence as u8],
        asset: None,
        recipients: vec!["TRecipient".into()],
    }
}

/// The block at `height`, its transactions, the undo record, the state row, the round rows when
/// `height` ends a round or is 1, and one missed row, as a commit writes them.
pub fn write_block(chunk: &mut Chunk<'_>, height: u32, transactions: u32) {
    let rows: Vec<TransactionRow> = (0..transactions)
        .map(|sequence| transaction_row(height, sequence))
        .collect();
    chunk
        .save_block(&block_row(height, transactions), &rows)
        .unwrap();
    if height > 1 {
        chunk.put_undo(height, &format!("undo {height}")).unwrap();
        chunk
            .save_missed(&[MissedRow {
                timestamp: i64::from(height) * 8 - 4,
                height,
                username: "late".into(),
            }])
            .unwrap();
    }
    if height == 1 || height.is_multiple_of(ROUND) {
        let round = u64::from(height / ROUND + 1);
        chunk.save_round(&round_rows(round)).unwrap();
    }
    chunk
        .put_state(&StateRow {
            height,
            block_id: block_id(height),
            record: format!("state {height}"),
        })
        .unwrap();
}

/// The rows of `round`: [`ROUND`] delegates, in the order `Database::round` reads them.
pub fn round_rows(round: u64) -> Vec<RoundRow> {
    (0..u64::from(ROUND))
        .map(|index| RoundRow {
            round,
            public_key: format!("02{index:064x}"),
            balance: 200 - index,
        })
        .collect()
}

/// A database with the blocks 1 to `tip`, one chunk per block.
pub fn chain(name: &str, tip: u32) -> (Database, Settings) {
    let (mut database, settings) = open(name);
    for height in 1..=tip {
        let mut chunk = database.begin().unwrap();
        write_block(&mut chunk, height, height % 3);
        chunk.commit().unwrap();
    }
    (database, settings)
}

/// A second connection to the file of `settings`, without foreign key checks: what a program
/// that changes the file outside the node does.
pub fn tamper(settings: &Settings) -> rusqlite::Connection {
    let connection = rusqlite::Connection::open(&settings.path).unwrap();
    connection
        .pragma_update(None, "foreign_keys", "OFF")
        .unwrap();
    connection
}
