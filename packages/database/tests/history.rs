//! The history queries of the served routes on the API's read-only connections: the transaction
//! criteria with the reference's semantics, orders and pages, blocks by id and height, a block's
//! transactions, a round; and the upgrade of a store of schema version 1, which fills the
//! recipients side table the address criteria read.

mod common;

use heartwood_crypto::{BlockId, TransactionId};
use heartwood_database::filters::asset_alternatives;
use heartwood_database::{
    BlockSelector, Database, Direction, Numeric, Order, Page, Readers, SCHEMA_VERSION, Settings,
    TransactionCriteria, TransactionFilter, TransactionRow,
};
use serde_json::json;

use common::{block_id, block_row, round_rows, scratch};

/// A transaction of the test chain: its block height and sequence, sender, type group and type,
/// top-level amount, fee, memo, asset and transfer recipients.
struct Spec {
    height: u32,
    sequence: u32,
    sender: &'static str,
    kind: (u32, u16),
    amount: u64,
    fee: u64,
    memo: Option<&'static str>,
    asset: Option<serde_json::Value>,
    recipients: &'static [&'static str],
}

/// The transactions of the test chain, in chain order.
fn specs() -> Vec<Spec> {
    let transfer = |items: &[(&str, &str)]| {
        let items: Vec<_> = items
            .iter()
            .map(|(amount, recipient)| json!({"amount": amount, "recipientId": recipient}))
            .collect();
        Some(json!({ "transfers": items }))
    };
    vec![
        Spec {
            height: 2,
            sequence: 0,
            sender: "TA",
            kind: (1, 6),
            amount: 0,
            fee: 100,
            memo: Some("hello"),
            asset: transfer(&[("10", "TB"), ("20", "TC")]),
            recipients: &["TB", "TC"],
        },
        Spec {
            height: 2,
            sequence: 1,
            sender: "TB",
            kind: (2, 2),
            amount: 0,
            fee: 200,
            memo: None,
            asset: Some(json!({"votes": {"alice": 60.5, "bob": 39.5}})),
            recipients: &[],
        },
        Spec {
            height: 3,
            sequence: 0,
            sender: "TC",
            kind: (1, 2),
            amount: 0,
            fee: 300,
            memo: Some("Hello%"),
            asset: Some(json!({"delegate": {"username": "carol"}})),
            recipients: &[],
        },
        Spec {
            height: 4,
            sequence: 0,
            sender: "TA",
            kind: (2, 0),
            amount: 500,
            fee: 400,
            memo: Some("x_y"),
            asset: None,
            recipients: &[],
        },
        Spec {
            height: 4,
            sequence: 1,
            sender: "TB",
            kind: (1, 6),
            amount: 0,
            fee: 100,
            memo: Some(r"a\b"),
            asset: transfer(&[("5", "TA"), ("6", "TA"), ("7", "TD")]),
            recipients: &["TA", "TD"],
        },
        Spec {
            height: 5,
            sequence: 0,
            sender: "TD",
            kind: (1, 7),
            amount: 0,
            fee: 250,
            memo: None,
            asset: None,
            recipients: &[],
        },
        Spec {
            height: 6,
            sequence: 0,
            sender: "TA",
            kind: (1, 1),
            amount: 0,
            fee: 500,
            memo: None,
            asset: Some(json!({"signature": {"publicKey": "02aa"}})),
            recipients: &[],
        },
    ]
}

/// The id of the transaction at `height` and `sequence`: distinct first bytes, so each prefix of
/// two hex digits names one transaction.
fn id(height: u32, sequence: u32) -> TransactionId {
    let mut bytes = [0x5a; 32];
    bytes[0] = u8::try_from(height * 16 + sequence).unwrap();
    TransactionId::from_array(bytes)
}

fn row(spec: &Spec) -> TransactionRow {
    TransactionRow {
        id: id(spec.height, spec.sequence),
        version: 3,
        block_id: block_id(spec.height),
        block_height: spec.height,
        sequence: spec.sequence,
        timestamp: spec.height * 8,
        nonce: u64::from(spec.height * 100 + spec.sequence),
        sender_id: spec.sender.into(),
        sender_public_key: format!("03{}", "0".repeat(60) + &hex(spec.sender)),
        recipient_id: None,
        kind: spec.kind.1,
        type_group: spec.kind.0,
        memo: spec.memo.map(|memo| memo.as_bytes().to_vec()),
        amount: spec.amount,
        fee: spec.fee,
        burned_fee: spec.fee / 2,
        serialised: vec![0xee, spec.height as u8, spec.sequence as u8],
        asset: spec.asset.as_ref().map(ToString::to_string),
        recipients: spec.recipients.iter().map(|r| (*r).to_owned()).collect(),
    }
}

/// A store with the blocks 1 to 6 of the test chain, round 1's rows, and its readers.
fn store(name: &str) -> (Database, Settings, Readers) {
    let settings = Settings::in_data_dir(&scratch(name));
    let mut database = Database::open(&settings).unwrap();
    let specs = specs();
    for height in 1..=6 {
        let rows: Vec<TransactionRow> = specs
            .iter()
            .filter(|spec| spec.height == height)
            .map(row)
            .collect();
        let count = u32::try_from(rows.len()).unwrap();
        let mut chunk = database.begin().unwrap();
        chunk.save_block(&block_row(height, count), &rows).unwrap();
        if height == 1 {
            chunk.save_round(&round_rows(1)).unwrap();
        }
        chunk.commit().unwrap();
    }
    let readers = Readers::open(&settings, 2).unwrap();
    (database, settings, readers)
}

/// The lowercase hex of `text`'s bytes.
fn hex(text: &str) -> String {
    text.bytes().map(|b| format!("{b:02x}")).collect()
}

const ALL: Page = Page {
    offset: 0,
    limit: 100,
};

fn default_order() -> Vec<Order> {
    vec![
        order("timestamp", Direction::Desc),
        order("sequence", Direction::Desc),
    ]
}

fn order(property: &str, direction: Direction) -> Order {
    Order {
        property: property.into(),
        direction,
    }
}

/// The (height, sequence) of each transaction `criteria` matches, in the default order.
fn matching(readers: &Readers, criteria: TransactionCriteria) -> Vec<(u32, u32)> {
    let filter = TransactionFilter::of(criteria);
    listed(readers, &filter, &default_order(), ALL)
}

fn listed(
    readers: &Readers,
    filter: &TransactionFilter,
    order: &[Order],
    page: Page,
) -> Vec<(u32, u32)> {
    let listing = readers.get().transactions(filter, order, page).unwrap();
    listing
        .rows
        .iter()
        .map(|record| (record.row.block_height, record.row.sequence))
        .collect()
}

fn criteria() -> TransactionCriteria {
    TransactionCriteria::default()
}

#[test]
fn a_listing_counts_every_match_and_returns_its_page() {
    let (_database, _settings, readers) = store("history-page");
    let all = matching(&readers, criteria());
    assert_eq!(
        all,
        [(6, 0), (5, 0), (4, 1), (4, 0), (3, 0), (2, 1), (2, 0)]
    );
    let page = Page {
        offset: 2,
        limit: 3,
    };
    let listing = readers
        .get()
        .transactions(&TransactionFilter::default(), &default_order(), page)
        .unwrap();
    assert_eq!(listing.total_count, 7);
    assert!(!listing.total_count_is_estimate);
    let rows: Vec<_> = listing.rows.iter().map(|r| r.row.block_height).collect();
    assert_eq!(rows, [4, 4, 3]);
    // A transfer's record carries its recipients.
    let transfer = &listing.rows[0].row;
    assert_eq!(transfer.recipients, ["TA", "TD"]);
    assert_eq!(transfer.memo.as_deref(), Some(&br"a\b"[..]));
}

#[test]
fn the_address_is_the_sender_or_a_recipient_of_the_row_or_a_transfer() {
    let (_database, _settings, readers) = store("history-address");
    let address = |values: &[&str]| TransactionCriteria {
        address: values.iter().map(|v| (*v).to_owned()).collect(),
        ..criteria()
    };
    assert_eq!(
        matching(&readers, address(&["TA"])),
        [(6, 0), (4, 1), (4, 0), (2, 0)]
    );
    assert_eq!(matching(&readers, address(&["TD"])), [(5, 0), (4, 1)]);
    assert_eq!(
        matching(&readers, address(&["TC", "TD"])),
        [(5, 0), (4, 1), (3, 0), (2, 0)]
    );
    assert_eq!(matching(&readers, address(&["TZ"])), []);
    let sender = |value: &str| TransactionCriteria {
        sender_id: vec![value.to_owned()],
        ..criteria()
    };
    assert_eq!(matching(&readers, sender("TB")), [(4, 1), (2, 1)]);
    let recipient = |value: &str| TransactionCriteria {
        recipient_id: vec![value.to_owned()],
        ..criteria()
    };
    assert_eq!(matching(&readers, recipient("TC")), [(2, 0)]);
    assert_eq!(matching(&readers, recipient("TA")), [(4, 1)]);
}

#[test]
fn a_type_without_a_type_group_means_type_group_1() {
    let (_database, _settings, readers) = store("history-type");
    let typed = |kind: &[u64], group: &[u64]| TransactionCriteria {
        kind: kind.to_vec(),
        type_group: group.to_vec(),
        ..criteria()
    };
    assert_eq!(matching(&readers, typed(&[6], &[])), [(4, 1), (2, 0)]);
    assert_eq!(matching(&readers, typed(&[0], &[])), []);
    assert_eq!(matching(&readers, typed(&[0], &[2])), [(4, 0)]);
    assert_eq!(matching(&readers, typed(&[], &[2])), [(4, 0), (2, 1)]);
    assert_eq!(
        matching(&readers, typed(&[0, 2], &[1, 2])),
        [(4, 0), (3, 0), (2, 1)]
    );
}

#[test]
fn the_memo_is_a_case_sensitive_pattern_on_its_bytes() {
    let (_database, _settings, readers) = store("history-memo");
    let memo = |patterns: &[&str]| TransactionCriteria {
        memo: patterns.iter().map(|p| p.as_bytes().to_vec()).collect(),
        ..criteria()
    };
    assert_eq!(matching(&readers, memo(&["hello"])), [(2, 0)]);
    assert_eq!(matching(&readers, memo(&["HELLO"])), []);
    assert_eq!(matching(&readers, memo(&["%ll%"])), [(3, 0), (2, 0)]);
    assert_eq!(matching(&readers, memo(&[r"Hello\%"])), [(3, 0)]);
    assert_eq!(matching(&readers, memo(&["x_y"])), [(4, 0)]);
    assert_eq!(matching(&readers, memo(&[r"x\_y", "a%"])), [(4, 1), (4, 0)]);
    assert_eq!(matching(&readers, memo(&[r"a\\b"])), [(4, 1)]);
    // A pattern whose trailing escape a memo reaches fails the whole read.
    let filter = TransactionFilter::of(memo(&[r"hel\"]));
    let error = readers
        .get()
        .transactions(&filter, &default_order(), ALL)
        .unwrap_err();
    assert!(error.to_string().contains("escape character"), "{error}");
}

#[test]
fn the_asset_is_a_containment_with_the_query_texts_expanded() {
    let (_database, _settings, readers) = store("history-asset");
    let asset = |value: serde_json::Value| TransactionCriteria {
        asset: asset_alternatives(&value).unwrap(),
        ..criteria()
    };
    assert_eq!(
        matching(&readers, asset(json!({"votes": {"alice": "60.5"}}))),
        [(2, 1)]
    );
    assert_eq!(
        matching(
            &readers,
            asset(json!({"transfers": [{"recipientId": "TD"}]}))
        ),
        [(4, 1)]
    );
    assert_eq!(
        matching(&readers, asset(json!({"transfers": [{"amount": "10"}]}))),
        [(2, 0)]
    );
    assert_eq!(
        matching(&readers, asset(json!({}))),
        [(6, 0), (4, 1), (3, 0), (2, 1), (2, 0)]
    );
    assert_eq!(
        matching(&readers, asset(json!({"votes": {"carol": "1"}}))),
        []
    );
}

#[test]
fn ids_are_prefixes_and_numbers_are_values_or_ranges() {
    let (_database, _settings, readers) = store("history-numbers");
    let prefix = |text: &str| TransactionCriteria {
        id: vec![text.to_owned()],
        ..criteria()
    };
    assert_eq!(matching(&readers, prefix(&id(4, 1).to_hex())), [(4, 1)]);
    assert_eq!(
        matching(&readers, prefix(&id(4, 1).to_hex()[..2])),
        [(4, 1)]
    );
    assert_eq!(
        matching(&readers, prefix(&id(4, 1).to_hex().to_uppercase())),
        []
    );
    assert_eq!(matching(&readers, prefix("4_")), [(4, 1), (4, 0)]);
    let fee = |values: Vec<Numeric>| TransactionCriteria {
        fee: values,
        ..criteria()
    };
    assert_eq!(
        matching(&readers, fee(vec![Numeric::Equal(100)])),
        [(4, 1), (2, 0)]
    );
    let range = |from, to| Numeric::Range { from, to };
    assert_eq!(
        matching(&readers, fee(vec![range(Some(250), Some(400))])),
        [(5, 0), (4, 0), (3, 0)]
    );
    assert_eq!(
        matching(&readers, fee(vec![range(Some(450), None)])),
        [(6, 0)]
    );
    assert_eq!(
        matching(
            &readers,
            fee(vec![range(None, Some(100)), Numeric::Equal(500)])
        ),
        [(6, 0), (4, 1), (2, 0)]
    );
    let both = TransactionCriteria {
        block_height: vec![Numeric::Equal(4)],
        amount: vec![range(Some(1), None)],
        ..criteria()
    };
    assert_eq!(matching(&readers, both), [(4, 0)]);
    let block = TransactionCriteria {
        block_id: vec![block_id(2).to_hex()],
        ..criteria()
    };
    assert_eq!(matching(&readers, block), [(2, 1), (2, 0)]);
}

#[test]
fn groups_are_all_of_their_criteria_one_of() {
    let (_database, _settings, readers) = store("history-groups");
    let sender = |value: &str| TransactionCriteria {
        sender_id: vec![value.to_owned()],
        ..criteria()
    };
    let transfers = TransactionCriteria {
        kind: vec![6],
        ..criteria()
    };
    let filter = TransactionFilter {
        groups: vec![vec![sender("TA"), sender("TB")], vec![transfers]],
    };
    assert_eq!(
        listed(&readers, &filter, &default_order(), ALL),
        [(4, 1), (2, 0)]
    );
    let nothing = TransactionFilter {
        groups: vec![Vec::new()],
    };
    assert_eq!(listed(&readers, &nothing, &default_order(), ALL), []);
}

#[test]
fn orders_break_ties_by_the_unique_key_and_put_null_where_postgresql_does() {
    let (_database, _settings, readers) = store("history-order");
    let all = TransactionFilter::default();
    let by = |keys: &[(&str, Direction)]| {
        let keys: Vec<Order> = keys.iter().map(|(p, d)| order(p, *d)).collect();
        listed(&readers, &all, &keys, ALL)
    };
    assert_eq!(
        by(&[("fee", Direction::Asc)]),
        [(2, 0), (4, 1), (2, 1), (5, 0), (3, 0), (4, 0), (6, 0)]
    );
    assert_eq!(
        by(&[("fee", Direction::Desc)]),
        [(6, 0), (4, 0), (3, 0), (5, 0), (2, 1), (4, 1), (2, 0)]
    );
    // Memos by their bytes; no memo last from the lowest, first from the highest.
    assert_eq!(
        by(&[("memo", Direction::Asc)]),
        [(3, 0), (4, 1), (2, 0), (4, 0), (2, 1), (5, 0), (6, 0)]
    );
    assert_eq!(
        by(&[("memo", Direction::Desc)]),
        [(6, 0), (5, 0), (2, 1), (4, 0), (2, 0), (4, 1), (3, 0)]
    );
    // Assets in jsonb's order: objects of one key each, so by that key's bytes, then by its
    // value (two transfers: the array with fewer items first); no asset last.
    assert_eq!(
        by(&[("asset", Direction::Asc)]),
        [(3, 0), (6, 0), (2, 0), (4, 1), (2, 1), (4, 0), (5, 0)]
    );
    assert_eq!(
        by(&[("typeGroup", Direction::Asc), ("amount", Direction::Desc)]),
        [(6, 0), (5, 0), (4, 1), (3, 0), (2, 0), (4, 0), (2, 1)]
    );
    // An order on a property that is no column finds nothing, as the reference answers.
    let listing = readers
        .get()
        .transactions(&all, &[order("address", Direction::Asc)], ALL)
        .unwrap();
    assert_eq!((listing.rows.len(), listing.total_count), (0, 0));
}

#[test]
fn blocks_are_found_by_id_and_by_height() {
    let (mut database, _settings, readers) = store("history-blocks");
    let reader = readers.get();
    let by_height = reader.block(BlockSelector::Height(3)).unwrap().unwrap();
    assert_eq!(by_height, block_row(3, 1));
    let by_id = reader.block(BlockSelector::Id(block_id(3))).unwrap();
    assert_eq!(by_id, Some(by_height));
    assert_eq!(reader.block(BlockSelector::Height(7)).unwrap(), None);
    let unknown = BlockId::from_array([0x42; 32]);
    assert_eq!(reader.block(BlockSelector::Id(unknown)).unwrap(), None);
    let heights: Vec<u32> = reader
        .blocks_by_height(&[5, 1, 5, 99])
        .unwrap()
        .iter()
        .map(|row| row.height)
        .collect();
    assert_eq!(heights, [1, 5]);
    let transactions = reader.block_transactions(&block_id(4)).unwrap();
    let ids: Vec<_> = transactions.iter().map(|t| t.row.id).collect();
    assert_eq!(ids, [id(4, 0), id(4, 1)]);
    let one = reader.transaction(&id(2, 0)).unwrap().unwrap();
    assert_eq!(one.row.recipients, ["TB", "TC"]);
    assert_eq!(
        reader
            .transaction(&TransactionId::from_array([1; 32]))
            .unwrap(),
        None
    );
    assert_eq!(reader.round(1).unwrap(), round_rows(1));
    assert_eq!(reader.round(2).unwrap(), []);
    drop(reader);
    // A reader sees what the writer commits after it opened.
    let mut chunk = database.begin().unwrap();
    chunk.save_block(&block_row(7, 0), &[]).unwrap();
    chunk.commit().unwrap();
    let reader = readers.get();
    assert!(reader.block(BlockSelector::Height(7)).unwrap().is_some());
}

#[test]
fn a_store_of_schema_version_1_gets_the_recipients_of_its_transfers() {
    let dir = scratch("history-upgrade");
    let settings = Settings::in_data_dir(&dir);
    std::fs::create_dir_all(settings.path.parent().unwrap()).unwrap();
    {
        let connection = rusqlite::Connection::open(&settings.path).unwrap();
        connection
            .execute_batch(heartwood_database::connection::SCHEMA)
            .unwrap();
        connection
            .execute_batch(
                "INSERT INTO meta (key, value) VALUES ('schema_version', '1');
                 PRAGMA user_version = 1;",
            )
            .unwrap();
        for height in 1..=6 {
            let specs: Vec<Spec> = specs().into_iter().filter(|s| s.height == height).collect();
            let block = block_row(height, u32::try_from(specs.len()).unwrap());
            connection
                .execute(
                    "INSERT INTO blocks (height, id, version, timestamp, previous_block, \
                     number_of_transactions, total_amount, total_fee, burned_fee, reward, \
                     donations, payload_length, payload_hash, generator_public_key, username, \
                     block_signature) VALUES (?1, ?2, 0, ?3, NULL, ?4, 0, 0, 0, 0, '{}', 0, '', \
                     '', NULL, '')",
                    rusqlite::params![
                        height,
                        block.id.to_hex(),
                        block.timestamp,
                        block.number_of_transactions
                    ],
                )
                .unwrap();
            for spec in &specs {
                let row = row(spec);
                connection
                    .execute(
                        "INSERT INTO transactions (id, version, block_id, block_height, \
                         sequence, timestamp, nonce, sender_id, sender_public_key, type, \
                         type_group, amount, fee, burned_fee, serialised, asset) VALUES \
                         (?1, 3, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 0, 0, 0, x'00', ?11)",
                        rusqlite::params![
                            row.id.to_hex(),
                            row.block_id.to_hex(),
                            row.block_height,
                            row.sequence,
                            row.timestamp,
                            i64::try_from(row.nonce).unwrap(),
                            row.sender_id,
                            row.sender_public_key,
                            row.kind,
                            row.type_group,
                            row.asset,
                        ],
                    )
                    .unwrap();
            }
        }
    }
    let database = Database::open(&settings).unwrap();
    let connection = rusqlite::Connection::open(&settings.path).unwrap();
    let recipients: Vec<(String, String)> = connection
        .prepare("SELECT transaction_id, recipient_id FROM transaction_recipients ORDER BY rowid")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let expected: Vec<(String, String)> = [(2, 0, "TB"), (2, 0, "TC"), (4, 1, "TA"), (4, 1, "TD")]
        .iter()
        .map(|&(h, s, r)| (id(h, s).to_hex(), r.to_owned()))
        .collect();
    assert_eq!(recipients, expected);
    let version: u32 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(version, SCHEMA_VERSION);
    let meta: String = connection
        .query_row(
            "SELECT value FROM meta WHERE key = 'schema_version'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(meta, SCHEMA_VERSION.to_string());
    drop(database);
    // The upgraded store answers the address criteria.
    let readers = Readers::open(&settings, 1).unwrap();
    let filter = TransactionFilter::of(TransactionCriteria {
        recipient_id: vec!["TD".into()],
        ..criteria()
    });
    assert_eq!(listed(&readers, &filter, &default_order(), ALL), [(4, 1)]);
}

#[test]
fn a_transfer_without_its_recipients_refuses_the_upgrade() {
    let dir = scratch("history-upgrade-corrupt");
    let settings = Settings::in_data_dir(&dir);
    std::fs::create_dir_all(settings.path.parent().unwrap()).unwrap();
    {
        let connection = rusqlite::Connection::open(&settings.path).unwrap();
        connection
            .execute_batch(heartwood_database::connection::SCHEMA)
            .unwrap();
        connection
            .execute_batch(
                "PRAGMA user_version = 1;
                 INSERT INTO meta (key, value) VALUES ('schema_version', '1');
                 INSERT INTO blocks (height, id, version, timestamp, number_of_transactions,
                   total_amount, total_fee, burned_fee, reward, donations, payload_length,
                   payload_hash, generator_public_key, block_signature)
                   VALUES (1, 'b1', 0, 0, 1, 0, 0, 0, 0, '{}', 0, '', '', '');
                 INSERT INTO transactions (id, version, block_id, block_height, sequence,
                   timestamp, nonce, sender_id, sender_public_key, type, type_group, amount, fee,
                   burned_fee, serialised, asset)
                   VALUES ('t1', 3, 'b1', 1, 0, 0, 1, 'TA', '', 6, 1, 0, 0, 0, x'00',
                   '{\"transfers\": [{\"amount\": \"1\"}]}');",
            )
            .unwrap();
    }
    let error = Database::open(&settings).unwrap_err();
    assert!(error.to_string().contains("without a recipient"), "{error}");
    // Nothing of the step stays: the file is still of version 1.
    let connection = rusqlite::Connection::open(&settings.path).unwrap();
    let version: u32 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(version, 1);
}
