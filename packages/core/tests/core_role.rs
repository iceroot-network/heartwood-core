//! The `core` role as its operators run it, over the real binary: one process holding the core,
//! relay and forger locks of its instance, refused by a relay or a forger of the instance and
//! refusing them (each way), started as a daemon by default and in the foreground with the node
//! in its own process group with `--daemon=false`, forging on a one-node devnet, and stopped by
//! SIGINT within 30 s in the core order (the relay's first two steps, the forger's three, then the
//! relay's last two). A `remoteAccess` that admits no loopback address, and a core profile with
//! no forger, stop the start. `version --verbose` names a test seam only in a build that has it.
//!
//! Each test's node listens at its own address of the loopback block of these tests (127.13.0.41
//! to 127.13.0.47, or the same last octets of the block `HEARTWOOD_TEST_LOOPBACK` names; P2P port
//! 6002, API port 6003), and its forger connects from the same address, which the node's
//! `remoteAccess` lists. Its `peers.json` names the node itself, as a one-node devnet's does.

use std::fs;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::Duration;

use heartwood_cli::supervisor::signals::is_locked;
use serde_json::{Value, json};
use tokio::time::Instant;

const P2P_PORT: u16 = 6002;
const API_PORT: u16 = 6003;
const TOKEN: &str = "core";
const NETWORK: &str = "devnet";

/// A process, with its group, killed when dropped.
struct Killed(Child);

/// Send `signal` to the process group `group`.
fn signal_group(signal: &str, group: u32) -> bool {
    let group = format!("-{group}");
    let sent = Command::new("kill")
        .args([signal, "--", &group])
        .stderr(Stdio::null())
        .status();
    sent.is_ok_and(|status| status.success())
}

impl Drop for Killed {
    fn drop(&mut self) {
        signal_group("-KILL", self.0.id());
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Stops a node started as a daemon when dropped, whether the test passed or not.
struct Stopped<'a>(&'a Node);

impl Drop for Stopped<'_> {
    fn drop(&mut self) {
        let _ = self.0.run(&["core", "stop"]);
        if let Some(pid) = self.0.pid("core") {
            let _ = Command::new("kill")
                .args(["-KILL", &pid.to_string()])
                .status();
        }
    }
}

/// The address `last` of the tests' loopback block.
fn host(last: u8) -> String {
    let block = std::env::var("HEARTWOOD_TEST_LOOPBACK").unwrap_or_default();
    let block = if block.is_empty() { "127.13.0" } else { &block };
    format!("{block}.{last}")
}

/// A node's scratch root: `config/` (the chain of `network generate`, the node's files) and the
/// other four directories beside it.
struct Node {
    root: PathBuf,
    host: String,
}

impl Node {
    /// A node named `name` at the address `host` whose chain `network generate` makes now, with
    /// its epoch now, and a core profile (the relay's components and the forger).
    fn new(name: &str, host: String) -> Node {
        let root = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join("core-role")
            .join(name);
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let node = Node {
            root,
            host: host.clone(),
        };
        let generated = Command::new(env!("CARGO_BIN_EXE_heartwood"))
            .args(["network", "generate", "--network", NETWORK, "--seed", name])
            .arg("--out")
            .arg(node.config())
            .arg("--chainOnly")
            .output()
            .unwrap();
        assert!(generated.status.success(), "{generated:?}");
        let env = format!(
            "CORE_P2P_HOST={host}\nCORE_P2P_PORT={P2P_PORT}\nCORE_API_HOST={host}\nCORE_API_PORT={API_PORT}\nCORE_P2P_MIN_NETWORK_REACH=0\n"
        );
        fs::write(node.config().join(".env"), env).unwrap();
        let peers = json!({"list": [{"ip": host, "port": P2P_PORT}]});
        fs::write(node.config().join("peers.json"), peers.to_string()).unwrap();
        node.profile(json!([host]));
        node
    }

    /// Write `app.json`: the core profile has the relay's components with `remote_access` and
    /// the forger, the relay and forger profiles the same components as the roles alone.
    fn profile(&self, remote_access: Value) {
        let plugin = |name: &str| json!({"package": format!("@solar-network/{name}")});
        let p2p =
            json!({"package": "@solar-network/p2p", "options": {"remoteAccess": remote_access}});
        let relay = ["logger", "state", "database", "transactions", "pool"].map(plugin);
        let relay: Vec<Value> = (relay.into_iter())
            .chain([p2p, plugin("blockchain"), plugin("api")])
            .collect();
        let hosts = json!([{"hostname": self.host, "port": P2P_PORT}]);
        let forger = json!({"package": "@solar-network/forger", "options": {"hosts": hosts, "localAddress": self.host}});
        let alone = [plugin("logger"), plugin("transactions"), forger.clone()];
        let app = json!({
            "core": {"plugins": relay.iter().cloned().chain([forger]).collect::<Vec<_>>()},
            "relay": {"plugins": relay},
            "forger": {"plugins": alone},
        });
        fs::write(self.config().join("app.json"), app.to_string()).unwrap();
    }

    fn config(&self) -> PathBuf {
        self.root.join("config")
    }

    /// The node's run directory, where its locks and pid files are.
    fn run_dir(&self) -> PathBuf {
        self.root.join("data/run")
    }

    /// Whether the lock `<name>.lock` of the node is held.
    fn locked(&self, name: &str) -> bool {
        is_locked(&self.run_dir().join(format!("{name}.lock"))).unwrap()
    }

    /// `heartwood <args>` of this node.
    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_heartwood"));
        command
            .args(args)
            .args(["--token", TOKEN, "--network", NETWORK]);
        for (kind, dir) in [
            ("CONFIG", "config"),
            ("DATA", "data"),
            ("CACHE", "cache"),
            ("LOG", "log"),
            ("TEMP", "temp"),
        ] {
            command.env(format!("CORE_PATH_{kind}"), self.root.join(dir));
        }
        command.stdin(Stdio::null());
        command
    }

    /// Run `heartwood <args>` to its end.
    fn run(&self, args: &[&str]) -> Output {
        self.command(args).output().unwrap()
    }

    /// Start `heartwood <args>`, its output kept in `<name>.log`.
    fn start(&self, name: &str, args: &[&str]) -> Killed {
        let log = fs::File::create(self.root.join(format!("{name}.log"))).unwrap();
        let mut command = self.command(args);
        // A group of its own, as a terminal's foreground job is: its SIGINT goes to every process
        // of the group.
        command.process_group(0);
        command.stdout(log.try_clone().unwrap()).stderr(log);
        Killed(command.spawn().unwrap())
    }

    /// The output of the process started as `name`, so far.
    fn output(&self, name: &str) -> String {
        fs::read_to_string(self.root.join(format!("{name}.log"))).unwrap_or_default()
    }

    /// The node's own log file `<token>-<network>-core-current.log`, so far.
    fn log(&self) -> String {
        let file = format!("{TOKEN}-{NETWORK}-core-current.log");
        fs::read_to_string(self.root.join("log").join(file)).unwrap_or_default()
    }

    /// `GET path` on the node's API: the body's `data`, when it answers 200.
    fn api(&self, path: &str) -> Option<Value> {
        let mut stream = TcpStream::connect((self.host.as_str(), API_PORT)).ok()?;
        stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
        let host = &self.host;
        let request = format!("GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n");
        stream.write_all(request.as_bytes()).ok()?;
        let mut response = String::new();
        stream.read_to_string(&mut response).ok()?;
        let (head, body) = response.split_once("\r\n\r\n")?;
        head.starts_with("HTTP/1.1 200").then_some(())?;
        serde_json::from_str::<Value>(body)
            .ok()?
            .get("data")
            .cloned()
    }

    /// The height of the node's last block.
    fn height(&self) -> Option<u64> {
        self.api("/api/blocks/last")?["height"].as_u64()
    }

    /// Wait for the API to answer.
    fn await_api(&self) {
        wait_until(Duration::from_secs(60), "API", || {
            self.api("/api/node/status").is_some()
        });
    }

    /// The pid in the pid file `<name>.pid`.
    fn pid(&self, name: &str) -> Option<u32> {
        let text = fs::read_to_string(self.run_dir().join(format!("{name}.pid"))).ok()?;
        text.trim().parse().ok()
    }
}

fn wait_until(limit: Duration, what: &str, mut done: impl FnMut() -> bool) {
    let start = Instant::now();
    while !done() {
        assert!(start.elapsed() < limit, "no {what} after {limit:?}");
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// The exit status of `child` within `limit`.
fn exit_within(child: &mut Killed, limit: Duration) -> std::process::ExitStatus {
    let start = Instant::now();
    loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            return status;
        }
        assert!(start.elapsed() < limit, "still running after {limit:?}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// SIGINT to `child`, and its exit status within 30 s.
fn interrupt(child: &mut Killed) -> std::process::ExitStatus {
    assert!(signal_group("-INT", child.0.id()));
    exit_within(child, Duration::from_secs(30))
}

/// The text of `output`'s two streams.
fn text(output: &Output) -> String {
    let both = [&output.stdout, &output.stderr];
    both.map(|s| String::from_utf8_lossy(s).into_owned())
        .join("")
}

/// The position of the first line of `log` that has `what` at or after `from`.
fn position(log: &str, what: &str, from: usize) -> usize {
    let found = log[from..].find(what);
    let found = found.unwrap_or_else(|| panic!("no {what:?} after byte {from} in\n{log}"));
    from + found
}

#[test]
fn a_core_process_holds_all_three_locks_and_refuses_a_relay_and_a_forger() {
    let node = Node::new("locks", host(41));
    let mut core = node.start("core", &["core", "run", "--networkStart"]);
    node.await_api();
    for name in ["core", "relay", "forger"] {
        assert!(node.locked(name), "{name}.lock is not held");
    }
    // The core process's pid is in its own pid file only: a relay's or a forger's command finds
    // no process of theirs to signal.
    assert_eq!(node.pid("core"), Some(core.0.id()));
    assert_eq!(node.pid("relay"), None);
    assert_eq!(node.pid("forger"), None);
    for role in ["relay", "forger"] {
        // `start` refuses (a foreground one as well as a daemon), and so does `run`.
        for flags in [&["start"][..], &["start", "--daemon=false"], &["run"]] {
            let args: Vec<&str> = [role].into_iter().chain(flags.iter().copied()).collect();
            let refused = node.run(&args);
            assert!(!refused.status.success(), "{args:?}: {}", text(&refused));
            // A start names the core process that runs, as the reference's aborts do.
            let said = match flags[0] {
                "start" => format!("{TOKEN}-{NETWORK}-core is already running"),
                _ => format!("another {role} of this instance holds"),
            };
            assert!(
                text(&refused).contains(&said),
                "{args:?}: {}",
                text(&refused)
            );
        }
    }
    // A second core node is refused as well.
    let second = node.run(&["core", "run"]);
    assert!(text(&second).contains("another core of this instance holds"));
    assert!(interrupt(&mut core).success(), "{}", node.output("core"));
    for name in ["core", "relay", "forger"] {
        assert!(!node.locked(name), "{name}.lock is held after the stop");
    }
    assert_eq!(node.pid("core"), None);
}

#[test]
fn core_start_refuses_while_a_relay_or_a_forger_runs() {
    let node = Node::new("beside", host(42));
    let mut relay = node.start("relay", &["relay", "run", "--networkStart"]);
    node.await_api();
    let refused = node.run(&["core", "start"]);
    assert!(!refused.status.success(), "{}", text(&refused));
    assert!(
        text(&refused).contains(&format!("{TOKEN}-{NETWORK}-relay is already running")),
        "{}",
        text(&refused)
    );
    let refused = node.run(&["core", "run"]);
    assert!(text(&refused).contains("another relay of this instance holds"));
    assert!(interrupt(&mut relay).success(), "{}", node.output("relay"));
    // A forger alone: its relay is not there, and it waits for one.
    let mut forger = node.start("forger", &["forger", "run"]);
    wait_until(Duration::from_secs(30), "the forger's lock", || {
        node.locked("forger")
    });
    let refused = node.run(&["core", "start"]);
    assert!(!refused.status.success(), "{}", text(&refused));
    assert!(
        text(&refused).contains(&format!("{TOKEN}-{NETWORK}-forger is already running")),
        "{}",
        text(&refused)
    );
    let refused = node.run(&["core", "run"]);
    assert!(text(&refused).contains("another forger of this instance holds"));
    // The core's refused start took nothing: the relay's and the core's locks are free.
    assert!(!node.locked("core") && !node.locked("relay"));
    assert!(
        interrupt(&mut forger).success(),
        "{}",
        node.output("forger")
    );
}

#[test]
fn core_start_detaches_by_default_and_core_stop_ends_it() {
    let node = Node::new("detached", host(43));
    let _stop = Stopped(&node);
    let started = Instant::now();
    let output = node.run(&["core", "start", "--networkStart"]);
    assert!(output.status.success(), "{}", text(&output));
    assert!(
        started.elapsed() < Duration::from_secs(30),
        "start did not detach"
    );
    node.await_api();
    for name in ["core", "relay", "forger"] {
        assert!(node.locked(name), "{name}.lock is not held");
    }
    assert!(
        node.locked("core.supervisor"),
        "no supervisor of the core role"
    );
    let status = node.run(&["core", "status"]);
    assert!(status.status.success());
    assert!(
        text(&status).contains(&format!("{TOKEN}-{NETWORK}-core")),
        "{}",
        text(&status)
    );
    // Another start of the instance, of its relay or of its forger is refused while the daemon
    // runs, naming the core process; and so is a rollback of its store.
    let core = format!("{TOKEN}-{NETWORK}-core is already running");
    for command in [["core", "start"], ["relay", "start"], ["forger", "start"]] {
        let again = node.run(&command);
        assert!(!again.status.success(), "{command:?}");
        assert!(
            text(&again).contains(&core),
            "{command:?}: {}",
            text(&again)
        );
    }
    let rollback = node.run(&["snapshot", "rollback", "--number", "1"]);
    assert!(!rollback.status.success(), "{}", text(&rollback));
    assert!(
        text(&rollback).contains("already running"),
        "{}",
        text(&rollback)
    );
    let stop = node.run(&["core", "stop"]);
    assert!(stop.status.success(), "{}", text(&stop));
    for name in ["core", "relay", "forger", "core.supervisor"] {
        assert!(!node.locked(name), "{name}.lock is held after the stop");
    }
    assert!(node.log().contains("Received signal"), "{}", node.log());
}

/// The process group of the process `pid`, from `/proc`.
fn group_of(pid: u32) -> u32 {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
    // The fields after the command's name, which is in parentheses: state, ppid, pgrp.
    let after = stat.rsplit_once(')').unwrap().1;
    after.split_whitespace().nth(2).unwrap().parse().unwrap()
}

#[test]
fn core_start_without_the_daemon_stays_in_the_foreground_with_the_node_in_its_own_group() {
    let node = Node::new("foreground", host(44));
    let args = ["core", "start", "--daemon=false", "--networkStart"];
    let mut supervisor = node.start("core", &args);
    let _stop = Stopped(&node);
    node.await_api();
    let pid = node.pid("core").expect("the core node's pid file");
    assert_ne!(pid, supervisor.0.id());
    assert!(
        supervisor.0.try_wait().unwrap().is_none(),
        "the start returned"
    );
    // The node leads a group of its own, not the supervisor's, so a Ctrl-C at the terminal
    // reaches the supervisor, which stops the node in its order.
    assert_eq!(group_of(pid), pid);
    assert_ne!(group_of(pid), group_of(supervisor.0.id()));
    // The terminal's SIGINT ends the `start` command itself (by the signal, as a shell shows its
    // 130) and reaches the supervisor, which stops the node in its order and exits.
    interrupt(&mut supervisor);
    wait_until(Duration::from_secs(30), "the stop of the node", || {
        !node.locked("core") && !node.locked("core.supervisor")
    });
    assert!(
        node.log().contains("Received signal 2, stopping the core"),
        "{}",
        node.log()
    );
    assert!(!node.locked("relay") && !node.locked("forger"));
}

#[test]
fn a_one_node_core_devnet_forges() {
    let node = Node::new("forging", host(45));
    let mut core = node.start("core", &["core", "run", "--networkStart"]);
    node.await_api();
    wait_until(Duration::from_secs(120), "height 3", || {
        node.height().is_some_and(|height| height >= 3)
    });
    let log = node.output("core");
    assert!(log.contains("forged a new block"), "{log}");
    assert!(interrupt(&mut core).success(), "{}", node.output("core"));
}

#[test]
fn sigint_stops_a_core_node_within_30_seconds_in_the_core_order() {
    let node = Node::new("order", host(46));
    let mut core = node.start("core", &["core", "run", "--networkStart"]);
    node.await_api();
    wait_until(Duration::from_secs(120), "a forged block", || {
        node.output("core").contains("forged a new block")
    });
    let signalled = Instant::now();
    let status = interrupt(&mut core);
    assert!(status.success(), "{}", node.output("core"));
    assert!(signalled.elapsed() < Duration::from_secs(30));
    let log = node.output("core");
    // The signal, the relay's first steps (its server closes its forger's sessions, its API and
    // blockchain service stop), the forger's stop, then the relay's last steps (the pool is
    // flushed and the store closed): the order of the lines.
    let signal = position(&log, "Received signal 2, stopping the core", 0);
    let api = position(&log, "Public API (HTTP) Server stopped", signal);
    let chain = position(&log, "The blockchain has been stopped", api);
    let stopping = position(&log, "Stopping the forger", chain);
    let forger = position(&log, "The forger has stopped", stopping);
    let store = position(&log, "its store is closed", forger);
    assert!(signal < api && api < chain && chain < stopping && stopping < forger && forger < store);
    assert!(!node.locked("core") && !node.locked("relay") && !node.locked("forger"));
}

#[test]
fn a_remote_access_without_a_loopback_address_stops_the_start_naming_the_option() {
    let node = Node::new("remote", host(47));
    node.profile(json!(["10.0.0.1", "192.0.2.0/24"]));
    let refused = node.run(&["core", "run", "--networkStart"]);
    assert!(!refused.status.success(), "{}", text(&refused));
    assert!(
        text(&refused).contains("remoteAccess"),
        "{}",
        text(&refused)
    );
    // Nothing started: no lock is held and no port was bound.
    assert!(!node.locked("core") && !node.locked("relay") && !node.locked("forger"));
    assert!(node.api("/api/node/status").is_none());
}

#[test]
fn a_core_profile_without_the_forger_stops_the_start_naming_it() {
    let node = Node::new("no-forger", host(48));
    let app = fs::read_to_string(node.config().join("app.json")).unwrap();
    let mut app: Value = serde_json::from_str(&app).unwrap();
    let plugins = app["core"]["plugins"].as_array_mut().unwrap();
    plugins.retain(|plugin| plugin["package"] != "@solar-network/forger");
    fs::write(node.config().join("app.json"), app.to_string()).unwrap();
    let refused = node.run(&["core", "run", "--networkStart"]);
    assert!(!refused.status.success());
    assert!(
        text(&refused).contains("@solar-network/forger"),
        "{}",
        text(&refused)
    );
}

#[test]
fn the_verbose_version_names_a_test_seam_only_in_a_build_that_has_it() {
    let output = Command::new(env!("CARGO_BIN_EXE_heartwood"))
        .args(["version", "--verbose"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let shown = text(&output);
    assert!(shown.starts_with(heartwood_core::VERSION), "{shown}");
    assert!(shown.contains("target: "), "{shown}");
    assert_eq!(
        shown.contains("difftest"),
        cfg!(feature = "difftest"),
        "{shown}"
    );
    let plain = Command::new(env!("CARGO_BIN_EXE_heartwood"))
        .arg("version")
        .output()
        .unwrap();
    assert_eq!(text(&plain).trim(), heartwood_core::VERSION);
}
