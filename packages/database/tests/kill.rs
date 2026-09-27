//! A killed commit leaves the old chunk or the new one, never a part of one.
//!
//! A child process (this test binary, running [`writer`]) commits one chunk after another: each
//! chunk writes a block with its rows, its undo record and state row, and rewrites every wallet
//! row with the chunk's height. The parent kills it with `SIGKILL` at a random moment, many
//! times over the same file. After each kill the store's tip must be the last height the child
//! reported committed, or the next one (committed, not yet reported), and every row must be that
//! tip's: the boot checks pass and all the wallet rows carry the tip's height.
//!
//! `HEARTWOOD_DATABASE_KILLS` sets the number of kills (20 by default).

mod common;

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use heartwood_database::{Database, Settings};

/// Set in the child: the database file it writes.
const CHILD: &str = "HEARTWOOD_DATABASE_KILL_CHILD";

/// The wallet rows each chunk rewrites.
const WALLETS: u32 = 200;

fn wallet(index: u32) -> String {
    format!("T{index:04}")
}

fn record(height: u32, index: u32) -> String {
    format!("{height}:{index}:{}", "x".repeat(100))
}

/// The child's loop; a no-op test when [`CHILD`] is not set.
#[test]
fn writer() {
    let Some(path) = std::env::var_os(CHILD) else {
        return;
    };
    let mut database = Database::open(&Settings { path: path.into() }).unwrap();
    let mut height = database
        .last_block()
        .unwrap()
        .map_or(0, |block| block.row.height);
    loop {
        height += 1;
        let mut chunk = database.begin().unwrap();
        let mut savepoint = chunk.savepoint().unwrap();
        common::write_block(&mut savepoint, height, height % 3);
        for index in 0..WALLETS {
            savepoint
                .put_wallet(&wallet(index), &record(height, index))
                .unwrap();
        }
        savepoint.commit().unwrap();
        chunk.commit().unwrap();
        println!("committed {height}");
    }
}

/// Kills the child when the test ends, whatever happens.
struct Killed(Child);

impl Drop for Killed {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Every row of the store is the tip's.
fn check(database: &Database, tip: u32) {
    let report = database.integrity(&common::milestones()).unwrap();
    assert_eq!(report.tip, Some(tip));
    assert!(report.is_ok(), "{:?}", report.problems);
    let blocks = database.blocks(1, tip).unwrap();
    for (block, height) in blocks.iter().zip(1..) {
        assert_eq!(block.row, common::block_row(height, height % 3));
        assert_eq!(block.transactions.len() as u32, height % 3);
    }
    let state = database.state_row().unwrap().unwrap();
    assert_eq!((state.height, state.record), (tip, format!("state {tip}")));
    let wallets = database.wallets().unwrap();
    assert_eq!(wallets.len() as u32, WALLETS);
    for (row, index) in wallets.iter().zip(0..) {
        assert_eq!(row.address, wallet(index));
        assert_eq!(row.record, record(tip, index), "a wallet of another chunk");
    }
    assert_eq!(database.missed_blocks().unwrap().len() as u32, tip - 1);
}

#[test]
fn a_killed_commit_leaves_the_old_or_the_new_chunk() {
    if std::env::var_os(CHILD).is_some() {
        return;
    }
    let kills: u32 = std::env::var("HEARTWOOD_DATABASE_KILLS")
        .ok()
        .map_or(20, |text| text.parse().unwrap());
    let settings = Settings::in_data_dir(&common::scratch("kill"));
    let mut seed: u64 = 0x2545_f491_4f6c_dd1d;
    let (mut old, mut new) = (0, 0);

    for _ in 0..kills {
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["writer", "--exact", "--nocapture", "--test-threads=1"])
            .env(CHILD, &settings.path)
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let mut child = Killed(child);
        let (sender, receiver) = mpsc::channel();
        let reader = std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if let Some(height) = line.strip_prefix("committed ") {
                    sender.send(height.parse::<u32>().unwrap()).unwrap();
                }
            }
        });

        // Wait for this run's first commit, then kill at a random moment of the following ones.
        let first = receiver
            .recv_timeout(Duration::from_secs(60))
            .expect("the child commits");
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        std::thread::sleep(Duration::from_micros(seed % 20_000));
        child.0.kill().unwrap();
        child.0.wait().unwrap();
        reader.join().unwrap();
        let reported = receiver.try_iter().last().unwrap_or(first);

        let database = Database::open(&settings).unwrap();
        let tip = database.last_block().unwrap().unwrap().row.height;
        match tip.checked_sub(reported) {
            Some(0) => old += 1,
            Some(1) => new += 1,
            _ => panic!("tip {tip}, last reported commit {reported}"),
        }
        check(&database, tip);
    }
    println!("{kills} kills: {old} left the reported chunk, {new} the one after it");
}
