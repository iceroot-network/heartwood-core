//! `heartwood snapshot rollback` as its operators run it, on a relay's store of the committed
//! one-node chain (tip 265, rounds of 53): it refuses while the relay runs, with the reference's
//! text and the instance's name, and holds the relay's lock while it runs, so that a relay started
//! meanwhile refuses, and removes the pid file a dead relay left, so that neither `relay status`
//! nor `relay stop` reads it meanwhile; each of the reference's refusals prints the reference's
//! lines, leaves the store as it was and exits with 1; a rollback to 133 lands on 106, prints the
//! reference's three lines and exits with 0, and the relay's next start replays the chain for want
//! of a state and writes round 3's rows again; the colon spelling is the same command; a data
//! directory without a store, or with another network's store, is refused.

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::OnceLock;
use std::time::Duration;

use common::{Setup, corpus};
use heartwood_blockchain::{ChainStore, ReplayReason, RestoreMode, RestorePath, restore};
use heartwood_cli::supervisor::signals::{is_locked, take_lock};
use heartwood_core::app::restored;
use heartwood_core::config::RelayConfig;
use heartwood_crypto::Block;
use heartwood_database::{ChainMeta, Database, Settings};
use heartwood_kernel::config::crypto;
use heartwood_transactions::Handlers;

const TOKEN: &str = "t";
const NETWORK: &str = "devnet";

/// A store of the committed chain at its tip, made once and copied into each test's relay.
fn template() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("rollback-template");
        let _ = fs::remove_dir_all(&dir);
        let files = crypto::load(&corpus().join("crypto")).unwrap();
        let mut store = ChainStore::open(&Settings::in_data_dir(&dir)).unwrap();
        let mut state = restore(&mut store, &files, &Handlers, RestoreMode::Auto)
            .unwrap()
            .state;
        let milestones = &files.milestones;
        let blocks = fs::read_to_string(corpus().join("blocks.jsonl")).unwrap();
        let mut chunk = store.chunk().unwrap();
        for line in blocks.lines() {
            let line: serde_json::Value = serde_json::from_str(line).unwrap();
            let bytes =
                heartwood_crypto::utils::hex::decode(line["hex"].as_str().unwrap()).unwrap();
            let block = Block::receive(&bytes, milestones).unwrap();
            let applied = state.apply_block(&block, milestones, &Handlers).unwrap();
            chunk.applied(&state, &block, &applied, milestones).unwrap();
        }
        chunk.commit().unwrap();
        drop(store);
        dir
    })
}

/// A relay of the committed chain with its store at the tip.
struct Relay {
    setup: Setup,
}

impl Relay {
    fn new(name: &str) -> Relay {
        let setup = Setup::new(&format!("rollback-{name}"), "");
        let data = setup.root.join("data");
        fs::create_dir_all(data.join("database")).unwrap();
        for entry in fs::read_dir(template().join("database")).unwrap() {
            let entry = entry.unwrap();
            fs::copy(entry.path(), data.join("database").join(entry.file_name())).unwrap();
        }
        Relay { setup }
    }

    fn data(&self) -> PathBuf {
        self.setup.root.join("data")
    }

    fn store(&self) -> PathBuf {
        Settings::in_data_dir(&self.data()).path
    }

    fn lock(&self) -> PathBuf {
        self.data().join("run/relay.lock")
    }

    /// `heartwood <args>` for this relay's instance, with its directories.
    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_heartwood"));
        command
            .args(args)
            .args(["--token", TOKEN, "--network", NETWORK])
            .env_clear()
            .env("HOME", self.setup.home())
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .envs(self.setup.process(&[]))
            .stdin(Stdio::null());
        command
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command(args).output().unwrap()
    }

    /// The lines of the command's log file, without their times.
    fn logged(&self) -> Vec<String> {
        let file = format!("{TOKEN}-{NETWORK}-snapshot-current.log");
        let text = fs::read_to_string(self.setup.root.join("log").join(file)).unwrap_or_default();
        let lines = text.lines().filter_map(|line| line.split_once("] "));
        lines.map(|(_, line)| line.to_owned()).collect()
    }

    /// The store's tip, whether it holds a state row, and its highest round with rows.
    fn stored(&self) -> (u32, bool, u64) {
        let database = Database::open(&Settings::in_data_dir(&self.data())).unwrap();
        let tip = database.last_block().unwrap().unwrap().row.height;
        let state = database.state_row().unwrap().is_some();
        let chunk_counts = {
            let mut database = database;
            let chunk = database.begin().unwrap();
            chunk.row_counts(0, 0).unwrap()
        };
        (tip, state, chunk_counts.last_round)
    }

    /// The relay's start as `relay run` starts it, in this process.
    fn start(&self) -> (ChainStore, heartwood_blockchain::Restored) {
        let run = common::run(&format!("--token {TOKEN} --network {NETWORK}"));
        let config = RelayConfig::load(
            &run,
            &self.setup.process(&[]),
            &self.setup.home(),
            &self.setup.root,
        );
        restored(&config.unwrap()).unwrap()
    }
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn a_rollback_refuses_while_the_relay_runs_and_holds_the_lock_while_it_runs() {
    let relay = Relay::new("lock");
    let before = fs::read(relay.store()).unwrap();
    // The relay's lock held, as a running relay holds it.
    fs::create_dir_all(relay.data().join("run")).unwrap();
    let held = take_lock(&relay.lock()).unwrap().unwrap();
    let output = relay.run(&["snapshot", "rollback", "--height", "133"]);
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    assert_eq!(
        stderr(&output).trim(),
        format!("The \"{TOKEN}-{NETWORK}-relay\" process is already running")
    );
    assert_eq!(fs::read(relay.store()).unwrap(), before);
    assert!(relay.logged().is_empty());
    drop(held);
    // The store held by another writer, so that the command waits inside its run: a relay started
    // meanwhile refuses, for the command holds the relay's lock.
    let mut writer = Database::open(&Settings::in_data_dir(&relay.data())).unwrap();
    let chunk = writer.begin().unwrap();
    let mut rollback = relay
        .command(&["snapshot", "rollback", "--height", "133"])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut waits = 0;
    while !is_locked(&relay.lock()).unwrap() {
        waits += 1;
        assert!(waits < 400, "the command took no lock within 4 s");
        std::thread::sleep(Duration::from_millis(10));
    }
    let started = relay.run(&["relay", "run"]);
    assert_eq!(started.status.code(), Some(1), "{}", stderr(&started));
    assert!(
        stderr(&started).contains("another relay of this instance holds"),
        "{}",
        stderr(&started)
    );
    drop(chunk);
    drop(writer);
    let status = rollback.wait().unwrap();
    assert_eq!(status.code(), Some(0));
    assert_eq!(relay.stored(), (106, false, 2));
    assert!(!is_locked(&relay.lock()).unwrap());
}

#[test]
fn a_stale_pid_file_goes_so_that_status_and_stop_during_the_rollback_read_no_pid() {
    let relay = Relay::new("stale-pid");
    // The pid file of a relay that died without removing it, naming a live process.
    let pid_file = relay.data().join("run/relay.pid");
    fs::create_dir_all(relay.data().join("run")).unwrap();
    let mut other = Command::new("sleep").arg("60").spawn().unwrap();
    fs::write(&pid_file, format!("{}\n", other.id())).unwrap();
    // The store held by another writer, so that the command waits inside its run.
    let mut writer = Database::open(&Settings::in_data_dir(&relay.data())).unwrap();
    let chunk = writer.begin().unwrap();
    let rollback = relay
        .command(&["snapshot", "rollback", "--height", "133"])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut waits = 0;
    while !is_locked(&relay.lock()).unwrap() || pid_file.exists() {
        waits += 1;
        assert!(
            waits < 400,
            "no lock, or the pid file still there, after 4 s"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    // Neither `relay status` nor `relay stop` reads the stale pid, so the process is untouched.
    let status = relay.run(&["relay", "status"]);
    let listed = String::from_utf8_lossy(&status.stdout).into_owned();
    assert!(!listed.contains(&other.id().to_string()), "{listed}");
    let stop = relay.run(&["relay", "stop"]);
    assert_eq!(stop.status.code(), Some(1), "{}", stderr(&stop));
    assert!(
        other.try_wait().unwrap().is_none(),
        "the process was signalled"
    );
    drop(chunk);
    drop(writer);
    let done = rollback.wait_with_output().unwrap();
    assert_eq!(done.status.code(), Some(0), "{}", stderr(&done));
    assert_eq!(relay.stored(), (106, false, 2));
    other.kill().unwrap();
    other.wait().unwrap();
}

#[test]
fn each_refusal_prints_the_references_lines_leaves_the_store_and_exits_with_1() {
    let relay = Relay::new("refusals");
    let before = fs::read(relay.store()).unwrap();
    let start = "INFO: Rolling back the blockchain";
    let cases: [(&[&str], &[&str]); 7] = [
        (
            &[],
            &["ERROR: Please specify either a height or number of blocks to roll back"],
        ),
        (
            &["--height", "0", "--number", "0"],
            &["ERROR: Please specify either a height or number of blocks to roll back"],
        ),
        (
            &["--height", "-5"],
            &[start, "ERROR: Rollback height -5 is invalid"],
        ),
        (
            &["--number", "265"],
            &[start, "ERROR: Rollback height 0 is invalid"],
        ),
        (
            &["--height", "265"],
            &[
                start,
                "ERROR: Rollback height 265 is not less than the current height 265",
            ],
        ),
        (
            &["--height=300"],
            &[
                start,
                "ERROR: Rollback height 300 is not less than the current height 265",
            ],
        ),
        (
            &["--height", "40"],
            &[
                start,
                "INFO: Last block height is: 265",
                "ERROR: Rollback failed",
                "ERROR: Error: Cannot find block on height 1",
            ],
        ),
    ];
    for (flags, lines) in cases {
        let _ = fs::remove_dir_all(relay.setup.root.join("log"));
        let mut args = vec!["snapshot", "rollback"];
        args.extend(flags);
        let output = relay.run(&args);
        assert_eq!(
            output.status.code(),
            Some(1),
            "{flags:?}: {}",
            stderr(&output)
        );
        assert_eq!(relay.logged(), lines, "{flags:?}");
        assert_eq!(fs::read(relay.store()).unwrap(), before, "{flags:?}");
    }
}

#[test]
fn a_rollback_to_133_lands_on_106_and_the_next_start_replays_and_writes_round_3() {
    let relay = Relay::new("to-133");
    let output = relay.run(&["snapshot", "rollback", "--height", "133"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(stderr(&output).is_empty(), "{}", stderr(&output));
    assert_eq!(
        relay.logged(),
        [
            "INFO: Rolling back the blockchain",
            "INFO: Last block height is: 265",
            "INFO: Rolling back blockchain to last finished round 3 with last block height 106"
        ]
    );
    // No state row and no round above round 2 until the next start.
    assert_eq!(relay.stored(), (106, false, 2));
    let (store, started) = relay.start();
    assert_eq!(started.path, RestorePath::Replay(ReplayReason::NoState));
    assert_eq!(started.state.height(), 106);
    assert_eq!(store.database().round(3).unwrap().len(), 53);
    drop(store);
    assert_eq!(relay.stored(), (106, true, 3));
}

#[test]
fn the_colon_spelling_is_the_same_command() {
    let (spaced, colon) = (Relay::new("spaced"), Relay::new("colon"));
    let a = spaced.run(&["snapshot", "rollback", "--number", "60"]);
    let b = colon.run(&["snapshot:rollback", "--number=60"]);
    assert_eq!((a.status.code(), b.status.code()), (Some(0), Some(0)));
    assert_eq!(spaced.logged(), colon.logged());
    assert_eq!(spaced.stored(), (159, false, 3));
    assert_eq!(colon.stored(), spaced.stored());
}

#[test]
fn a_data_directory_without_a_store_or_with_another_networks_is_refused() {
    let relay = Relay::new("no-store");
    fs::remove_dir_all(relay.data()).unwrap();
    let output = relay.run(&["snapshot", "rollback", "--height", "133"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).contains("there is no store in"),
        "{}",
        stderr(&output)
    );
    assert!(
        !relay.data().exists(),
        "the refusal made the data directory"
    );
    let other = Relay::new("other-network");
    {
        let mut database = Database::open(&Settings::in_data_dir(&other.data())).unwrap();
        let meta = database.meta().unwrap().unwrap();
        let mut chunk = database.begin().unwrap();
        let nethash = "00".repeat(32);
        chunk.put_meta(&ChainMeta { nethash, ..meta }).unwrap();
        chunk.commit().unwrap();
    }
    let before = fs::read(other.store()).unwrap();
    let output = other.run(&["snapshot", "rollback", "--height", "133"]);
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("is not this network's store"),
        "{}",
        stderr(&output)
    );
    assert_eq!(fs::read(other.store()).unwrap(), before);
}
