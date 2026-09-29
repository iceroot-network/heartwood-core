//! The relay end to end (feature `difftest`, run with the integration tests): `heartwood relay
//! run` with the harness's seed pointing at the standalone mock peer on this host imports the
//! committed one-node corpus, serves `/api/blocks/last` at its tip, writes the judge's line for the
//! tip to its digest file, and stops gracefully on SIGINT; a relay whose API cannot listen stops
//! with an error once its blockchain has started.

#![cfg(feature = "difftest")]

mod common;

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use common::{Setup, corpus};
use serde_json::Value;

/// A process killed when dropped.
struct Killed(Child);

impl Drop for Killed {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// A port nothing listens on now.
fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// The mock peer binary of the peer-to-peer crate, built when it is not there yet.
fn mock_peer() -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    let dir = exe
        .parent()
        .and_then(|deps| deps.parent())
        .unwrap()
        .to_path_buf();
    let binary = dir.join("mock-peer");
    if !binary.exists() {
        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
        let args = [
            "build",
            "--locked",
            "-p",
            "heartwood-p2p",
            "--features",
            "difftest",
            "--bin",
            "mock-peer",
        ];
        assert!(Command::new(cargo).args(args).status().unwrap().success());
    }
    binary
}

/// `GET path` on the API at `port`: the status line and the body, when it answers.
fn get(port: u16, path: &str) -> Option<(String, Value)> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    let request = format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).ok()?;
    let mut response = String::new();
    stream.read_to_string(&mut response).ok()?;
    let (head, body) = response.split_once("\r\n\r\n")?;
    Some((
        head.lines().next()?.to_owned(),
        serde_json::from_str(body).ok()?,
    ))
}

#[test]
#[ignore = "integration"]
fn a_relay_imports_the_corpus_from_a_mock_peer_and_serves_its_tip() {
    let api = free_port();
    let env = format!(
        "CORE_API_PORT={api}\nCORE_P2P_PORT={}\nCORE_P2P_MIN_NETWORK_REACH=1\nCORE_LOG_LEVEL=info\n",
        free_port()
    );
    let setup = Setup::new("relay", &env);
    setup.write("peers.json", r#"{"list": []}"#);
    let mut peer = Command::new(mock_peer())
        .args(["--listen", "127.0.0.1:0", "--chain"])
        .arg(corpus())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut lines = BufReader::new(peer.stdout.take().unwrap()).lines();
    let peer = Killed(peer);
    let listening = lines.next().unwrap().unwrap();
    let address = listening.rsplit(' ').next().unwrap().to_owned();
    let digests = setup.root.join("digests.jsonl");
    let mut relay = Command::new(env!("CARGO_BIN_EXE_heartwood"));
    relay
        .args([
            "relay",
            "run",
            "--token",
            "solar",
            "--network",
            "testnet",
            "--difftest-seed",
            &address,
        ])
        .arg("--difftest-digest")
        .arg(&digests)
        .env_clear()
        .env("HOME", setup.home())
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .envs(setup.process(&[]))
        .stdout(Stdio::null());
    let mut relay = Killed(relay.spawn().unwrap());
    // The tip as the corpus's last block line gives it.
    let blocks = std::fs::read_to_string(corpus().join("blocks.jsonl")).unwrap();
    let tip: Value = serde_json::from_str(blocks.lines().last().unwrap()).unwrap();
    // At most 300 s, polling every half second.
    let mut polls = 0;
    let served = loop {
        polls += 1;
        assert!(relay.0.try_wait().unwrap().is_none(), "the relay stopped");
        assert!(
            polls <= 600,
            "the relay did not serve the corpus's tip in time"
        );
        match get(api, "/api/blocks/last") {
            Some((status, body)) if body["data"]["height"] == tip["h"] => break (status, body),
            _ => std::thread::sleep(Duration::from_millis(500)),
        }
    };
    assert!(served.0.contains("200"), "{}", served.0);
    assert_eq!(served.1["data"]["id"], tip["id"]);
    // The digest file's line for the tip is the judge's.
    let tip_line = |file: PathBuf| {
        let text = std::fs::read_to_string(file).unwrap();
        let lines = text
            .lines()
            .map(|l| serde_json::from_str::<Value>(l).unwrap());
        lines
            .rev()
            .find(|line| line["h"] == tip["h"] && line["step"] == "apply")
            .unwrap()
    };
    assert_eq!(
        tip_line(digests)["sd"],
        tip_line(corpus().join("ts-digests.jsonl"))["sd"]
    );
    // SIGINT: a graceful stop with status 0.
    let pid = relay.0.id().to_string();
    assert!(
        Command::new("kill")
            .args(["-INT", &pid])
            .status()
            .unwrap()
            .success()
    );
    let mut waits = 0;
    let status = loop {
        if let Some(status) = relay.0.try_wait().unwrap() {
            break status;
        }
        waits += 1;
        assert!(waits <= 300, "the relay did not stop within 30 s");
        std::thread::sleep(Duration::from_millis(100));
    };
    assert_eq!(status.code(), Some(0));
    drop(peer);
}

#[test]
#[ignore = "integration"]
fn a_relay_whose_api_cannot_listen_stops_with_an_error() {
    let taken = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = taken.local_addr().unwrap().port();
    let env = format!(
        "CORE_API_HOST=127.0.0.1\nCORE_API_PORT={port}\nCORE_P2P_PORT={}\n",
        free_port()
    );
    let setup = Setup::new("api-taken", &env);
    let mut relay = Command::new(env!("CARGO_BIN_EXE_heartwood"));
    relay
        .args(["relay", "run", "--token", "solar", "--network", "testnet"])
        .args(["--networkStart", "--skipDiscovery"])
        .env_clear()
        .env("HOME", setup.home())
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .envs(setup.process(&[]))
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let mut relay = Killed(relay.spawn().unwrap());
    // The blockchain starts at once on a new network; at most 120 s.
    let mut waits = 0;
    let status = loop {
        if let Some(status) = relay.0.try_wait().unwrap() {
            break status;
        }
        waits += 1;
        assert!(waits <= 1200, "the relay did not stop within 120 s");
        std::thread::sleep(Duration::from_millis(100));
    };
    let mut stderr = String::new();
    let mut pipe = relay.0.stderr.take().unwrap();
    pipe.read_to_string(&mut stderr).unwrap();
    assert_eq!(status.code(), Some(1), "{stderr}");
    assert!(
        stderr.contains(&format!("cannot listen on 127.0.0.1:{port}")),
        "{stderr}"
    );
    drop(taken);
}

/// A temp directory that cannot be made stops no start: the relay reads only the reference's
/// marker files there. It stops here because its API cannot listen, once its blockchain started.
#[test]
#[ignore = "integration"]
fn a_relay_starts_when_its_temp_directory_cannot_be_made() {
    let taken = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = taken.local_addr().unwrap().port();
    let env = format!(
        "CORE_API_HOST=127.0.0.1\nCORE_API_PORT={port}\nCORE_P2P_PORT={}\n",
        free_port()
    );
    let setup = Setup::new("temp-blocked", &env);
    let blocked = setup.root.join("a-file");
    std::fs::write(&blocked, "").unwrap();
    let temp = blocked.join("temp").display().to_string();
    let mut relay = Command::new(env!("CARGO_BIN_EXE_heartwood"));
    relay
        .args(["relay", "run", "--token", "solar", "--network", "testnet"])
        .args(["--networkStart", "--skipDiscovery"])
        .env_clear()
        .env("HOME", setup.home())
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .envs(setup.process(&[("CORE_PATH_TEMP", &temp)]))
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let mut relay = Killed(relay.spawn().unwrap());
    let mut waits = 0;
    let status = loop {
        if let Some(status) = relay.0.try_wait().unwrap() {
            break status;
        }
        waits += 1;
        assert!(waits <= 1200, "the relay did not stop within 120 s");
        std::thread::sleep(Duration::from_millis(100));
    };
    let mut stderr = String::new();
    let mut pipe = relay.0.stderr.take().unwrap();
    pipe.read_to_string(&mut stderr).unwrap();
    assert_eq!(status.code(), Some(1), "{stderr}");
    assert!(
        stderr.contains(&format!("cannot listen on 127.0.0.1:{port}")),
        "{stderr}"
    );
    drop(taken);
}
