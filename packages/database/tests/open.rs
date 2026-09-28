//! Opening a store: a new file gets the current schema version in a new directory, a reopened file
//! keeps its rows, a closed writer leaves them all in the database file, and a file of another
//! schema version or of another program is refused.

mod common;

use std::path::Path;

use common::{chain, open, scratch};
use heartwood_database::{
    ChainMeta, Database, DatabaseError, Readers, SCHEMA_VERSION, Settings, TransactionRow,
};

fn refused(path: &Path) -> String {
    match Database::open(&Settings {
        path: path.to_path_buf(),
    }) {
        Err(DatabaseError::SchemaVersion { found, expected }) => {
            assert_eq!(expected, SCHEMA_VERSION);
            found
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_new_file_gets_its_directory_and_the_current_schema_version() {
    let data = scratch("open-new");
    let settings = Settings::in_data_dir(&data);
    assert_eq!(settings.path, data.join("database").join("blockchain.db"));
    assert!(!settings.path.exists());
    drop(Database::open(&settings).unwrap());

    let connection = rusqlite::Connection::open(&settings.path).unwrap();
    let version: u32 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(version, SCHEMA_VERSION);
    let mode: String = connection
        .pragma_query_value(None, "journal_mode", |row| row.get(0))
        .unwrap();
    assert_eq!(mode, "wal");
    let tables: Vec<String> = connection
        .prepare("SELECT name FROM sqlite_schema WHERE type = 'table' ORDER BY name")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(
        tables,
        [
            "blocks",
            "delegate_versions",
            "meta",
            "milestone_entries",
            "missed_blocks",
            "rounds",
            "state",
            "transaction_recipients",
            "transactions",
            "undo",
            "wallets"
        ]
    );
}

#[test]
fn a_reopened_file_keeps_its_rows() {
    let (database, settings) = chain("open-reopen", 7);
    let blocks = database.blocks(1, 7).unwrap();
    drop(database);
    let database = Database::open(&settings).unwrap();
    assert_eq!(database.blocks(1, 7).unwrap(), blocks);
    assert!(database.integrity(&common::milestones()).unwrap().is_ok());
}

#[test]
fn a_closed_writer_leaves_every_committed_row_in_the_database_file() {
    let (database, settings) = chain("open-checkpoint", 7);
    let blocks = database.blocks(1, 7).unwrap();
    // Read-only connections that outlive the writer, as a relay's may.
    let readers = Readers::open(&settings, 2).unwrap();
    drop(database);
    let copy = scratch("open-checkpoint-copy").join("blockchain.db");
    std::fs::copy(&settings.path, &copy).unwrap();
    let database = Database::open(&Settings { path: copy }).unwrap();
    assert_eq!(database.blocks(1, 7).unwrap(), blocks);
    drop(readers);
}

#[test]
fn a_file_of_another_schema_version_is_refused() {
    let (database, settings) = chain("open-version", 2);
    drop(database);
    let connection = rusqlite::Connection::open(&settings.path).unwrap();
    connection
        .pragma_update(None, "user_version", SCHEMA_VERSION + 1)
        .unwrap();
    assert_eq!(refused(&settings.path), (SCHEMA_VERSION + 1).to_string());

    // The file's version is right but its meta names another.
    connection
        .pragma_update(None, "user_version", SCHEMA_VERSION)
        .unwrap();
    let mut database = Database::open(&settings).unwrap();
    let mut chunk = database.begin().unwrap();
    chunk
        .put_meta(&ChainMeta {
            schema_version: SCHEMA_VERSION + 1,
            nethash: String::new(),
            genesis_id: common::block_id(1),
            state_format: 1,
        })
        .unwrap();
    chunk.commit().unwrap();
    drop(database);
    assert_eq!(refused(&settings.path), (SCHEMA_VERSION + 1).to_string());
}

#[test]
fn a_file_of_another_program_is_refused() {
    let dir = scratch("open-foreign");

    let foreign = dir.join("foreign.db");
    rusqlite::Connection::open(&foreign)
        .unwrap()
        .execute_batch("CREATE TABLE notes (text TEXT)")
        .unwrap();
    assert!(refused(&foreign).contains("another program"));

    let garbage = dir.join("garbage.db");
    std::fs::write(&garbage, vec![b'x'; 8192]).unwrap();
    assert!(refused(&garbage).contains("not an SQLite database"));

    // A directory that cannot be created.
    let file = dir.join("file");
    std::fs::write(&file, b"").unwrap();
    let settings = Settings::in_data_dir(&file);
    assert!(matches!(
        Database::open(&settings),
        Err(DatabaseError::Directory { path, .. }) if path == file.join("database")
    ));
}

#[test]
fn foreign_keys_are_enforced() {
    let (mut database, _) = open("open-foreign-keys");
    let mut chunk = database.begin().unwrap();
    let orphan = TransactionRow {
        block_id: common::block_id(9),
        ..common::transaction_row(1, 0)
    };
    assert!(matches!(
        chunk.save_block(&common::block_row(1, 1), &[orphan]),
        Err(DatabaseError::Sqlite(_))
    ));
    assert!(matches!(
        chunk.put_undo(5, "no block"),
        Err(DatabaseError::Sqlite(_))
    ));
}
