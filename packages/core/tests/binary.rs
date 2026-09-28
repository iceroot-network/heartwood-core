//! The `heartwood` binary as its operators run it: the forger process against a mock relay (it
//! opens no listening socket, forges through its relay, refuses a second forger of its instance,
//! and stops on SIGINT after its post in flight), the forger's refusals of its key file and its
//! profile, `start` and `stop` of a role with their refusals, the supervisor's `errored` state,
//! the colon spellings, `network generate` against the genesis vectors, and a relay's refusal of
//! devnet addressing outside its guards. The relay's other tests, which need the peer-to-peer
//! server, are in `forging_e2e.rs` and run with the integration tests.
//!
//! Every relay and forger address of these tests is in the loopback block 127.13.0.0/24.

mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
#[cfg(feature = "difftest")]
use std::process::{Child, Stdio};
use std::process::{Command, Output};
use std::time::Duration;

use tokio::time::Instant;

use heartwood_crypto::KeyPair;
use serde_json::{Value, json};

#[cfg(feature = "difftest")]
/// The forger tests' relay address.
const RELAY_IP: [u8; 4] = [127, 13, 0, 21];

/// The `heartwood` binary.
fn heartwood() -> Command {
    Command::new(env!("CARGO_BIN_EXE_heartwood"))
}

#[cfg(feature = "difftest")]
/// A process killed when dropped.
struct Killed(Child);

#[cfg(feature = "difftest")]
impl Drop for Killed {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// A node's directories for one test: `config/` with the committed corpus's crypto files, and
/// the five `CORE_PATH_*` in the scratch root.
struct Node {
    root: PathBuf,
}

impl Node {
    fn new(name: &str) -> Node {
        let root = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join("core-binary")
            .join(name);
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("config/crypto")).unwrap();
        for file in [
            "network.json",
            "milestones.json",
            "genesisBlock.json",
            "exceptions.json",
        ] {
            let source = common::corpus().join("crypto").join(file);
            fs::copy(source, root.join("config/crypto").join(file)).unwrap();
        }
        fs::write(root.join("config/.env"), "").unwrap();
        Node { root }
    }

    fn config(&self) -> PathBuf {
        self.root.join("config")
    }

    fn data(&self) -> PathBuf {
        self.root.join("data")
    }

    /// `heartwood <args>` with this node's directories.
    fn command(&self, args: &[&str]) -> Command {
        let mut command = heartwood();
        command.args(args);
        for (kind, dir) in [
            ("CONFIG", "config"),
            ("DATA", "data"),
            ("CACHE", "cache"),
            ("LOG", "log"),
            ("TEMP", "temp"),
        ] {
            command.env(format!("CORE_PATH_{kind}"), self.root.join(dir));
        }
        command.env("CORE_LOG_LEVEL", "debug");
        command
    }

    /// Run `heartwood <args>` to its end.
    fn run(&self, args: &[&str]) -> Output {
        self.command(args).output().unwrap()
    }

    /// Start `heartwood <args>`, its output kept in `<name>.out` and `<name>.err`.
    #[cfg(feature = "difftest")]
    fn spawn(&self, name: &str, args: &[&str]) -> Killed {
        let out = fs::File::create(self.root.join(format!("{name}.out"))).unwrap();
        let err = fs::File::create(self.root.join(format!("{name}.err"))).unwrap();
        let child = (self.command(args).stdin(Stdio::null()))
            .stdout(out)
            .stderr(err)
            .spawn()
            .unwrap();
        Killed(child)
    }

    /// The output of the process started as `name`, so far.
    #[cfg(feature = "difftest")]
    fn output(&self, name: &str) -> String {
        let read = |ext: &str| fs::read_to_string(self.root.join(format!("{name}.{ext}")));
        format!(
            "{}{}",
            read("out").unwrap_or_default(),
            read("err").unwrap_or_default()
        )
    }

    /// Write the key file with `keys` and `mode`.
    fn keys(&self, keys: &[&KeyPair], mode: u32) {
        let keys: Vec<String> = keys
            .iter()
            .map(|k| k.secret_key().to_hex().to_string())
            .collect();
        let path = self.config().join("delegates.json");
        fs::write(&path, json!({ "keys": keys }).to_string()).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
    }

    /// Write `app.json` with the forger profile `forger` (its plugins) and empty other profiles.
    fn app(&self, forger: Value) {
        let app = json!({"core": {"plugins": []}, "relay": {"plugins": []}, "forger": {"plugins": forger}});
        fs::write(self.config().join("app.json"), app.to_string()).unwrap();
    }

    /// A forger profile whose relay is `relay`.
    fn forger_profile(&self, relay: std::net::SocketAddr) {
        let hosts = json!([{"hostname": relay.ip().to_string(), "port": relay.port()}]);
        self.app(json!([
            {"package": "@solar-network/logger"},
            {"package": "@solar-network/transactions"},
            {"package": "@solar-network/forger", "options": {"hosts": hosts}},
        ]));
    }
}

/// The test's delegate key.
fn delegate() -> KeyPair {
    KeyPair::from_passphrase("heartwood binary test delegate").unwrap()
}

/// Wait until `done` holds, at most `limit`.
fn wait_until(limit: Duration, what: &str, mut done: impl FnMut() -> bool) {
    let start = Instant::now();
    while !done() {
        assert!(start.elapsed() < limit, "no {what} after {limit:?}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[cfg(feature = "difftest")]
/// Wait for `child` to exit, at most `limit`; its exit status.
fn exit_within(child: &mut Killed, limit: Duration) -> std::process::ExitStatus {
    let start = Instant::now();
    loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            return status;
        }
        assert!(start.elapsed() < limit, "still running after {limit:?}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[cfg(feature = "difftest")]
fn signal(child: &Killed, name: &str) {
    let pid = child.0.id().to_string();
    let status = Command::new("kill").args([name, &pid]).status().unwrap();
    assert!(status.success());
}

#[cfg(feature = "difftest")]
/// The listening TCP sockets of the process `pid`.
fn listening_sockets(pid: u32) -> Vec<String> {
    let inodes: Vec<String> = fs::read_dir(format!("/proc/{pid}/fd"))
        .unwrap()
        .filter_map(|entry| {
            let link = fs::read_link(entry.ok()?.path()).ok()?;
            let link = link.to_string_lossy();
            let inode = link.strip_prefix("socket:[")?.strip_suffix(']')?;
            Some(inode.to_owned())
        })
        .collect();
    let mut listening = Vec::new();
    for table in ["/proc/net/tcp", "/proc/net/tcp6"] {
        for line in fs::read_to_string(table)
            .unwrap_or_default()
            .lines()
            .skip(1)
        {
            let fields: Vec<&str> = line.split_whitespace().collect();
            if fields.get(3) == Some(&"0A")
                && fields
                    .get(9)
                    .is_some_and(|i| inodes.contains(&(*i).to_owned()))
            {
                listening.push(line.to_owned());
            }
        }
    }
    listening
}

#[test]
fn a_forger_refuses_a_key_file_it_cannot_trust_with_the_references_messages() {
    let node = Node::new("forger-keys");
    node.forger_profile(([127, 13, 0, 29], 6002).into());
    let run = || node.run(&["forger", "run", "--token", "t", "--network", "devnet"]);
    let refusal = |output: Output| {
        assert!(!output.status.success());
        String::from_utf8_lossy(&output.stderr).into_owned()
    };
    assert!(refusal(run()).contains("delegates.json file does not exist"));
    node.keys(&[&delegate()], 0o620);
    assert!(refusal(run()).contains("is open to its group or to others (mode 620)"));
    node.keys(&[], 0o600);
    assert!(refusal(run()).contains("Unable to detect any private keys"));
}

#[test]
fn a_forger_profile_runs_only_the_forger_and_needs_it() {
    let node = Node::new("forger-profile");
    node.keys(&[&delegate()], 0o600);
    node.app(json!([{"package": "@solar-network/logger"}, {"package": "@solar-network/pool"}]));
    let run = || node.run(&["forger", "run", "--token", "t", "--network", "devnet"]);
    let output = run();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("@solar-network/pool"));
    node.app(json!([{"package": "@solar-network/logger"}]));
    let output = run();
    assert!(!output.status.success());
    let text = String::from_utf8_lossy(&output.stderr);
    assert!(text.contains("has no @solar-network/forger"), "{text}");
}

#[test]
fn start_refuses_a_running_role_and_stop_a_stopped_one() {
    let node = Node::new("forger-start");
    node.keys(&[&delegate()], 0o600);
    // No relay answers: the forger waits for one.
    node.forger_profile(([127, 13, 0, 28], 6002).into());
    let instance = ["--token", "t", "--network", "devnet"];
    let with = |command: &[&'static str]| [command, &instance[..]].concat();
    let stopped = node.run(&with(&["forger", "stop"]));
    assert!(!stopped.status.success(), "a stopped forger cannot stop");
    let started = node.run(&with(&["forger", "start", "--daemon"]));
    assert!(started.status.success(), "{started:?}");
    wait_until(Duration::from_secs(20), "forger's pid file", || {
        node.data().join("run/forger.pid").exists()
    });
    let again = node.run(&with(&["forger:start", "--daemon"]));
    assert!(!again.status.success(), "a running forger cannot start");
    let status = node.run(&with(&["forger", "status"]));
    let table = String::from_utf8_lossy(&status.stdout);
    assert!(
        table.contains("t-devnet-forger") && table.contains("online"),
        "{table}"
    );
    let stop = node.run(&with(&["forger", "stop"]));
    assert!(stop.status.success(), "{stop:?}");
    assert!(!node.data().join("run/forger.pid").exists());
    assert!(!node.run(&with(&["forger", "stop"])).status.success());
}

#[test]
fn the_supervisor_marks_a_role_errored_after_five_runs_under_a_second() {
    let node = Node::new("supervisor-errored");
    let output = node.run(&[
        "supervise",
        "--role",
        "relay",
        "--token",
        "t",
        "--network",
        "devnet",
        "--",
        "/bin/true",
    ]);
    assert!(!output.status.success());
    let state = fs::read_to_string(node.data().join("run/relay.state.json")).unwrap();
    let state: Value = serde_json::from_str(&state).unwrap();
    assert_eq!(state["status"], "errored", "{state}");
}

#[test]
fn the_colon_spellings_are_the_same_commands() {
    let help = |args: &[&str]| heartwood().args(args).output().unwrap().stdout;
    assert_eq!(
        help(&["relay:start", "--help"]),
        help(&["relay", "start", "--help"])
    );
    assert_eq!(
        help(&["forger:log", "--help"]),
        help(&["forger", "log", "--help"])
    );
    let version = heartwood().arg("version").output().unwrap();
    assert!(String::from_utf8_lossy(&version.stdout).starts_with("4.3.1-heartwood."));
}

#[test]
fn a_relay_with_devnet_addressing_refuses_to_start_outside_the_guards() {
    let node = Node::new("relay-devnet-guards");
    let peers = r#"{"list": [{"ip": "127.13.0.41", "port": 6002}]}"#;
    fs::write(node.config().join("peers.json"), peers).unwrap();
    let address = "127.13.0.40";
    let relay = |remote_access: &[&str], host: Option<&str>| {
        let p2p = json!({
            "package": "@solar-network/p2p",
            "options": {
                "remoteAccess": remote_access,
                "devnet": { "addressing": "alias", "address": address },
            },
        });
        let mut plugins = common::relay_plugins();
        let list = plugins.as_array_mut().unwrap();
        let at = list
            .iter()
            .position(|p| p["package"] == "@solar-network/p2p");
        list[at.unwrap()] = p2p;
        let app = json!({"core": {"plugins": []}, "relay": {"plugins": plugins}, "forger": {"plugins": []}});
        fs::write(node.config().join("app.json"), app.to_string()).unwrap();
        let env = host.map(|host| format!("CORE_P2P_HOST={host}\n"));
        fs::write(node.config().join(".env"), env.unwrap_or_default()).unwrap();
        // A relay that passed the guards would run on: it is killed after 60 s and fails.
        let args = ["relay", "run", "--token", "t", "--network", "devnet"];
        let mut command = node.command(&args);
        let stdio = || std::process::Stdio::piped();
        let mut child = command.stdout(stdio()).stderr(stdio()).spawn().unwrap();
        let started = Instant::now();
        while child.try_wait().unwrap().is_none() {
            if started.elapsed() > Duration::from_secs(60) {
                let _ = child.kill();
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let output = child.wait_with_output().unwrap();
        assert!(!output.status.success());
        String::from_utf8_lossy(&output.stderr).into_owned()
    };
    let refused = relay(&[address], None);
    let expected = format!("the server listens on 0.0.0.0, not on the node's address {address}");
    assert!(refused.contains(&expected), "{refused}");
    let refused = relay(&["10.0.0.1"], Some(address));
    assert!(
        refused.contains("remoteAccess must list loopback addresses only"),
        "{refused}"
    );
}

#[test]
fn network_generate_writes_the_genesis_vectors_byte_for_byte() {
    let text = include_str!("../../crypto/tests/vectors/V10-genesis.jsonl");
    let records: Vec<Value> = (text.lines().filter(|line| !line.trim().is_empty()).skip(1))
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(records.len(), 3);
    for (k, record) in records.iter().enumerate() {
        let node = Node::new(&format!("generate-{k}"));
        let out = node.root.join("out");
        let (seed, epoch) = (
            record["input"]["seed"].as_str().unwrap(),
            record["input"]["epoch"].as_str().unwrap(),
        );
        let output = node.run(&[
            "network",
            "generate",
            "--network",
            "devnet",
            "--seed",
            seed,
            "--epoch",
            epoch,
            "--out",
            out.to_str().unwrap(),
            "--chainOnly",
        ]);
        assert!(output.status.success(), "{output:?}");
        for (file, text) in record["output"]["files"].as_object().unwrap() {
            assert_eq!(
                fs::read_to_string(out.join(file)).unwrap(),
                text.as_str().unwrap(),
                "{seed} {file}"
            );
        }
        let keys = fs::metadata(out.join("delegates.json"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(keys & 0o777, 0o600);
    }
}

#[cfg(feature = "difftest")]
mod against_a_mock_relay {
    use std::net::SocketAddr;
    use std::sync::{Arc, Mutex};

    use heartwood_crypto::Block;
    use heartwood_crypto::crypto::slots;
    use heartwood_kernel::config::crypto;
    use heartwood_p2p::testing::{Answer, MockPeer, Reply};
    use heartwood_p2p_wire::RouteId;
    use heartwood_p2p_wire::codecs::{blocks, transactions};
    use heartwood_p2p_wire::internal::{
        self, CurrentRound, DelegateWallet, InternalBody, LastBlockData, NetworkState,
        NetworkStateStatus, QuorumDetails, SlotNumber, SlotNumberRequest,
    };
    use heartwood_p2p_wire::messages::Proto;
    use heartwood_p2p_wire::messages::blocks::PostBlockResponse;
    use heartwood_p2p_wire::status::{
        self, NetworkConfig, PeerConfig, SignedState, StatusHeaderData, TokenConfig,
    };
    use serde_json::{Map, Value, json};

    use super::*;

    fn reply<T: InternalBody>(body: &T) -> Reply {
        Reply::Payload(internal::encode(body))
    }

    fn object(value: Value) -> Map<String, Value> {
        let Value::Object(fields) = value else {
            unreachable!("an object")
        };
        fields
    }

    fn now_ms() -> i64 {
        use heartwood_kernel::Clock;
        heartwood_kernel::SystemClock.now_ms()
    }

    /// A relay whose delegate of every slot is the test's, which takes each block after `hold`.
    struct Relay {
        epoch: i64,
        block_time: u64,
        wallet: DelegateWallet,
        public_key: String,
        milestones: heartwood_crypto::Milestones,
        hold: Duration,
        posted: Mutex<Vec<(i64, Instant)>>,
    }

    impl Relay {
        fn new(node: &Node, hold: Duration) -> Relay {
            let files = crypto::load(&node.config().join("crypto")).unwrap();
            let params = files.milestones.at(1);
            let public_key = delegate().public_key().to_compressed().to_hex();
            let wallet = object(
                json!({"address": "", "publicKey": public_key, "balance": "0", "nonce": "0",
                "delegate": {"username": "genesis_1", "voteBalance": "0", "rank": 1}}),
            );
            Relay {
                epoch: slots::epoch_unix_ms(params.epoch()).unwrap(),
                block_time: params.block_time(),
                wallet: DelegateWallet::new(wallet).unwrap(),
                public_key,
                milestones: files.milestones.clone(),
                hold,
                posted: Mutex::default(),
            }
        }

        fn answer(&self, route: RouteId, payload: &[u8]) -> Reply {
            let now = now_ms();
            let time = slots::epoch_time(now, self.epoch);
            let slot = slots::slot_number(time, self.block_time);
            let height = u32::try_from(slot.max(1)).unwrap();
            match route {
                RouteId::GetCurrentRound => reply(&CurrentRound {
                    all_delegates: vec![self.wallet.clone()],
                    can_forge: true,
                    current: 1,
                    current_forger: Some(self.wallet.clone()),
                    delegates: vec![self.wallet.clone()],
                    last_block: LastBlockData::new(object(
                        json!({"height": height, "id": "0".repeat(64)}),
                    ))
                    .unwrap(),
                    next_forger: Some(self.wallet.clone()),
                    reward: "0".into(),
                    timestamp: slots::slot_time(slot, self.block_time),
                }),
                RouteId::GetSlotNumber => {
                    let asked = internal::decode::<SlotNumberRequest>(payload).ok();
                    let timestamp = asked.and_then(|request| request.timestamp).unwrap_or(time);
                    reply(&SlotNumber(slots::slot_number(timestamp, self.block_time)))
                }
                RouteId::GetNetworkState => reply(&NetworkState {
                    status: NetworkStateStatus::Default,
                    node_height: Some(height),
                    last_block_id: Some("0".repeat(64)),
                    last_generator: None,
                    last_slot_number: Some(slot - 1),
                    quorum_details: QuorumDetails {
                        has_quorum: 53,
                        ..QuorumDetails::default()
                    },
                }),
                RouteId::GetStatus => {
                    let header = StatusHeaderData {
                        id: "0".repeat(64),
                        version: 0,
                        timestamp: u32::try_from(slots::slot_time(slot - 1, self.block_time))
                            .unwrap(),
                        previous_block: "0".repeat(64),
                        height,
                        number_of_transactions: 0,
                        total_amount: 0,
                        total_fee: 0,
                        reward: 0,
                        payload_length: 0,
                        payload_hash: "00".repeat(32),
                        generator_public_key: self.public_key.clone(),
                        block_signature: "00".repeat(64),
                    };
                    let state = SignedState {
                        height,
                        forging_allowed: true,
                        current_slot: slot,
                        header,
                    };
                    let config = PeerConfig {
                        version: "4.3.1".into(),
                        network: NetworkConfig {
                            version: 90,
                            name: "devnet".into(),
                            nethash: "0".repeat(64),
                            explorer: String::new(),
                            token: TokenConfig {
                                name: "dROOT".into(),
                                symbol: "dRT".into(),
                            },
                        },
                        plugins: Vec::new(),
                    };
                    Reply::Payload(status::response(&state, &config, &[]).encode())
                }
                RouteId::GetUnconfirmedTransactions => Reply::Payload(
                    transactions::get_unconfirmed_transactions_encode_response(&Default::default()),
                ),
                RouteId::PostBlock => {
                    let posted = blocks::post_block_decode_request(payload).ok();
                    let bytes = posted.and_then(|request| request.block).unwrap_or_default();
                    let block = Block::decode_trusted(&bytes, &self.milestones).unwrap();
                    self.posted
                        .lock()
                        .unwrap()
                        .push((i64::from(block.header().timestamp), Instant::now()));
                    let answer = PostBlockResponse {
                        status: Some(true),
                        height: Some(block.height()),
                        ..PostBlockResponse::default()
                    };
                    let answer = Reply::Payload(blocks::post_block_encode_response(&answer));
                    Reply::After(self.hold, Box::new(answer))
                }
                _ => Reply::Silence,
            }
        }
    }

    /// The relay of `node`, holding each post `hold`, on a runtime of its own.
    fn relay(node: &Node, hold: Duration) -> (Arc<Relay>, SocketAddr, tokio::runtime::Runtime) {
        let relay = Arc::new(Relay::new(node, hold));
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        let answering = Arc::clone(&relay);
        let answer: Answer = Arc::new(move |route, payload| answering.answer(route, payload));
        let peer = runtime
            .block_on(MockPeer::answering((RELAY_IP, 0).into(), answer))
            .unwrap();
        let address = peer.address();
        runtime.spawn(async move {
            let _peer = peer;
            std::future::pending::<()>().await;
        });
        (relay, address, runtime)
    }

    #[test]
    fn a_forger_forges_through_its_relay_opens_no_listening_socket_and_is_alone() {
        let node = Node::new("forger-mock-relay");
        node.keys(&[&delegate()], 0o600);
        let (relay, address, _runtime) = relay(&node, Duration::ZERO);
        node.forger_profile(address);
        let args = ["forger", "run", "--token", "t", "--network", "devnet"];
        let forger = node.spawn("forger", &args);
        wait_until(Duration::from_secs(30), "posted block", || {
            !relay.posted.lock().unwrap().is_empty()
        });
        assert_eq!(listening_sockets(forger.0.id()), Vec::<String>::new());
        let second = node.run(&args);
        assert!(!second.status.success());
        let text = String::from_utf8_lossy(&second.stderr);
        assert!(
            text.contains("another forger of this instance holds"),
            "{text}"
        );
        assert!(
            node.output("forger").contains("forged a new block"),
            "{}",
            node.output("forger")
        );
    }

    #[test]
    fn sigint_stops_the_forger_once_its_post_in_flight_is_answered() {
        let node = Node::new("forger-sigint");
        node.keys(&[&delegate()], 0o600);
        let hold = Duration::from_secs(3);
        let (relay, address, _runtime) = relay(&node, hold);
        node.forger_profile(address);
        let mut forger = node.spawn(
            "forger",
            &["forger", "run", "--token", "t", "--network", "devnet"],
        );
        wait_until(Duration::from_secs(30), "posted block", || {
            !relay.posted.lock().unwrap().is_empty()
        });
        let posted = relay.posted.lock().unwrap()[0].1;
        signal(&forger, "-INT");
        let status = exit_within(&mut forger, Duration::from_secs(30));
        assert!(status.success(), "{status:?}: {}", node.output("forger"));
        assert!(
            posted.elapsed() >= hold,
            "the post in flight was left before its answer"
        );
        assert!(
            node.output("forger")
                .contains("Received signal 2, stopping the forger")
        );
    }
}
