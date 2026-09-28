//! The devnet library against fakes: a chain generator that writes small crypto files, process
//! operations that record every call, and relay APIs scripted per address. Every devnet here uses
//! the loopback block 127.13.0.0/24, and the tests that bind its addresses run one at a time.

#![cfg(feature = "devnet")]

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::fs;
use std::net::{IpAddr, SocketAddr, TcpListener};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use heartwood_cli::devnet::{
    self, Consistency, Failover, InitOptions, LogStream, Manifest, NodeSelection, Tools,
};
use heartwood_cli::errors::{DevnetError, HttpError, NetworkError, ProcessError};
use heartwood_cli::network::NetworkGenerator;
use heartwood_cli::seams::{
    ApiBlock, ApiNodeStatus, ApiPeer, ChainGenerator, Env, GenerateOptions, Generated,
    GeneratedDelegate, Instance, NodeApi, ProcessOps, Role, StatusRow, SupervisedStatus,
};
use heartwood_crypto::crypto::hash::sha256;
use heartwood_crypto::utils::hex;
use heartwood_kernel::{Clock, ManualClock};
use zeroize::Zeroizing;

/// The address block of this file's devnets.
const PREFIX: [u8; 3] = [127, 13, 0];
/// The clock at `init`: 2026-09-21T10:13:20.000Z.
const T0: i64 = 1_790_158_400_000;
const NETHASH: &str = "ab12";

static SERIAL: Mutex<()> = Mutex::new(());

/// Run one at a time: the devnets bind the same addresses.
fn serial() -> MutexGuard<'static, ()> {
    SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// An empty directory for the test `name`.
fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("devnet-tests")
        .join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn epoch(ms: i64) -> String {
    heartwood_kernel::timestamp::UtcDateTime::from_unix_ms(ms)
        .unwrap()
        .to_iso_string()
}

fn ip(k: u8) -> IpAddr {
    IpAddr::from([127, 13, 0, k])
}

fn api(k: u8) -> SocketAddr {
    SocketAddr::new(ip(10 + k), 6003)
}

/// A generator that writes four small crypto files and 53 delegates with keys `{i:064x}`.
struct FakeChain {
    name: &'static str,
}

impl ChainGenerator for FakeChain {
    fn generate(
        &self,
        options: &GenerateOptions,
        _: &dyn Clock,
    ) -> Result<Generated, NetworkError> {
        assert_eq!(
            (options.network.as_str(), options.chain_only),
            ("devnet", true)
        );
        let dir = options.out.join("crypto");
        fs::create_dir_all(&dir).unwrap();
        let epoch = options.epoch.clone().unwrap();
        let network = format!(
            "{{\n    \"name\": \"{}\",\n    \"nethash\": \"{NETHASH}\"\n}}\n",
            self.name
        );
        let milestones = format!(
            "[\n    {{\n        \"height\": 1,\n        \"epoch\": \"{epoch}\",\n        \"blockTime\": 8\n    }}\n]\n"
        );
        fs::write(dir.join("network.json"), network).unwrap();
        fs::write(dir.join("milestones.json"), milestones).unwrap();
        fs::write(
            dir.join("genesisBlock.json"),
            "{\n    \"id\": \"genesis\"\n}\n",
        )
        .unwrap();
        fs::write(dir.join("exceptions.json"), "{}\n").unwrap();
        let delegate = |i: usize| GeneratedDelegate {
            username: format!("genesis_{i}"),
            public_key: format!("02{i:064x}"),
            secret_key: Zeroizing::new(format!("{i:064x}")),
        };
        Ok(Generated {
            out: options.out.clone(),
            nethash: NETHASH.to_owned(),
            genesis_id: "genesis".to_owned(),
            epoch,
            delegates: (1..=53).map(delegate).collect(),
            forger_aux: [7; 32],
            files: Vec::new(),
        })
    }
}

/// Process operations that record their calls, with the statuses they give.
#[derive(Default)]
struct Ops {
    calls: RefCell<Vec<String>>,
    statuses: RefCell<BTreeMap<String, SupervisedStatus>>,
    envs: RefCell<BTreeMap<String, Env>>,
}

impl Ops {
    fn set(&self, name: &str, status: SupervisedStatus) {
        self.statuses.borrow_mut().insert(name.to_owned(), status);
    }

    fn calls(&self) -> Vec<String> {
        self.calls.take()
    }

    fn running(&self, name: &str) -> bool {
        self.statuses
            .borrow()
            .get(name)
            .is_some_and(|status| status.is_running())
    }
}

impl ProcessOps for Ops {
    fn start(&self, instance: &Instance, env: &Env) -> Result<(), ProcessError> {
        let name = instance.name();
        self.calls.borrow_mut().push(format!("start {name}"));
        if self.running(&name) {
            return Err(ProcessError::AlreadyRunning { name });
        }
        self.envs.borrow_mut().insert(name.clone(), env.clone());
        self.set(&name, SupervisedStatus::Online);
        Ok(())
    }

    fn stop(&self, instance: &Instance) -> Result<(), ProcessError> {
        let name = instance.name();
        self.calls.borrow_mut().push(format!("stop {name}"));
        if !self.running(&name) {
            return Err(ProcessError::NotRunning { name });
        }
        self.set(&name, SupervisedStatus::Stopped);
        Ok(())
    }

    fn kill(&self, instance: &Instance) -> Result<(), ProcessError> {
        let name = instance.name();
        self.calls.borrow_mut().push(format!("kill {name}"));
        self.set(&name, SupervisedStatus::Stopped);
        Ok(())
    }

    fn status(&self, instance: &Instance) -> Result<StatusRow, ProcessError> {
        let name = instance.name();
        let status = self.statuses.borrow().get(&name).copied();
        Ok(StatusRow {
            pid: None,
            name,
            version: "0.1.0".to_owned(),
            status: status.unwrap_or(SupervisedStatus::Stopped),
            uptime: None,
            restarts: 0,
            cpu_percent: None,
            rss_bytes: None,
        })
    }
}

/// Relay APIs scripted per address: each answering relay's chain of block ids, from height 1.
#[derive(Default)]
struct Api {
    chains: RefCell<BTreeMap<SocketAddr, Vec<String>>>,
    /// The polls of `node_status` that fail before the APIs answer.
    silent: Cell<u32>,
}

impl Api {
    fn chain(&self, k: u8, ids: &[&str]) {
        let ids = ids.iter().map(|id| (*id).to_owned()).collect();
        self.chains.borrow_mut().insert(api(k), ids);
    }

    fn every(&self, nodes: u8, ids: &[&str]) {
        (1..=nodes).for_each(|k| self.chain(k, ids));
    }

    fn down(&self, address: SocketAddr) -> HttpError {
        let message = "Connection refused".to_owned();
        HttpError::Connection {
            address,
            path: "/api".to_owned(),
            message,
        }
    }
}

impl NodeApi for Api {
    fn node_status(&self, address: SocketAddr) -> Result<ApiNodeStatus, HttpError> {
        if self.silent.get() > 0 {
            self.silent.set(self.silent.get() - 1);
            return Err(self.down(address));
        }
        let chains = self.chains.borrow();
        let chain = chains.get(&address).ok_or_else(|| self.down(address))?;
        let now = u32::try_from(chain.len()).unwrap();
        Ok(ApiNodeStatus {
            synced: true,
            now,
            blocks_count: 0,
        })
    }

    fn last_block(&self, address: SocketAddr) -> Result<ApiBlock, HttpError> {
        let chains = self.chains.borrow();
        let chain = chains.get(&address).ok_or_else(|| self.down(address))?;
        let height = u32::try_from(chain.len()).unwrap();
        Ok(ApiBlock {
            id: chain.last().unwrap().clone(),
            height,
        })
    }

    fn block_at(&self, address: SocketAddr, height: u32) -> Result<Option<ApiBlock>, HttpError> {
        let chains = self.chains.borrow();
        let chain = chains.get(&address).ok_or_else(|| self.down(address))?;
        let id = chain.get(usize::try_from(height).unwrap() - 1).cloned();
        Ok(id.map(|id| ApiBlock { id, height }))
    }

    fn peers(&self, address: SocketAddr) -> Result<Vec<ApiPeer>, HttpError> {
        let chains = self.chains.borrow();
        let others = chains.keys().filter(|other| **other != address);
        let peer = |a: &SocketAddr| ApiPeer {
            ip: a.ip().to_string(),
            port: 6002,
            height: None,
        };
        Ok(others.map(peer).collect())
    }
}

/// A devnet under test: its directory and the fakes it acts through.
struct Bench {
    root: PathBuf,
    ops: Ops,
    api: Api,
    clock: ManualClock,
    pauses: RefCell<Vec<Duration>>,
    _serial: MutexGuard<'static, ()>,
}

impl Bench {
    fn new(name: &str) -> Bench {
        let serial = serial();
        Bench {
            root: scratch(name),
            ops: Ops::default(),
            api: Api::default(),
            clock: ManualClock::new(T0),
            pauses: RefCell::new(Vec::new()),
            _serial: serial,
        }
    }

    fn with<T>(&self, run: impl FnOnce(&Tools<'_>) -> T) -> T {
        let pause = |time: Duration| self.pauses.borrow_mut().push(time);
        run(&Tools {
            ops: &self.ops,
            api: &self.api,
            clock: &self.clock,
            pause: &pause,
        })
    }

    fn init(&self, options: &InitOptions) -> Result<Manifest, DevnetError> {
        let chain = FakeChain { name: "devnet" };
        self.with(|tools| devnet::init(&self.root, options, &chain, tools))
    }

    fn file(&self, path: &str) -> String {
        fs::read_to_string(self.root.join(path)).unwrap()
    }

    fn all(&self) -> NodeSelection {
        NodeSelection::default()
    }
}

fn options(validators: u32) -> InitOptions {
    InitOptions {
        validators,
        address_prefix: PREFIX,
        ..InitOptions::default()
    }
}

fn names(manifest: &Manifest, k: u32) -> Vec<String> {
    manifest.node(k).unwrap().delegates.clone()
}

fn span(from: usize, to: usize) -> Vec<String> {
    (from..=to).map(|i| format!("genesis_{i}")).collect()
}

const NODE3_ENV: &str =
    "# A devnet node, written by `heartwood devnet init`. Keep comments on their own lines.
CORE_LOG_LEVEL=info
CORE_LOG_LEVEL_FILE=info

CORE_P2P_HOST=127.13.0.13
CORE_P2P_PORT=6002
CORE_P2P_MIN_NETWORK_REACH=1

CORE_API_HOST=127.13.0.13
CORE_API_PORT=6003

CORE_WEBHOOKS_HOST=127.13.0.13
CORE_WEBHOOKS_PORT=6004
";

const NODE3_PEERS: &str = r#"{
    "list": [
        {
            "ip": "127.13.0.11",
            "port": 6002
        },
        {
            "ip": "127.13.0.12",
            "port": 6002
        },
        {
            "ip": "127.13.0.14",
            "port": 6002
        },
        {
            "ip": "127.13.0.15",
            "port": 6002
        }
    ]
}
"#;

/// Node 3's `app.json` in a five-node devnet.
fn node3_app() -> String {
    let plugin = |name: &str| {
        format!(
            "            {{\n                \"package\": \"@solar-network/{name}\"\n            }}"
        )
    };
    let before: Vec<String> = ["logger", "state", "database", "transactions", "pool"]
        .map(plugin)
        .into();
    let after: Vec<String> = ["blockchain", "api", "webhooks"].map(plugin).into();
    let p2p = r#"            {
                "package": "@solar-network/p2p",
                "options": {
                    "remoteAccess": [
                        "127.13.0.13"
                    ],
                    "devnet": {
                        "addressing": "alias",
                        "address": "127.13.0.13"
                    }
                }
            }"#;
    let forger = format!(
        r#"            {{
                "package": "@solar-network/forger",
                "options": {{
                    "hosts": [
                        {{
                            "hostname": "127.13.0.13",
                            "port": 6002
                        }}
                    ],
                    "localAddress": "127.13.0.13",
                    "aux": "{}"
                }}
            }}"#,
        "07".repeat(32)
    );
    let relay = [before, vec![p2p.to_owned()], after].concat().join(",\n");
    let forger = [plugin("logger"), plugin("transactions"), forger].join(",\n");
    format!(
        "{{\n    \"core\": {{\n        \"plugins\": []\n    }},\n    \"relay\": {{\n        \"plugins\": [\n{relay}\n        ]\n    }},\n    \"forger\": {{\n        \"plugins\": [\n{forger}\n        ]\n    }}\n}}\n"
    )
}

fn keys_file(from: usize, to: usize) -> String {
    let keys: Vec<String> = (from..=to)
        .map(|i| format!("        \"{i:064x}\""))
        .collect();
    format!("{{\n    \"keys\": [\n{}\n    ]\n}}\n", keys.join(",\n"))
}

#[test]
fn init_writes_five_validators_with_the_split_the_files_and_the_manifest() {
    let bench = Bench::new("five");
    let manifest = bench.init(&options(5)).unwrap();
    let sizes: Vec<usize> = manifest
        .nodes
        .iter()
        .map(|node| node.delegates.len())
        .collect();
    assert_eq!(sizes, [11, 11, 11, 10, 10]);
    assert_eq!(names(&manifest, 1), span(1, 11));
    assert_eq!(names(&manifest, 3), span(23, 33));
    assert_eq!(names(&manifest, 5), span(44, 53));
    let node3 = manifest.node(3).unwrap();
    assert_eq!(
        (node3.address, node3.forger_address),
        (ip(13), Some(ip(13)))
    );
    assert_eq!(node3.dir, bench.root.join("node3"));
    assert_eq!(bench.file("node3/config/.env"), NODE3_ENV);
    assert_eq!(bench.file("node3/config/app.json"), node3_app());
    assert_eq!(bench.file("node3/config/peers.json"), NODE3_PEERS);
    let config = "{\n    \"token\": \"node3\",\n    \"network\": \"devnet\"\n}\n";
    assert_eq!(bench.file("node3/config/config.json"), config);
    assert_eq!(bench.file("node3/config/delegates.json"), keys_file(23, 33));
    let mode = fs::metadata(bench.root.join("node3/config/delegates.json")).unwrap();
    assert_eq!(mode.permissions().mode() & 0o777, 0o600);
    for dir in ["data", "cache", "log", "temp"] {
        assert!(bench.root.join("node5").join(dir).is_dir());
    }
    // The crypto files are copies, and the manifest holds their hashes.
    for (name, hash) in &manifest.crypto_hashes {
        let genesis = fs::read(bench.root.join("genesis/crypto").join(name)).unwrap();
        assert_eq!(
            fs::read(bench.root.join("node4/config/crypto").join(name)).unwrap(),
            genesis
        );
        assert_eq!(*hash, hex::encode(&sha256(&genesis)));
    }
    let names: Vec<&str> = manifest
        .crypto_hashes
        .iter()
        .map(|(n, _)| n.as_str())
        .collect();
    assert_eq!(names, devnet::CRYPTO_FILES);
    assert_eq!(manifest.epoch, epoch(T0 + 30_000));
    assert_eq!(
        (manifest.nethash.as_str(), manifest.genesis_id.as_str()),
        (NETHASH, "genesis")
    );
    assert_eq!(Manifest::read(&bench.root).unwrap(), manifest);
    let text = bench.file("devnet.json");
    assert!(text.contains("\"addressPrefix\": \"127.13.0\""), "{text}");
    assert!(!bench.root.join("devnet.json.tmp").exists());
}

#[test]
fn init_gives_one_node_every_delegate_and_its_own_address_as_its_peer() {
    let bench = Bench::new("one");
    let manifest = bench.init(&options(1)).unwrap();
    assert_eq!(names(&manifest, 1), span(1, 53));
    assert_eq!(bench.file("node1/config/delegates.json"), keys_file(1, 53));
    let peers = "{\n    \"list\": [\n        {\n            \"ip\": \"127.13.0.11\",\n            \"port\": 6002\n        }\n    ]\n}\n";
    assert_eq!(bench.file("node1/config/peers.json"), peers);
}

#[test]
fn init_lays_out_relay_only_nodes_and_a_failover_forger() {
    let bench = Bench::new("failover");
    let failover = vec!["1:1,6,7".parse::<Failover>().unwrap()];
    let manifest = bench
        .init(&InitOptions {
            relays: 2,
            failover,
            ..options(5)
        })
        .unwrap();
    assert_eq!(manifest.nodes.len(), 7);
    let node1 = manifest.node(1).unwrap();
    assert_eq!(node1.forger_address, Some(ip(111)));
    let hosts: Vec<SocketAddr> = [11, 16, 17].map(|k| SocketAddr::new(ip(k), 6002)).into();
    assert_eq!(node1.forger_hosts, hosts);
    for k in [6, 7] {
        let node = manifest.node(k).unwrap();
        assert_eq!(
            (node.forger_address, node.roles(None)),
            (None, vec![Role::Relay])
        );
        let keys = bench.file(&format!("node{k}/config/delegates.json"));
        assert_eq!(keys, "{\n    \"keys\": []\n}\n");
    }
    let app = |k: u32| -> serde_json::Value {
        serde_json::from_str(&bench.file(&format!("node{k}/config/app.json"))).unwrap()
    };
    let access = |k: u32| app(k)["relay"]["plugins"][5]["options"]["remoteAccess"].clone();
    assert_eq!(
        access(1),
        serde_json::json!(["127.13.0.11", "127.13.0.111"])
    );
    assert_eq!(
        access(6),
        serde_json::json!(["127.13.0.16", "127.13.0.111"])
    );
    assert_eq!(
        access(7),
        serde_json::json!(["127.13.0.17", "127.13.0.111"])
    );
    assert_eq!(access(2), serde_json::json!(["127.13.0.12"]));
    let forger = app(1)["forger"]["plugins"][2]["options"].clone();
    assert_eq!(forger["localAddress"], "127.13.0.111");
    let hosts: Vec<&str> = forger["hosts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| h["hostname"].as_str().unwrap())
        .collect();
    assert_eq!(hosts, ["127.13.0.11", "127.13.0.16", "127.13.0.17"]);
    assert_eq!(Manifest::read(&bench.root).unwrap(), manifest);
    assert!(bench.file("devnet.json").contains("\"1:1,6,7\""));
}

#[test]
fn init_refuses_an_existing_devnet_unless_forced_and_then_stops_it_first() {
    let bench = Bench::new("force");
    bench.init(&options(2)).unwrap();
    let again = bench.init(&options(2));
    assert!(
        matches!(again, Err(DevnetError::Exists { .. })),
        "{again:?}"
    );
    bench
        .ops
        .set("node2-devnet-relay", SupervisedStatus::Online);
    bench
        .ops
        .set("node1-devnet-forger", SupervisedStatus::Online);
    let manifest = bench
        .init(&InitOptions {
            force: true,
            ..options(1)
        })
        .unwrap();
    assert_eq!(
        bench.ops.calls(),
        ["stop node1-devnet-forger", "stop node2-devnet-relay"]
    );
    assert_eq!(manifest.nodes.len(), 1);
    assert!(!bench.root.join("node2").exists());
}

#[test]
fn init_uses_a_directory_that_holds_other_files_but_no_devnet() {
    let bench = Bench::new("other-files");
    fs::write(bench.root.join("notes.txt"), "keep").unwrap();
    bench.init(&options(1)).unwrap();
    assert_eq!(bench.file("notes.txt"), "keep");
}

#[test]
fn init_guards_its_addresses_its_network_and_its_counts() {
    let bench = Bench::new("guards");
    let refused = |options: &InitOptions| bench.init(options).unwrap_err();
    let lan = InitOptions {
        address_prefix: [10, 0, 0],
        ..options(5)
    };
    assert!(matches!(refused(&lan), DevnetError::Guard(text) if text.contains("10.0.0")));
    let loud = InitOptions {
        log_level: "loud\nCORE_P2P_HOST=0.0.0.0".to_owned(),
        ..options(5)
    };
    assert!(matches!(refused(&loud), DevnetError::Guard(_)));
    for (validators, relays) in [(0, 0), (54, 0), (53, 48)] {
        let counts = InitOptions {
            relays,
            ..options(validators)
        };
        assert!(
            matches!(refused(&counts), DevnetError::Counts(_)),
            "{validators} {relays}"
        );
    }
    for text in ["6:6", "1:1,9", "1:1,1", "1:"] {
        let failover = vec![text.parse().unwrap_or(Failover {
            node: 1,
            hosts: Vec::new(),
        })];
        let layout = InitOptions {
            relays: 2,
            failover,
            ..options(5)
        };
        assert!(matches!(refused(&layout), DevnetError::Counts(_)), "{text}");
    }
    let twice = vec![
        Failover {
            node: 1,
            hosts: vec![1],
        },
        Failover {
            node: 1,
            hosts: vec![2],
        },
    ];
    assert!(matches!(
        refused(&InitOptions {
            failover: twice,
            ..options(5)
        }),
        DevnetError::Counts(_)
    ));
    // A listener address another process holds.
    let held = TcpListener::bind(SocketAddr::new(ip(12), 6002)).unwrap();
    let taken = refused(&options(5));
    assert!(
        matches!(taken, DevnetError::Address { address, .. } if address == held.local_addr().unwrap())
    );
    drop(held);
    // A chain whose network is not named devnet: refused, and nothing is left behind.
    let chain = FakeChain { name: "testnet" };
    let named = bench.with(|tools| devnet::init(&bench.root, &options(5), &chain, tools));
    assert!(matches!(named, Err(DevnetError::Guard(text)) if text.contains("testnet")));
    assert_eq!(fs::read_dir(&bench.root).unwrap().count(), 0);
    assert!(bench.ops.calls().is_empty());
}

#[test]
fn start_re_stamps_a_near_epoch_and_removes_the_signing_records() {
    let bench = Bench::new("restamp");
    let before = bench.init(&options(5)).unwrap();
    bench.api.every(5, &["genesis"]);
    let record = bench.root.join("node2/data/forger/signing-record.json");
    fs::create_dir_all(record.parent().unwrap()).unwrap();
    fs::write(&record, "{}").unwrap();
    let network = bench.file("genesis/crypto/network.json");
    // 25 s before the epoch: kept.
    bench.clock.set(T0 + 5_000);
    bench
        .with(|tools| devnet::start(&bench.root, &bench.all(), tools))
        .unwrap();
    assert_eq!(Manifest::read(&bench.root).unwrap(), before);
    assert!(record.exists());
    bench
        .with(|tools| devnet::stop(&bench.root, &bench.all(), false, tools))
        .unwrap();
    // 5 s before the epoch, nothing running and no store: re-stamped 30 s after now.
    bench.clock.set(T0 + 25_000);
    bench
        .with(|tools| devnet::start(&bench.root, &bench.all(), tools))
        .unwrap();
    let after = Manifest::read(&bench.root).unwrap();
    assert_eq!(after.epoch, epoch(T0 + 55_000));
    assert_eq!(
        (after.nethash.as_str(), after.genesis_id.as_str()),
        (NETHASH, "genesis")
    );
    assert!(!record.parent().unwrap().exists());
    let milestones = bench.file("genesis/crypto/milestones.json");
    assert!(milestones.contains(&epoch(T0 + 55_000)));
    assert_eq!(
        milestones,
        bench.file("node5/config/crypto/milestones.json")
    );
    assert_eq!(
        after.crypto_hashes[1].1,
        hex::encode(&sha256(milestones.as_bytes()))
    );
    assert_eq!(bench.file("genesis/crypto/network.json"), network);
    assert_eq!(after.crypto_hashes[0], before.crypto_hashes[0]);
    bench
        .with(|tools| devnet::stop(&bench.root, &bench.all(), false, tools))
        .unwrap();
    // Past the epoch, but a node has a chain store: kept.
    bench.clock.set(T0 + 600_000);
    let store = bench.root.join("node4/data/database/blockchain.db");
    fs::create_dir_all(store.parent().unwrap()).unwrap();
    fs::write(&store, "").unwrap();
    bench
        .with(|tools| devnet::start(&bench.root, &bench.all(), tools))
        .unwrap();
    assert_eq!(Manifest::read(&bench.root).unwrap(), after);
    // Past the epoch, no store, but a node runs: kept.
    fs::remove_file(&store).unwrap();
    bench
        .with(|tools| devnet::stop_node(&bench.root, 3, None, false, tools))
        .unwrap();
    bench
        .with(|tools| devnet::start_node(&bench.root, 3, None, tools))
        .unwrap();
    assert_eq!(Manifest::read(&bench.root).unwrap(), after);
}

#[test]
fn start_starts_the_relays_then_the_forgers_once_the_apis_answer() {
    let bench = Bench::new("start");
    bench
        .init(&InitOptions {
            relays: 1,
            ..options(3)
        })
        .unwrap();
    bench.api.every(4, &["genesis"]);
    bench.api.silent.set(5);
    bench
        .with(|tools| devnet::start(&bench.root, &bench.all(), tools))
        .unwrap();
    let relays = (1..=4).map(|k| format!("start node{k}-devnet-relay"));
    let forgers = (1..=3).map(|k| format!("start node{k}-devnet-forger"));
    assert_eq!(bench.ops.calls(), relays.chain(forgers).collect::<Vec<_>>());
    assert_eq!(*bench.pauses.borrow(), [Duration::from_secs(1); 2]);
    let envs = bench.ops.envs.borrow();
    let env = &envs["node2-devnet-forger"];
    let value = |key: &str| {
        env.iter()
            .rev()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.clone())
            .unwrap()
    };
    assert_eq!(value("CORE_P2P_HOST"), "127.13.0.12");
    assert_eq!(value("CORE_P2P_PORT"), "6002");
    assert_eq!(
        PathBuf::from(value("CORE_PATH_CONFIG")),
        bench.root.join("node2/config")
    );
    assert_eq!(
        PathBuf::from(value("CORE_PATH_TEMP")),
        bench.root.join("node2/temp")
    );
    drop(envs);
    // A selected role that runs refuses the start, before anything starts.
    let running = bench.with(|tools| devnet::start(&bench.root, &bench.all(), tools));
    assert!(matches!(
        running,
        Err(DevnetError::Process(ProcessError::AlreadyRunning { .. }))
    ));
    assert!(bench.ops.calls().is_empty());
}

#[test]
fn start_gives_up_on_relays_whose_api_never_answers_and_starts_no_forger() {
    let bench = Bench::new("silent");
    bench.init(&options(2)).unwrap();
    bench.api.chain(1, &["genesis"]);
    let timeout = bench.with(|tools| devnet::start(&bench.root, &bench.all(), tools));
    assert!(
        matches!(&timeout, Err(DevnetError::Timeout { what, seconds: 30 }) if what.contains("node 2")),
        "{timeout:?}"
    );
    assert_eq!(bench.pauses.borrow().len(), 30);
    assert_eq!(
        bench.ops.calls(),
        ["start node1-devnet-relay", "start node2-devnet-relay"]
    );
}

#[test]
fn start_refuses_changed_crypto_files_open_listeners_and_taken_addresses() {
    let bench = Bench::new("preflight");
    bench.init(&options(2)).unwrap();
    bench.api.every(2, &["genesis"]);
    let start = || bench.with(|tools| devnet::start(&bench.root, &bench.all(), tools));
    let milestones = bench.root.join("node2/config/crypto/milestones.json");
    let text = bench.file("node2/config/crypto/milestones.json");
    fs::write(
        &milestones,
        text.replace("\"blockTime\": 8", "\"blockTime\": 4"),
    )
    .unwrap();
    assert!(
        matches!(start(), Err(DevnetError::Manifest { reason, .. }) if reason.contains("node 2's crypto/milestones.json"))
    );
    fs::write(&milestones, text).unwrap();
    let env = bench.root.join("node1/config/.env");
    let text = bench.file("node1/config/.env");
    fs::write(
        &env,
        text.replace("CORE_API_HOST=127.13.0.11", "CORE_API_HOST=0.0.0.0"),
    )
    .unwrap();
    assert!(matches!(start(), Err(DevnetError::Guard(text)) if text.contains("CORE_API_HOST")));
    fs::write(&env, text).unwrap();
    let held = TcpListener::bind(SocketAddr::new(ip(12), 6003)).unwrap();
    assert!(matches!(start(), Err(DevnetError::Address { .. })));
    drop(held);
    assert!(bench.ops.calls().is_empty());
    start().unwrap();
}

#[test]
fn stop_stops_the_forgers_before_the_relays_and_skips_what_does_not_run() {
    let bench = Bench::new("stop");
    bench.init(&options(3)).unwrap();
    bench.api.every(3, &["genesis"]);
    bench
        .with(|tools| devnet::start(&bench.root, &bench.all(), tools))
        .unwrap();
    bench
        .ops
        .set("node2-devnet-forger", SupervisedStatus::Errored);
    bench.ops.calls();
    bench
        .with(|tools| devnet::stop(&bench.root, &bench.all(), false, tools))
        .unwrap();
    let expected = ["stop node1-devnet-forger", "stop node3-devnet-forger"]
        .into_iter()
        .map(str::to_owned)
        .chain((1..=3).map(|k| format!("stop node{k}-devnet-relay")));
    assert_eq!(bench.ops.calls(), expected.collect::<Vec<_>>());
    bench
        .with(|tools| devnet::start(&bench.root, &bench.all(), tools))
        .unwrap();
    bench.ops.calls();
    let relays = NodeSelection {
        nodes: vec![3, 1],
        role: Some(Role::Relay),
    };
    bench
        .with(|tools| devnet::stop(&bench.root, &relays, true, tools))
        .unwrap();
    assert_eq!(
        bench.ops.calls(),
        ["kill node1-devnet-relay", "kill node3-devnet-relay"]
    );
    let unknown = NodeSelection {
        nodes: vec![4],
        role: None,
    };
    let refused = bench.with(|tools| devnet::stop(&bench.root, &unknown, false, tools));
    assert!(matches!(refused, Err(DevnetError::NoNode(4))));
}

#[test]
fn the_node_verbs_act_on_one_node_in_the_roles_order() {
    let bench = Bench::new("verbs");
    bench.init(&options(3)).unwrap();
    bench.api.every(3, &["genesis"]);
    bench
        .with(|tools| devnet::start(&bench.root, &bench.all(), tools))
        .unwrap();
    bench.ops.calls();
    let root = &bench.root;
    bench
        .with(|tools| devnet::stop_node(root, 2, Some(Role::Forger), false, tools))
        .unwrap();
    assert_eq!(bench.ops.calls(), ["stop node2-devnet-forger"]);
    bench
        .with(|tools| devnet::stop_node(root, 2, None, true, tools))
        .unwrap();
    assert_eq!(bench.ops.calls(), ["kill node2-devnet-relay"]);
    bench
        .with(|tools| devnet::start_node(root, 2, None, tools))
        .unwrap();
    assert_eq!(
        bench.ops.calls(),
        ["start node2-devnet-relay", "start node2-devnet-forger"]
    );
    bench
        .with(|tools| devnet::restart_node(root, 3, None, tools))
        .unwrap();
    let restart = ["stop node3-devnet-forger", "stop node3-devnet-relay"];
    let restart = restart
        .into_iter()
        .chain(["start node3-devnet-relay", "start node3-devnet-forger"]);
    assert_eq!(bench.ops.calls(), restart.collect::<Vec<_>>());
    bench
        .with(|tools| devnet::restart_node(root, 1, Some(Role::Relay), tools))
        .unwrap();
    assert_eq!(
        bench.ops.calls(),
        ["stop node1-devnet-relay", "start node1-devnet-relay"]
    );
    // A role that does not run is not restarted, and nothing stops.
    bench
        .ops
        .set("node1-devnet-forger", SupervisedStatus::Stopped);
    let stopped = bench.with(|tools| devnet::restart_node(root, 1, None, tools));
    assert!(matches!(
        stopped,
        Err(DevnetError::Process(ProcessError::NotRunning { .. }))
    ));
    assert!(bench.ops.calls().is_empty());
    let unknown = bench.with(|tools| devnet::restart_node(root, 9, None, tools));
    assert!(matches!(unknown, Err(DevnetError::NoNode(9))));
}

#[test]
fn reset_stops_a_node_and_deletes_its_state_but_keeps_its_config_and_signing_record() {
    let bench = Bench::new("reset");
    bench.init(&options(2)).unwrap();
    bench.api.every(2, &["genesis"]);
    bench
        .with(|tools| devnet::start(&bench.root, &bench.all(), tools))
        .unwrap();
    bench.ops.calls();
    let node = bench.root.join("node2");
    for file in [
        "data/database/blockchain.db",
        "data/forger/signing-record.json",
        "log/x.log",
        "temp/clear-pool.lock",
        "cache/c",
    ] {
        fs::create_dir_all(node.join(file).parent().unwrap()).unwrap();
        fs::write(node.join(file), "x").unwrap();
    }
    bench
        .with(|tools| devnet::reset(&bench.root, 2, tools))
        .unwrap();
    assert_eq!(
        bench.ops.calls(),
        ["stop node2-devnet-forger", "stop node2-devnet-relay"]
    );
    assert!(!node.join("data/database").exists());
    assert!(node.join("data/forger/signing-record.json").exists());
    for dir in ["log", "temp", "cache"] {
        assert_eq!(fs::read_dir(node.join(dir)).unwrap().count(), 0, "{dir}");
    }
    assert!(node.join("config/delegates.json").exists());
    assert!(bench.root.join("node1/data").exists());
}

#[test]
fn wait_polls_every_two_seconds_until_the_answering_relays_reach_the_height() {
    let bench = Bench::new("wait");
    bench.init(&options(3)).unwrap();
    let root = &bench.root;
    let wait = |height, node, seconds| {
        bench.with(|tools| devnet::wait(root, height, node, Duration::from_secs(seconds), tools))
    };
    // No relay answers: a timeout after 120 s of polls.
    let silent = wait(2, None, 120);
    assert!(matches!(
        silent,
        Err(DevnetError::Timeout { seconds: 120, .. })
    ));
    assert_eq!(*bench.pauses.borrow(), vec![Duration::from_secs(2); 60]);
    bench.pauses.take();
    bench.api.chain(1, &["genesis", "b2", "b3"]);
    bench.api.chain(2, &["genesis", "b2"]);
    assert!(matches!(wait(3, None, 4), Err(DevnetError::Timeout { .. })));
    assert_eq!(bench.pauses.take().len(), 2);
    assert_eq!(wait(2, None, 4).unwrap(), 2);
    assert_eq!(wait(3, Some(1), 4).unwrap(), 3);
    assert!(bench.pauses.borrow().is_empty());
    assert!(matches!(wait(1, Some(7), 4), Err(DevnetError::NoNode(7))));
}

#[test]
fn status_compares_the_relays_at_their_lowest_common_height_and_counts_online_delegates() {
    let bench = Bench::new("status");
    bench
        .init(&InitOptions {
            relays: 1,
            ..options(5)
        })
        .unwrap();
    let status = |bench: &Bench| {
        bench
            .with(|tools| devnet::status(&bench.root, tools))
            .unwrap()
    };
    let quiet = status(&bench);
    assert_eq!(
        (quiet.consistency, quiet.online_delegates),
        (Consistency::Unknown, 0)
    );
    assert!(!quiet.has_quorum());
    assert_eq!(quiet.nodes.len(), 6);
    assert_eq!(quiet.nodes[5].forger, None);
    for k in 1..=3 {
        bench
            .ops
            .set(&format!("node{k}-devnet-forger"), SupervisedStatus::Online);
    }
    bench
        .ops
        .set("node4-devnet-forger", SupervisedStatus::Launching);
    bench.api.chain(1, &["genesis", "b2", "b3", "b4"]);
    bench.api.chain(2, &["genesis", "b2", "b3"]);
    bench.api.chain(3, &["genesis", "b2", "b3", "b4", "b5"]);
    let consistent = status(&bench);
    assert_eq!(consistent.consistency, Consistency::Consistent(3));
    assert_eq!(
        (consistent.online_delegates, consistent.delegates()),
        (33, 53)
    );
    assert!(consistent.has_quorum());
    let view = consistent.nodes[0].api.clone().unwrap();
    assert_eq!(
        (view.status.now, view.last_block.id.as_str(), view.peers),
        (4, "b4", 2)
    );
    assert_eq!(consistent.nodes[3].api, None);
    assert_eq!(
        consistent.to_string(),
        "chain consistent at height 3; delegates with an online forger: 33 of 53 (27 needed)"
    );
    bench.api.chain(6, &["genesis", "b2", "x3", "x4"]);
    bench
        .ops
        .set("node3-devnet-forger", SupervisedStatus::Stopped);
    let diverged = status(&bench);
    assert_eq!(diverged.consistency, Consistency::Diverged(3));
    assert_eq!(diverged.online_delegates, 22);
    assert!(!diverged.has_quorum());
    assert!(
        diverged
            .to_string()
            .starts_with("chain DIVERGED at 3; delegates with an online forger: 22 of 53")
    );
}

#[test]
fn logs_merge_the_selected_processes_in_time_order_with_their_prefixes() {
    let bench = Bench::new("logs");
    bench.init(&options(2)).unwrap();
    let log = |name: &str, lines: &[&str]| {
        let path = bench.root.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            path,
            lines
                .iter()
                .map(|line| format!("{line}\n"))
                .collect::<String>(),
        )
        .unwrap();
    };
    log(
        "node1/log/node1-devnet-relay-out.log",
        &[
            "[2026-09-27 10:00:01.000] INFO: r1",
            "[2026-09-27 10:00:03.000] INFO: r3",
            "  a continuation",
        ],
    );
    log(
        "node1/log/node1-devnet-forger-out.log",
        &[
            "[2026-09-27 10:00:00.500] INFO: f0",
            "[2026-09-27 10:00:02.000] INFO: f2",
        ],
    );
    log(
        "node2/log/node2-devnet-relay-out.log",
        &["[2026-09-27 10:00:02.500] INFO: s2"],
    );
    log("node2/log/node2-devnet-relay-error.log", &["boom"]);
    let read = |selection: NodeSelection, stream, lines| {
        let mut out = Vec::new();
        devnet::logs(&bench.root, &selection, stream, lines, &mut out).unwrap();
        String::from_utf8(out).unwrap()
    };
    let merged = read(NodeSelection::default(), LogStream::Out, 15);
    assert_eq!(
        merged,
        "node1/forger: [2026-09-27 10:00:00.500] INFO: f0\n\
         node1/relay: [2026-09-27 10:00:01.000] INFO: r1\n\
         node1/forger: [2026-09-27 10:00:02.000] INFO: f2\n\
         node2/relay: [2026-09-27 10:00:02.500] INFO: s2\n\
         node1/relay: [2026-09-27 10:00:03.000] INFO: r3\n\
         node1/relay:   a continuation\n"
    );
    let last = read(
        NodeSelection {
            nodes: vec![1],
            role: Some(Role::Relay),
        },
        LogStream::Out,
        2,
    );
    assert_eq!(
        last,
        "[2026-09-27 10:00:03.000] INFO: r3\n  a continuation\n"
    );
    let errors = read(NodeSelection::default(), LogStream::Error, 15);
    assert_eq!(errors, "node2/relay: boom\n");
    assert_eq!(read(NodeSelection::default(), LogStream::File, 15), "");
}

#[test]
fn env_gives_a_nodes_dot_env_values_then_its_five_directories() {
    let bench = Bench::new("env");
    bench.init(&options(2)).unwrap();
    let env = devnet::env(&bench.root, 2).unwrap();
    let pairs: Vec<(String, String)> = env
        .iter()
        .map(|(k, v)| {
            (
                k.to_string_lossy().into_owned(),
                v.to_string_lossy().into_owned(),
            )
        })
        .collect();
    let keys: Vec<&str> = pairs.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(
        keys,
        [
            "CORE_LOG_LEVEL",
            "CORE_LOG_LEVEL_FILE",
            "CORE_P2P_HOST",
            "CORE_P2P_PORT",
            "CORE_P2P_MIN_NETWORK_REACH",
            "CORE_API_HOST",
            "CORE_API_PORT",
            "CORE_WEBHOOKS_HOST",
            "CORE_WEBHOOKS_PORT",
            "CORE_PATH_CONFIG",
            "CORE_PATH_DATA",
            "CORE_PATH_CACHE",
            "CORE_PATH_LOG",
            "CORE_PATH_TEMP",
        ]
    );
    assert_eq!(pairs[3].1, "6002");
    assert_eq!(pairs[2].1, "127.13.0.12");
    assert_eq!(
        pairs[11].1,
        bench.root.join("node2/cache").to_string_lossy()
    );
    assert!(matches!(
        devnet::env(&bench.root, 3),
        Err(DevnetError::NoNode(3))
    ));
    let missing = devnet::env(&bench.root.join("nowhere"), 1);
    assert!(matches!(missing, Err(DevnetError::Manifest { .. })));
}

#[test]
#[ignore = "integration"]
fn init_writes_a_devnet_over_the_real_network_generator() {
    let bench = Bench::new("real-generator");
    let manifest = bench
        .with(|tools| devnet::init(&bench.root, &options(5), &NetworkGenerator, tools))
        .unwrap();
    let sizes: Vec<usize> = manifest
        .nodes
        .iter()
        .map(|node| node.delegates.len())
        .collect();
    assert_eq!(sizes, [11, 11, 11, 10, 10]);
    assert_eq!(
        names(&manifest, 1).first().map(String::as_str),
        Some("genesis_1")
    );
    let network: serde_json::Value =
        serde_json::from_str(&bench.file("genesis/crypto/network.json")).unwrap();
    assert_eq!(network["name"], "devnet");
    assert_eq!(network["nethash"], manifest.nethash.as_str());
    let keys: serde_json::Value =
        serde_json::from_str(&bench.file("node1/config/delegates.json")).unwrap();
    assert_eq!(keys["keys"].as_array().unwrap().len(), 11);
    // The forger's aux is the seed's, as the devnet tooling derives it.
    let aux = heartwood_crypto::genesis::derive::aux("solar-devnet-v1/", "forger", "solar-devnet");
    assert!(
        bench
            .file("node1/config/app.json")
            .contains(&hex::encode(&aux))
    );
    assert_eq!(Manifest::read(&bench.root).unwrap(), manifest);
}
