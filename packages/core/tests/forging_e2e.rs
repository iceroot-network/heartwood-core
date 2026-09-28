//! A relay and its forger as separate `heartwood` processes, the forger's client over the relay's
//! own peer-to-peer server (run with the integration tests: they need the whole server): a
//! relay serves without a key file and with one it cannot read, a second relay of the instance
//! refuses, the forger forges the chain of a network `network generate` made, and SIGINT stops
//! the relay (its forger's sessions closed first) and then the forger, each within 30 s.
//!
//! The relay listens at 127.13.0.31 (P2P 6002, API 6003), in the loopback block of these tests;
//! its forger connects from the same address, which the relay's `remoteAccess` lists.

use std::fs;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::time::Instant;

const HOST: &str = "127.13.0.31";
const P2P_PORT: u16 = 6002;
const API_PORT: u16 = 6003;

/// A process killed when dropped.
struct Killed(Child);

impl Drop for Killed {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// A node's scratch root: `config/` (the chain of `network generate`, the node's files) and the
/// other four directories beside it.
struct Node {
    root: PathBuf,
}

impl Node {
    /// A node named `name` whose chain `network generate` makes now, with its epoch now.
    fn new(name: &str) -> Node {
        let root = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join("forging-e2e")
            .join(name);
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let node = Node { root };
        let config = node.config();
        let generated = Command::new(env!("CARGO_BIN_EXE_heartwood"))
            .args([
                "network",
                "generate",
                "--network",
                "devnet",
                "--seed",
                name,
                "--out",
            ])
            .arg(&config)
            .arg("--chainOnly")
            .output()
            .unwrap();
        assert!(generated.status.success(), "{generated:?}");
        let env = format!(
            "CORE_P2P_HOST={HOST}\nCORE_P2P_PORT={P2P_PORT}\nCORE_API_HOST={HOST}\nCORE_API_PORT={API_PORT}\nCORE_P2P_MIN_NETWORK_REACH=0\n"
        );
        fs::write(config.join(".env"), env).unwrap();
        fs::write(config.join("peers.json"), r#"{"list": []}"#).unwrap();
        let plugin = |name: &str| json!({"package": format!("@solar-network/{name}")});
        let p2p = json!({"package": "@solar-network/p2p", "options": {"remoteAccess": [HOST]}});
        let relay = ["logger", "state", "database", "transactions", "pool"].map(plugin);
        let relay: Vec<Value> = (relay.into_iter())
            .chain([p2p, plugin("blockchain"), plugin("api")])
            .collect();
        let hosts = json!([{"hostname": HOST, "port": P2P_PORT}]);
        let forger = json!({"package": "@solar-network/forger", "options": {"hosts": hosts, "localAddress": HOST}});
        let app = json!({
            "core": {"plugins": []},
            "relay": {"plugins": relay},
            "forger": {"plugins": [plugin("logger"), plugin("transactions"), forger]},
        });
        fs::write(config.join("app.json"), app.to_string()).unwrap();
        node
    }

    fn config(&self) -> PathBuf {
        self.root.join("config")
    }

    /// Start `heartwood <role> run` of this node, its output kept in `<name>.log`.
    fn start(&self, name: &str, role: &str, extra: &[&str]) -> Killed {
        let log = fs::File::create(self.root.join(format!("{name}.log"))).unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_heartwood"));
        command
            .args([role, "run", "--token", "e2e", "--network", "devnet"])
            .args(extra);
        for (kind, dir) in [
            ("CONFIG", "config"),
            ("DATA", "data"),
            ("CACHE", "cache"),
            ("LOG", "log"),
            ("TEMP", "temp"),
        ] {
            command.env(format!("CORE_PATH_{kind}"), self.root.join(dir));
        }
        let child = (command
            .stdin(Stdio::null())
            .stdout(log.try_clone().unwrap())
            .stderr(log))
        .spawn()
        .unwrap();
        Killed(child)
    }

    fn output(&self, name: &str) -> String {
        fs::read_to_string(self.root.join(format!("{name}.log"))).unwrap_or_default()
    }
}

/// `GET path` on the relay's API: the body's `data`, when it answers 200.
fn api(path: &str) -> Option<Value> {
    let mut stream = TcpStream::connect((HOST, API_PORT)).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    let request = format!("GET {path} HTTP/1.1\r\nHost: {HOST}\r\nConnection: close\r\n\r\n");
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
    let pid = child.0.id().to_string();
    assert!(
        Command::new("kill")
            .args(["-INT", &pid])
            .status()
            .unwrap()
            .success()
    );
    exit_within(child, Duration::from_secs(30))
}

#[test]
#[ignore = "integration"]
fn a_relay_serves_without_the_key_file_and_with_one_it_cannot_read_and_is_alone() {
    let node = Node::new("relay-keys");
    let key_file = node.config().join("delegates.json");
    fs::remove_file(&key_file).unwrap();
    for pass in 0..2 {
        let mut relay = node.start("relay", "relay", &["--networkStart"]);
        wait_until(Duration::from_secs(60), "API", || {
            api("/api/node/status").is_some()
        });
        if pass == 0 {
            let mut second = node.start("second", "relay", &["--networkStart"]);
            assert!(!exit_within(&mut second, Duration::from_secs(30)).success());
            assert!(
                node.output("second")
                    .contains("another relay of this instance holds")
            );
        }
        assert!(interrupt(&mut relay).success(), "{}", node.output("relay"));
        // The second pass: a key file that nobody can read, which the relay never opens.
        fs::write(&key_file, r#"{"keys": []}"#).unwrap();
        fs::set_permissions(&key_file, fs::Permissions::from_mode(0o000)).unwrap();
    }
}

#[test]
#[ignore = "integration"]
fn a_forger_forges_through_its_relays_server_and_both_stop_on_sigint() {
    let node = Node::new("forging");
    let mut relay = node.start("relay", "relay", &["--networkStart"]);
    wait_until(Duration::from_secs(60), "API", || {
        api("/api/node/status").is_some()
    });
    let mut forger = node.start("forger", "forger", &[]);
    let height = || api("/api/blocks/last").and_then(|block| block["height"].as_u64());
    wait_until(Duration::from_secs(120), "height 3", || {
        height().is_some_and(|h| h >= 3)
    });
    let log = node.output("forger");
    assert!(log.contains("forged a new block"), "{log}");
    // The relay first: it closes its forger's sessions, then stops.
    assert!(interrupt(&mut relay).success(), "{}", node.output("relay"));
    assert!(
        interrupt(&mut forger).success(),
        "{}",
        node.output("forger")
    );
    assert!(
        node.output("relay")
            .contains("Received signal 2, stopping the relay")
    );
}
