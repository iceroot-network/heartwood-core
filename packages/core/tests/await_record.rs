//! The AwaitBlock record of a real relay (feature `difftest`, run with the integration tests): a
//! relay and its forger as separate `heartwood` processes, a client of the relay's own
//! peer-to-peer server, and the harness's job stall. The next job of the forger's block is held
//! for 12 s; a request that arrives at its start waits the bound (8 s) and ends `bound` with the
//! tip before the job, a request 6 s later sees the job commit and ends `drained` with the tip
//! after it, and a request after the commit finds the new tip. The forger's own public calls are
//! recorded too, and no line names an internal route.
//!
//! A failure prints the relay's log from a little before the hold, the forger's log and every
//! record line.
//!
//! The relay listens at the address `.33` of the loopback block of these tests (P2P 6002, API
//! 6003; the block `HEARTWOOD_TEST_LOOPBACK` names, else 127.13.0), and the client connects from
//! `.34`.

#![cfg(feature = "difftest")]

use std::fs;
use std::io::{Read, Write};
use std::net::{IpAddr, SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use heartwood_p2p::nes::{ConnectOptions, Session};
use heartwood_p2p::transport;
use heartwood_p2p_wire::RouteId;
use heartwood_p2p_wire::requests::{ClientRequest, Headers};
use serde_json::{Value, json};
use tokio::time::Instant;

const P2P_PORT: u16 = 6002;
const API_PORT: u16 = 6003;

/// The relay's log line when the stall holds a job.
const HOLD_LINE: &str = "difftest: holding the job for 12000 ms";

/// A process killed when dropped.
struct Killed(Child);

impl Drop for Killed {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// The address `last` of the tests' loopback block.
fn host(last: u8) -> String {
    let block = std::env::var("HEARTWOOD_TEST_LOOPBACK").unwrap_or_default();
    let block = if block.is_empty() { "127.13.0" } else { &block };
    format!("{block}.{last}")
}

/// The scratch root of a node whose chain `network generate` makes now: `config/`, the other
/// four directories and the harness's `harness/` beside it.
fn generate(root: &Path, host: &str) {
    let _ = fs::remove_dir_all(root);
    fs::create_dir_all(root.join("harness")).unwrap();
    let config = root.join("config");
    let generated = Command::new(env!("CARGO_BIN_EXE_heartwood"))
        .args([
            "network",
            "generate",
            "--network",
            "devnet",
            "--seed",
            "await",
        ])
        .args(["--out"])
        .arg(&config)
        .arg("--chainOnly")
        .output()
        .unwrap();
    assert!(generated.status.success(), "{generated:?}");
    let env = format!(
        "CORE_P2P_HOST={host}\nCORE_P2P_PORT={P2P_PORT}\nCORE_API_HOST={host}\nCORE_API_PORT={API_PORT}\nCORE_P2P_MIN_NETWORK_REACH=0\nCORE_LOG_LEVEL=debug\nHEARTWOOD_DIFFTEST_DIR={}\n",
        root.join("harness").display()
    );
    fs::write(config.join(".env"), env).unwrap();
    let peers = json!({"list": [{"ip": host, "port": P2P_PORT}]});
    fs::write(config.join("peers.json"), peers.to_string()).unwrap();
    let plugin = |name: &str| json!({"package": format!("@solar-network/{name}")});
    let p2p = json!({"package": "@solar-network/p2p", "options": {"remoteAccess": [host]}});
    let relay = ["logger", "state", "database", "transactions", "pool"].map(plugin);
    let relay: Vec<Value> = (relay.into_iter())
        .chain([p2p, plugin("blockchain"), plugin("api")])
        .collect();
    let hosts = json!([{"hostname": host, "port": P2P_PORT}]);
    let forger = json!({"package": "@solar-network/forger", "options": {"hosts": hosts, "localAddress": host}});
    let app = json!({
        "core": {"plugins": []},
        "relay": {"plugins": relay},
        "forger": {"plugins": [plugin("logger"), plugin("transactions"), forger]},
    });
    fs::write(config.join("app.json"), app.to_string()).unwrap();
}

/// Start `heartwood <role> run` of the node at `root`, its output kept in `<role>.log`.
fn start(root: &Path, role: &str, extra: &[&str]) -> Killed {
    let log = fs::File::create(root.join(format!("{role}.log"))).unwrap();
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
        command.env(format!("CORE_PATH_{kind}"), root.join(dir));
    }
    let child = (command
        .stdin(Stdio::null())
        .stdout(log.try_clone().unwrap())
        .stderr(log))
    .spawn()
    .unwrap();
    Killed(child)
}

/// The height of the relay's last block at `host`, when its API answers.
fn height(host: &str) -> Option<u64> {
    let mut stream = TcpStream::connect((host, API_PORT)).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    let request =
        format!("GET /api/blocks/last HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).ok()?;
    let mut response = String::new();
    stream.read_to_string(&mut response).ok()?;
    let (head, body) = response.split_once("\r\n\r\n")?;
    head.starts_with("HTTP/1.1 200").then_some(())?;
    serde_json::from_str::<Value>(body).ok()?["data"]["height"].as_u64()
}

async fn wait_until(limit: Duration, what: &str, mut done: impl FnMut() -> bool) {
    let start = Instant::now();
    while !done() {
        assert!(start.elapsed() < limit, "no {what} after {limit:?}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// A getPeers request on `session`, which ends with its answer's success.
async fn get_peers(session: Session) -> bool {
    let headers = Headers::new("4.3.1", 0).unwrap();
    let payload = ClientRequest::GetPeers.encode(&headers);
    let answer = session
        .request(RouteId::GetPeers, payload, Duration::from_secs(30), 1 << 20)
        .await;
    answer.is_ok()
}

/// The lines of the relay's await record: its one file, `await-NNNN.jsonl`.
fn records(dir: &Path) -> Vec<Value> {
    let files: Vec<PathBuf> = fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("");
            name.starts_with("await-") && name.ends_with(".jsonl")
        })
        .collect();
    assert_eq!(files.len(), 1, "{files:?}");
    let text = fs::read_to_string(&files[0]).unwrap();
    text.lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

/// Prints, when the test panics while it lives, what the node at its root did: the relay's log
/// from 20 lines before the hold (its last 80 lines when it has none), the forger's log and every
/// line of the await records, so that a failure away from this machine can be read.
struct Diagnosis<'a>(&'a Path);

impl Drop for Diagnosis<'_> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!("{}", diagnosis(self.0));
        }
    }
}

/// The diagnosis of the node at `root` (see [`Diagnosis`]).
fn diagnosis(root: &Path) -> String {
    let relay = log_text(&root.join("relay.log"));
    let lines: Vec<&str> = relay.lines().collect();
    let hold = lines.iter().position(|line| line.contains(HOLD_LINE));
    let from = hold.map_or(lines.len().saturating_sub(80), |at| at.saturating_sub(20));
    let forger = log_text(&root.join("forger.log"));
    let harness = root.join("harness");
    let mut files: Vec<PathBuf> = fs::read_dir(&harness)
        .map(|entries| {
            entries
                .filter_map(|entry| Some(entry.ok()?.path()))
                .collect()
        })
        .unwrap_or_default();
    files.retain(|path| {
        let name = path.file_name().and_then(|name| name.to_str());
        name.is_some_and(|name| name.starts_with("await-") && name.ends_with(".jsonl"))
    });
    files.sort();
    let records: Vec<String> = files.iter().map(|file| log_text(file)).collect();
    format!(
        "==== relay.log from line {} of {}:\n{}\n==== forger.log:\n{}\n==== await records ({} files):\n{}",
        from + 1,
        lines.len(),
        lines[from..].join("\n"),
        forger.trim_end(),
        files.len(),
        records.concat().trim_end(),
    )
}

/// The text of the file at `path` without its terminal colour sequences, or why it could not be
/// read.
fn log_text(path: &Path) -> String {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) => return format!("({} not read: {error})\n", path.display()),
    };
    let mut plain = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            // `ESC [`, its parameters and the letter that ends the sequence.
            chars.by_ref().find(char::is_ascii_alphabetic);
        } else {
            plain.push(c);
        }
    }
    plain
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "integration"]
async fn a_held_job_ends_waits_by_the_bound_and_the_record_shows_it() {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join("await-record");
    let (relay_host, client_host) = (host(33), host(34));
    generate(&root, &relay_host);
    // Dropped after the processes, so it reads their whole logs.
    let _diagnosis = Diagnosis(&root);
    let _relay = start(&root, "relay", &["--networkStart"]);
    wait_until(Duration::from_secs(60), "API", || {
        height(&relay_host).is_some()
    })
    .await;
    let _forger = start(&root, "forger", &[]);
    wait_until(Duration::from_secs(120), "height 3", || {
        height(&relay_host).is_some_and(|h| h >= 3)
    })
    .await;

    // The client of the relay's server, from its own address.
    let address = SocketAddr::new(relay_host.parse().unwrap(), P2P_PORT);
    let local: IpAddr = client_host.parse().unwrap();
    let (session, connected) = Session::connect(
        address,
        ConnectOptions::default(),
        Arc::new(|_| {}),
        transport::tcp_bound(local),
    );
    connected.await.unwrap();

    // The next job is held 12 s; its file goes when the job starts.
    let stall = root.join("harness").join("stall-next-job");
    fs::write(&stall, "12000").unwrap();
    wait_until(Duration::from_secs(60), "the job", || !stall.exists()).await;
    let before = height(&relay_host).unwrap();
    let started = Instant::now();
    let first = tokio::spawn(get_peers(session.clone()));
    tokio::time::sleep_until(started + Duration::from_secs(6)).await;
    let second = tokio::spawn(get_peers(session.clone()));
    assert!(first.await.unwrap() && second.await.unwrap());
    let elapsed = started.elapsed();
    assert!(elapsed > Duration::from_secs(11), "{elapsed:?}");
    wait_until(Duration::from_secs(30), "the commit", || {
        height(&relay_host).is_some_and(|h| h > before)
    })
    .await;
    assert!(get_peers(session.clone()).await);
    session.terminate();

    let log = fs::read_to_string(root.join("relay.log")).unwrap_or_default();
    assert!(log.contains(HOLD_LINE), "no hold in the relay's log");
    let all = records(&root.join("harness"));
    let caller = client_host.as_str();
    let ours: Vec<&Value> = all.iter().filter(|line| line["caller"] == caller).collect();
    assert_eq!(ours.len(), 3, "{all:?}");
    // Only public routes have lines, the forger's own (getStatus, postBlock, ...) among them.
    let public = |name: &str| {
        RouteId::ALL
            .iter()
            .any(|r| r.name() == name && !r.is_internal())
    };
    assert!(
        all.iter()
            .all(|line| public(line["route"].as_str().unwrap()))
    );
    assert!(all.iter().any(|line| line["caller"] != caller));
    let [first, second, third] = [ours[0], ours[1], ours[2]];
    assert_eq!(first["route"], "getPeers");
    assert_eq!(first["end"], "bound");
    let waited = first["waitMs"].as_u64().unwrap();
    assert!((8_000..8_500).contains(&waited), "{first}");
    assert_eq!(
        (&first["tipBefore"], &first["tipAfter"]),
        (&json!(before), &json!(before))
    );
    assert_eq!(second["end"], "drained");
    let waited = second["waitMs"].as_u64().unwrap();
    assert!((4_000..8_000).contains(&waited), "{second}");
    assert!(second["tipAfter"].as_u64().unwrap() > before, "{second}");
    assert!(third["tipBefore"].as_u64().unwrap() > before, "{third}");
    assert!(third["waitMs"].as_u64().unwrap() < 8_000, "{third}");
}
