//! The read-only store window of a separate process: the blocks and the named delegates' missed
//! rows from a height on, read through a `SQLITE_OPEN_READ_ONLY` connection in one read
//! transaction, capped at a row count and interrupted after a timeout; read while the writer
//! commits without holding its commits back; and never opened, so never changed, while no writer
//! has the store open.

mod common;

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use common::{chain, open, write_block};
use heartwood_database::reader_ro::{MAX_ROWS, ReadOnlyError, ReadOnlyStore};
use heartwood_database::{MissedRow, Settings};

const TIMEOUT: Duration = Duration::from_secs(1);

/// Every file of `dir` with its bytes.
fn files(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    let entries = std::fs::read_dir(dir).unwrap();
    let entries = entries.map(|entry| entry.unwrap());
    let read = |entry: std::fs::DirEntry| {
        let name = entry.file_name().into_string().unwrap();
        (name, std::fs::read(entry.path()).unwrap())
    };
    entries.map(read).collect()
}

/// The store's own file and its log, byte for byte (the `-shm` holds the readers' marks).
fn store_and_log(settings: &Settings) -> (Vec<u8>, Vec<u8>) {
    let mut log = settings.path.clone().into_os_string();
    log.push("-wal");
    (
        std::fs::read(&settings.path).unwrap(),
        std::fs::read(log).unwrap(),
    )
}

#[test]
fn the_window_reads_the_blocks_and_the_named_missed_rows_from_a_height() {
    let (mut database, settings) = chain("ro-window", 20);
    let mut chunk = database.begin().unwrap();
    let other = MissedRow {
        timestamp: 1_000_000,
        height: 18,
        username: "other".into(),
    };
    chunk.save_missed(&[other]).unwrap();
    chunk.commit().unwrap();
    let before = store_and_log(&settings);

    let store = ReadOnlyStore::open(&settings, TIMEOUT).unwrap();
    let rows = store.window(&["late".into()], 15).unwrap();
    let heights: Vec<u32> = rows.blocks.iter().map(|block| block.height).collect();
    assert_eq!(heights, (15..=20).collect::<Vec<_>>());
    for block in &rows.blocks {
        // The stored timestamps count seconds from the network epoch, read as stored.
        assert_eq!(block.timestamp, block.height * 8);
        assert_eq!(block.id.len(), 64);
        assert_eq!(block.generator_public_key.len(), 66);
        assert!(block.username.is_some());
    }
    let missed: Vec<(u32, u32, &str)> = (rows.missed.iter())
        .map(|m| (m.timestamp, m.height, m.username.as_str()))
        .collect();
    let late: Vec<(u32, u32, &str)> = (15..=20).map(|h| (h * 8 - 4, h, "late")).collect();
    assert_eq!(missed, late);
    assert!(!rows.capped);

    let both = store.window(&["other".into(), "late".into()], 18).unwrap();
    assert_eq!(both.missed.len(), 4);
    assert_eq!(both.missed[1].username, "other");
    assert!(store.window(&[], 1).unwrap().missed.is_empty());
    assert!(
        store
            .window(&["nobody".into()], 1)
            .unwrap()
            .missed
            .is_empty()
    );
    assert!(
        store
            .window(&["late".into()], 21)
            .unwrap()
            .blocks
            .is_empty()
    );
    // A name is a bound value, never SQL.
    let quoted = store.window(&["late') OR ('1'='1".into()], 1).unwrap();
    assert!(quoted.missed.is_empty());
    drop(store);
    // The reader wrote nothing to the store or its log.
    assert_eq!(store_and_log(&settings), before);
    drop(database);
}

#[test]
fn each_list_is_capped_at_the_row_count() {
    let (mut database, settings) = open("ro-capped");
    let tip = u32::try_from(MAX_ROWS).unwrap() + 2;
    let mut chunk = database.begin().unwrap();
    for height in 1..=tip {
        write_block(&mut chunk, height, 0);
    }
    chunk.commit().unwrap();
    let store = ReadOnlyStore::open(&settings, TIMEOUT).unwrap();
    let rows = store.window(&["late".into()], 1).unwrap();
    assert_eq!((rows.blocks.len(), rows.missed.len()), (MAX_ROWS, MAX_ROWS));
    assert!(rows.capped);
    assert_eq!(rows.blocks.last().unwrap().height, tip - 2);
    let rows = store.window(&["late".into()], 3).unwrap();
    assert_eq!((rows.blocks.len(), rows.missed.len()), (MAX_ROWS, MAX_ROWS));
    assert!(!rows.capped);
    drop(database);
}

#[test]
fn a_statement_past_its_timeout_is_interrupted() {
    let (mut database, settings) = open("ro-timeout");
    let mut chunk = database.begin().unwrap();
    for height in 1..=3_000 {
        write_block(&mut chunk, height, 0);
    }
    chunk.commit().unwrap();
    let store = ReadOnlyStore::open(&settings, Duration::ZERO).unwrap();
    assert_eq!(
        store.window(&["late".into()], 1),
        Err(ReadOnlyError::TimedOut)
    );
    drop(database);
}

#[test]
fn a_store_that_no_writer_holds_is_not_opened_and_stays_as_it_was() {
    let (database, settings) = chain("ro-closed", 5);
    drop(database);
    let dir = settings.path.parent().unwrap();
    let before = files(dir);
    assert_eq!(before.keys().collect::<Vec<_>>(), ["blockchain.db"]);
    for timeout in [TIMEOUT, Duration::ZERO] {
        let opened = ReadOnlyStore::open(&settings, timeout);
        assert!(matches!(opened, Err(ReadOnlyError::NoWriter)));
    }
    assert_eq!(files(dir), before);
    let missing = Settings::in_data_dir(&dir.join("nothing"));
    let opened = ReadOnlyStore::open(&missing, TIMEOUT);
    assert!(matches!(opened, Err(ReadOnlyError::NoWriter)));
    assert!(!dir.join("nothing").exists());
}

#[test]
#[expect(
    clippy::disallowed_types,
    reason = "the test measures how long the writer's commits take while the reader reads"
)]
fn the_window_reads_while_the_writer_commits_and_never_holds_a_commit_back() {
    let (mut database, settings) = chain("ro-live", 20);
    let done = AtomicBool::new(false);
    let (commits, reads) = std::thread::scope(|scope| {
        let reader = scope.spawn(|| {
            let store = ReadOnlyStore::open(&settings, TIMEOUT).unwrap();
            let (mut reads, mut last) = (0_u32, 0);
            while !done.load(Ordering::Relaxed) {
                let rows = store.window(&["late".into()], 1).unwrap();
                let tip = rows.blocks.last().unwrap().height;
                assert!(tip >= last, "the tip went back from {last} to {tip}");
                assert_eq!(rows.blocks.len(), usize::try_from(tip).unwrap());
                assert_eq!(rows.missed.len(), usize::try_from(tip - 1).unwrap());
                (reads, last) = (reads + 1, tip);
            }
            reads
        });
        let mut slowest = Duration::ZERO;
        for height in 21..=220 {
            let start = std::time::Instant::now();
            let mut chunk = database.begin().unwrap();
            write_block(&mut chunk, height, 1);
            chunk.commit().unwrap();
            slowest = slowest.max(start.elapsed());
        }
        done.store(true, Ordering::Relaxed);
        (slowest, reader.join().unwrap())
    });
    assert!(reads > 0);
    // A commit never waits for the reader: WAL readers take no lock a commit needs.
    assert!(
        commits < Duration::from_secs(1),
        "a commit took {commits:?}"
    );
    drop(database);
}
