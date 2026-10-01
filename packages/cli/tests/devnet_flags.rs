//! The devnet library's flags against fakes: a core devnet (`init --core`, and `start` through
//! the `core` role), `status --json`, `status --resources`, `logs --follow`, and the `start` flags
//! `--keep-epoch` and `--no-wait`. The devnets use the loopback block `HEARTWOOD_TEST_LOOPBACK`
//! names, else 127.13.0.0/24; the tests that bind its addresses run one at a time.

#![cfg(feature = "devnet")]

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use heartwood_cli::devnet::{
    self, Consistency, Failover, InitOptions, LogStream, Manifest, NodeSelection, Resources,
    StartOptions, Tools,
};
use heartwood_cli::errors::{DevnetError, HttpError, NetworkError, ProcessError};
use heartwood_cli::seams::{
    ApiBlock, ApiNodeStatus, ApiPeer, ChainGenerator, Env, GenerateOptions, Generated,
    GeneratedDelegate, Instance, NodeApi, ProcessOps, Role, StatusRow, SupervisedStatus,
};
use heartwood_kernel::{Clock, ManualClock};
use serde_json::{Value, json};
use zeroize::Zeroizing;

/// The clock at `init`: 2026-09-21T10:13:20.000Z.
const T0: i64 = 1_790_158_400_000;

static SERIAL: Mutex<()> = Mutex::new(());

/// The three octets of the tests' loopback block.
fn prefix() -> [u8; 3] {
    let block = std::env::var("HEARTWOOD_TEST_LOOPBACK").unwrap_or_default();
    let octets: Vec<u8> = block
        .split('.')
        .filter_map(|part| part.parse().ok())
        .collect();
    <[u8; 3]>::try_from(octets).unwrap_or([127, 13, 0])
}

fn ip(k: u8) -> IpAddr {
    let [a, b, c] = prefix();
    IpAddr::from([a, b, c, k])
}

fn api(k: u8) -> SocketAddr {
    SocketAddr::new(ip(10 + k), 6003)
}

/// An empty directory for the test `name`.
fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("devnet-flags")
        .join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// A generator that writes four small crypto files and 53 delegates.
struct FakeChain;

impl ChainGenerator for FakeChain {
    fn generate(
        &self,
        options: &GenerateOptions,
        _: &dyn Clock,
    ) -> Result<Generated, NetworkError> {
        let dir = options.out.join("crypto");
        fs::create_dir_all(&dir).unwrap();
        let epoch = options.epoch.clone().unwrap();
        let network = "{\n    \"name\": \"devnet\",\n    \"nethash\": \"ab12\"\n}\n";
        let milestones = format!(
            "[\n    {{\n        \"height\": 1,\n        \"epoch\": \"{epoch}\"\n    }}\n]\n"
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
            nethash: "ab12".to_owned(),
            genesis_id: "genesis".to_owned(),
            epoch,
            delegates: (1..=53).map(delegate).collect(),
            forger_aux: [7; 32],
            files: Vec::new(),
        })
    }
}

/// Process operations that record their calls, with the statuses and pids they give.
#[derive(Default)]
struct Ops {
    calls: RefCell<Vec<String>>,
    statuses: RefCell<BTreeMap<String, SupervisedStatus>>,
    pids: RefCell<BTreeMap<String, u32>>,
}

impl Ops {
    fn set(&self, name: &str, status: SupervisedStatus) {
        self.statuses.borrow_mut().insert(name.to_owned(), status);
    }

    fn calls(&self) -> Vec<String> {
        self.calls.take()
    }

    fn running(&self, name: &str) -> bool {
        let statuses = self.statuses.borrow();
        statuses.get(name).is_some_and(|status| status.is_running())
    }
}

impl ProcessOps for Ops {
    fn start(&self, instance: &Instance, _: &Env) -> Result<(), ProcessError> {
        let name = instance.name();
        self.calls.borrow_mut().push(format!("start {name}"));
        if self.running(&name) {
            return Err(ProcessError::AlreadyRunning { name });
        }
        self.set(&name, SupervisedStatus::Online);
        Ok(())
    }

    fn stop(&self, instance: &Instance) -> Result<(), ProcessError> {
        let name = instance.name();
        self.calls.borrow_mut().push(format!("stop {name}"));
        self.set(&name, SupervisedStatus::Stopped);
        Ok(())
    }

    fn kill(&self, instance: &Instance) -> Result<(), ProcessError> {
        self.set(&instance.name(), SupervisedStatus::Stopped);
        Ok(())
    }

    fn status(&self, instance: &Instance) -> Result<StatusRow, ProcessError> {
        let name = instance.name();
        let status = self.statuses.borrow().get(&name).copied();
        let status = status.unwrap_or(SupervisedStatus::Stopped);
        let pid = self.pids.borrow().get(&name).copied();
        // A running process has run for 12.5 s.
        let uptime = status.is_running().then_some(Duration::from_millis(12_500));
        Ok(StatusRow {
            pid,
            name,
            version: "0.1.0".to_owned(),
            status,
            uptime,
            restarts: 0,
            cpu_percent: None,
            rss_bytes: None,
        })
    }
}

/// Relay APIs scripted per address: each answering relay's chain of block ids, from height 1. A
/// relay lists the others as its peers, each at the address and port `reports` gives for it, else
/// at its own address and port 6002.
#[derive(Default)]
struct Api {
    chains: RefCell<BTreeMap<SocketAddr, Vec<String>>>,
    reports: RefCell<BTreeMap<SocketAddr, (String, u16)>>,
    polls: Cell<u32>,
}

impl Api {
    fn every(&self, nodes: u8, ids: &[&str]) {
        let ids: Vec<String> = ids.iter().map(|id| (*id).to_owned()).collect();
        (1..=nodes).for_each(|k| drop(self.chains.borrow_mut().insert(api(k), ids.clone())));
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
        self.polls.set(self.polls.get() + 1);
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
        let reports = self.reports.borrow();
        let peer = |a: &SocketAddr| {
            let (ip, port) = reports
                .get(a)
                .cloned()
                .unwrap_or_else(|| (a.ip().to_string(), 6002));
            ApiPeer {
                ip,
                port,
                height: None,
            }
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
    home: PathBuf,
    _serial: MutexGuard<'static, ()>,
}

impl Bench {
    fn new(name: &str) -> Bench {
        let serial = SERIAL
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        Bench {
            root: scratch(name),
            ops: Ops::default(),
            api: Api::default(),
            clock: ManualClock::new(T0),
            pauses: RefCell::new(Vec::new()),
            home: scratch(&format!("{name}-home")),
            _serial: serial,
        }
    }

    fn with<T>(&self, run: impl FnOnce(&Tools<'_>) -> T) -> T {
        let pause = |time: Duration| self.pauses.borrow_mut().push(time);
        let (ops, api, clock) = (&self.ops, &self.api, &self.clock);
        run(&Tools {
            ops,
            api,
            clock,
            pause: &pause,
            process: &[],
            home: &self.home,
        })
    }

    fn init(&self, options: &InitOptions) -> Result<Manifest, DevnetError> {
        self.with(|tools| devnet::init(&self.root, options, &FakeChain, tools))
    }

    fn start(&self, options: &StartOptions) -> Result<(), DevnetError> {
        let all = NodeSelection::default();
        self.with(|tools| devnet::start_with(&self.root, &all, options, tools))
    }

    fn file(&self, path: &str) -> String {
        fs::read_to_string(self.root.join(path)).unwrap()
    }
}

fn options(validators: u32) -> InitOptions {
    InitOptions {
        validators,
        address_prefix: prefix(),
        ..InitOptions::default()
    }
}

fn core(validators: u32, relays: u32) -> InitOptions {
    InitOptions {
        core: true,
        relays,
        ..options(validators)
    }
}

fn packages(app: &str, profile: &str) -> Vec<String> {
    let app: Value = serde_json::from_str(app).unwrap();
    let plugins = app[profile]["plugins"].as_array().unwrap();
    plugins
        .iter()
        .map(|p| {
            p["package"]
                .as_str()
                .unwrap()
                .replace("@solar-network/", "")
        })
        .collect()
}

#[test]
fn init_core_writes_core_nodes_with_the_relay_and_forger_plugins_in_one_profile() {
    let bench = Bench::new("core-init");
    let manifest = bench.init(&core(2, 1)).unwrap();
    let flags: Vec<bool> = manifest.nodes.iter().map(|node| node.core).collect();
    assert_eq!((manifest.core, flags), (true, [true, true, false].to_vec()));
    assert_eq!(Manifest::read(&bench.root).unwrap(), manifest);
    let app = bench.file("node1/config/app.json");
    let (relay, joined) = (packages(&app, "relay"), packages(&app, "core"));
    assert_eq!(
        joined
            .split_last()
            .map(|(last, rest)| (last.as_str(), rest)),
        Some(("forger", relay.as_slice()))
    );
    assert!(packages(&bench.file("node3/config/app.json"), "core").is_empty());
    assert_eq!(manifest.nodes[0].roles(None), [Role::Core]);
    assert_eq!(manifest.nodes[2].roles(None), [Role::Relay]);
    // A split devnet keeps its manifest and roles.
    let split = bench
        .init(&InitOptions {
            force: true,
            ..options(2)
        })
        .unwrap();
    assert!(!split.core && !split.nodes[0].core);
    assert_eq!(split.nodes[0].roles(None), [Role::Relay, Role::Forger]);
    assert!(!bench.file("devnet.json").contains("\"core\": true"));
    let failover = InitOptions {
        failover: vec!["1:1,2".parse::<Failover>().unwrap()],
        ..core(2, 0)
    };
    assert!(matches!(bench.init(&failover), Err(DevnetError::Guard(_))));
}

#[test]
fn start_runs_one_core_process_per_validator_and_stop_ends_them() {
    let bench = Bench::new("core-start");
    bench.init(&core(2, 1)).unwrap();
    bench.api.every(3, &["genesis"]);
    bench.start(&StartOptions::default()).unwrap();
    let started = [
        "start node3-devnet-relay",
        "start node1-devnet-core",
        "start node2-devnet-core",
    ];
    assert_eq!(bench.ops.calls(), started);
    let again = bench.start(&StartOptions::default());
    assert!(matches!(
        again,
        Err(DevnetError::Process(ProcessError::AlreadyRunning { .. }))
    ));
    let stopped = |bench: &Bench| {
        let all = NodeSelection::default();
        bench
            .with(|tools| devnet::stop(&bench.root, &all, false, tools))
            .unwrap();
        bench.ops.calls()
    };
    let calls = stopped(&bench);
    let stops = [
        "stop node1-devnet-core",
        "stop node2-devnet-core",
        "stop node3-devnet-relay",
    ];
    assert_eq!(
        calls
            .iter()
            .filter(|call| call.starts_with("stop"))
            .collect::<Vec<_>>(),
        stops
    );
    // A role that a named core node lacks is refused before any process starts.
    let forgers = NodeSelection {
        nodes: vec![1, 2],
        role: Some(Role::Forger),
    };
    bench
        .with(|tools| devnet::start(&bench.root, &forgers, tools))
        .unwrap_err();
    assert!(bench.ops.calls().is_empty());
    // With no node named, a role selects itself on the nodes that have it.
    let relays = NodeSelection {
        nodes: Vec::new(),
        role: Some(Role::Relay),
    };
    bench
        .with(|tools| devnet::start(&bench.root, &relays, tools))
        .unwrap();
    assert_eq!(bench.ops.calls(), ["start node3-devnet-relay"]);
    bench
        .with(|tools| devnet::stop(&bench.root, &relays, false, tools))
        .unwrap();
    assert_eq!(bench.ops.calls(), ["stop node3-devnet-relay"]);
    let status = bench
        .with(|tools| devnet::status(&bench.root, tools))
        .unwrap();
    assert!(status.nodes[0].core && status.nodes[0].forger.is_none());
    assert_eq!(status.nodes[0].relay.name, "node1-devnet-core");
    bench.ops.set("node1-devnet-core", SupervisedStatus::Online);
    let status = bench
        .with(|tools| devnet::status(&bench.root, tools))
        .unwrap();
    assert_eq!(status.online_delegates, status.nodes[0].delegates);
    assert!(status.online_delegates > 0);
}

#[test]
fn a_core_nodes_listeners_are_read_from_its_core_profile() {
    let bench = Bench::new("core-guard");
    bench.init(&core(1, 0)).unwrap();
    bench.api.every(1, &["genesis"]);
    // Only the core profile, the one a core node loads, binds its P2P server beyond loopback.
    let path = bench.root.join("node1/config/app.json");
    let mut app: Value = serde_json::from_str(&bench.file("node1/config/app.json")).unwrap();
    let plugins = app["core"]["plugins"].as_array_mut().unwrap();
    let p2p = plugins
        .iter_mut()
        .find(|plugin| plugin["package"] == "@solar-network/p2p")
        .unwrap();
    p2p["options"]["server"] = json!({ "hostname": "0.0.0.0" });
    fs::write(&path, serde_json::to_string_pretty(&app).unwrap()).unwrap();
    let refused = bench.start(&StartOptions::default());
    assert!(
        matches!(&refused, Err(DevnetError::Guard(text)) if text.contains("0.0.0.0")),
        "{refused:?}"
    );
    assert!(bench.ops.calls().is_empty());
}

#[test]
fn status_json_gives_each_node_its_processes_api_and_the_address_its_peers_report() {
    let bench = Bench::new("status-json");
    bench
        .init(&InitOptions {
            relays: 1,
            ..options(2)
        })
        .unwrap();
    let json = |bench: &Bench| {
        bench
            .with(|tools| devnet::status_json(&bench.root, tools))
            .unwrap()
    };
    let quiet = json(&bench);
    assert_eq!(quiet["chain"]["consistency"], "unknown");
    assert_eq!(quiet["chain"]["height"], Value::Null);
    assert_eq!(quiet["nodes"][0]["api"], Value::Null);
    assert_eq!(quiet["nodes"][0]["reportedAddress"], Value::Null);
    bench
        .ops
        .set("node1-devnet-relay", SupervisedStatus::Online);
    bench
        .ops
        .set("node1-devnet-forger", SupervisedStatus::Online);
    bench
        .ops
        .pids
        .borrow_mut()
        .insert("node1-devnet-relay".to_owned(), 4242);
    bench.api.every(3, &["genesis", "b2"]);
    // Node 2's peers know it at another address than its own; node 3's at its own address but
    // another port, which is the address they report.
    let mut reports = bench.api.reports.borrow_mut();
    reports.insert(api(2), ("127.99.9.9".to_owned(), 6002));
    reports.insert(api(3), (ip(13).to_string(), 4001));
    drop(reports);
    let full = json(&bench);
    let (a, c) = (ip(11), ip(13));
    let node1 = &full["nodes"][0];
    assert_eq!(node1["node"], 1);
    assert_eq!(node1["address"], a.to_string());
    assert_eq!(node1["core"], false);
    assert_eq!(
        node1["processes"][0],
        json!({ "role": "relay", "status": "online", "pid": 4242, "uptime": 12.5, "restarts": 0 })
    );
    assert_eq!(node1["processes"][1]["role"], "forger");
    assert_eq!(full["nodes"][1]["processes"][0]["uptime"], Value::Null);
    assert_eq!(node1["apiUrl"], format!("http://{a}:6003"));
    assert_eq!(
        node1["api"],
        json!({ "height": 2, "lastBlockId": "b2", "synced": true, "peers": 2 })
    );
    assert_eq!(node1["reportedAddress"], format!("{a}:6002"));
    assert_eq!(full["nodes"][1]["reportedAddress"], Value::Null);
    assert_eq!(full["nodes"][2]["reportedAddress"], format!("{c}:4001"));
    assert_eq!(full["nodes"][2]["processes"].as_array().unwrap().len(), 1);
    assert_eq!(
        full["chain"],
        json!({ "consistency": "consistent", "height": 2, "compared": 3, "relays": 3 })
    );
    assert_eq!(full["delegates"]["total"], 53);
    assert_eq!(full["delegates"]["online"], 27);
    assert_eq!(full["delegates"]["quorum"], 27);
    assert_eq!(full["delegates"]["hasQuorum"], true);
    let status = bench
        .with(|tools| devnet::status(&bench.root, tools))
        .unwrap();
    assert_eq!(status.consistency, Consistency::Consistent(2));
    assert_eq!(status.nodes[1].reported, None);
}

/// A `/proc` tree with a process for each of `rows`: (pid, parent, ticks, cutime, rss and pss in
/// KiB, written bytes).
fn fake_proc(name: &str, rows: &[(u32, u32, u64, u64, u64, u64, u64)]) -> PathBuf {
    let proc = scratch(name);
    fs::create_dir_all(proc.join("self")).unwrap();
    for (pid, parent, ticks, reaped, rss, pss, written) in rows {
        let dir = proc.join(pid.to_string());
        fs::create_dir_all(&dir).unwrap();
        // The command name holds a space and a parenthesis, which the parser must get past.
        let stat = format!(
            "{pid} (a b) c) S {parent} 1 1 0 -1 0 0 0 0 0 {ticks} {ticks} {reaped} 0 20 0 1 0 5\n"
        );
        fs::write(dir.join("stat"), stat).unwrap();
        fs::write(
            dir.join("status"),
            format!("Name:\tx\nVmRSS:\t   {rss} kB\nThreads:\t1\n"),
        )
        .unwrap();
        fs::write(
            dir.join("smaps_rollup"),
            format!("Rss:\t{rss} kB\nPss:\t   {pss} kB\n"),
        )
        .unwrap();
        fs::write(
            dir.join("io"),
            format!("rchar: 5\nwrite_bytes: {written}\n"),
        )
        .unwrap();
    }
    proc
}

#[test]
fn resources_sum_a_whole_process_tree_from_a_fake_proc() {
    // 10 has a child 11 and a grandchild 12; 13 is a stranger; 14 is a child of 12 with no io file.
    let rows = [
        (10, 1, 100, 10, 1_000, 800, 4_096),
        (11, 10, 50, 0, 2_000, 1_500, 100),
        (12, 11, 5, 5, 300, 200, 1),
        (13, 1, 900, 900, 9_999, 9_999, 9_999),
        (14, 12, 1, 0, 10, 5, 0),
    ];
    let proc = fake_proc("proc-tree", &rows);
    fs::remove_file(proc.join("14/io")).unwrap();
    let used = devnet::tree_resources(&proc, 10).unwrap();
    // Ticks: user, system and the reaped children's, each 10 ms.
    let ticks = (100 * 2 + 10) + (50 * 2) + (5 * 2 + 5) + 2;
    let expected = Resources {
        rss: (1_000 + 2_000 + 300 + 10) * 1024,
        pss: Some((800 + 1_500 + 200 + 5) * 1024),
        cpu_ms: ticks * 10,
        written: Some(4_096 + 100 + 1),
    };
    assert_eq!(used, expected);
    assert_eq!(devnet::tree_resources(&proc, 12).unwrap().rss, 310 * 1024);
    assert_eq!(devnet::tree_resources(&proc, 99), None);
    // A tree with no smaps_rollup or io reads gives none of those.
    fs::remove_file(proc.join("13/smaps_rollup")).unwrap();
    fs::remove_file(proc.join("13/io")).unwrap();
    let alone = devnet::tree_resources(&proc, 13).unwrap();
    assert_eq!(
        (alone.pss, alone.written, alone.rss),
        (None, None, 9_999 * 1024)
    );
}

#[test]
fn status_resources_lists_the_running_processes_by_node_and_role() {
    let bench = Bench::new("resources");
    bench.init(&core(1, 1)).unwrap();
    let proc = fake_proc("resources-proc", &[(10, 1, 10, 0, 2_048, 1_024, 2_048_000)]);
    let resources = |bench: &Bench| {
        bench
            .with(|tools| devnet::resources_in(&bench.root, tools, &proc))
            .unwrap()
    };
    assert!(resources(&bench).is_empty());
    bench.ops.set("node1-devnet-core", SupervisedStatus::Online);
    bench
        .ops
        .pids
        .borrow_mut()
        .insert("node1-devnet-core".to_owned(), 10);
    bench
        .ops
        .pids
        .borrow_mut()
        .insert("node2-devnet-relay".to_owned(), 77);
    let rows = resources(&bench);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, "node1/core");
    assert_eq!(rows[0].1.cpu_ms, 200);
    assert_eq!(
        rows[0].1.to_string(),
        "rss 2 MiB, pss 1 MiB, cpu 200 ms, written 1 MiB"
    );
    let missing =
        bench.with(|tools| devnet::resources_in(&bench.root, tools, &proc.join("nowhere")));
    assert!(matches!(missing, Err(DevnetError::Guard(_))));
}

#[test]
fn status_resources_reads_a_real_process_tree() {
    // A shell with two sleeping children: the tree is three processes.
    let script = "sleep 60 & echo $!; sleep 60 & echo $!; wait";
    let mut shell = Command::new("sh")
        .args(["-c", script])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut lines = BufReader::new(shell.stdout.take().unwrap()).lines();
    let children: Vec<u32> = (0..2)
        .map(|_| lines.next().unwrap().unwrap().parse().unwrap())
        .collect();
    let proc = Path::new("/proc");
    let (whole, alone) = (
        devnet::tree_resources(proc, shell.id()),
        devnet::tree_resources(proc, children[0]),
    );
    // The children end before any assertion, so a failure leaves no process behind.
    let down: Vec<_> = children
        .iter()
        .map(|pid| Command::new("kill").arg(pid.to_string()).status())
        .collect();
    shell.wait().unwrap();
    assert!(down.iter().all(|status| status.is_ok()));
    let (whole, alone) = (whole.unwrap(), alone.unwrap());
    assert!(
        alone.rss > 0 && whole.rss > alone.rss,
        "{whole:?} {alone:?}"
    );
    assert!(whole.pss.is_some_and(|pss| pss > 0));
    assert!(whole.written.is_some());
    assert!(whole.pss.unwrap() <= whole.rss);
    assert_eq!(devnet::tree_resources(proc, shell.id()), None);
}

/// A writer the test reads while `follow` writes to it.
#[derive(Clone, Default)]
struct Shared(Arc<Mutex<Vec<u8>>>);

impl Write for Shared {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Shared {
    fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }

    /// Wait until the text is `want`, at most 10 s.
    fn expect(&self, want: &str) {
        let end = Instant::now() + Duration::from_secs(10);
        while self.text() != want && Instant::now() < end {
            thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(self.text(), want);
    }
}

#[test]
fn follow_keeps_tailing_the_growing_logs_with_their_prefixes() {
    let bench = Bench::new("follow");
    bench.init(&core(1, 1)).unwrap();
    let log = |name: &str| bench.root.join(name);
    let (core_log, relay_log) = (
        log("node1/log/node1-devnet-core-out.log"),
        log("node2/log/node2-devnet-relay-out.log"),
    );
    fs::create_dir_all(core_log.parent().unwrap()).unwrap();
    fs::write(&core_log, "old 1\nold 2\n").unwrap();
    let append = |path: &Path, text: &str| {
        let mut file = fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(path)
            .unwrap();
        file.write_all(text.as_bytes()).unwrap();
    };
    let (out, stop) = (Shared::default(), AtomicBool::new(false));
    thread::scope(|scope| {
        let mut writer = out.clone();
        let (root, stop) = (&bench.root, &stop);
        let follower = scope.spawn(move || {
            let all = NodeSelection::default();
            let stop = || stop.load(Ordering::SeqCst);
            devnet::follow(root, &all, LogStream::Out, 1, &mut writer, &stop)
        });
        // The last line of the existing log comes first; nothing older.
        out.expect("node1/core: old 2\n");
        append(&core_log, "new 1\nhalf a li");
        out.expect("node1/core: old 2\nnode1/core: new 1\n");
        append(&core_log, "ne\n");
        out.expect("node1/core: old 2\nnode1/core: new 1\nnode1/core: half a line\n");
        // A log that appears later is picked up from its start.
        fs::create_dir_all(relay_log.parent().unwrap()).unwrap();
        append(&relay_log, "relay 1\n");
        let all =
            "node1/core: old 2\nnode1/core: new 1\nnode1/core: half a line\nnode2/relay: relay 1\n";
        out.expect(all);
        // A log that shrinks is read again from its start.
        fs::write(&core_log, "fresh\n").unwrap();
        out.expect(&format!("{all}node1/core: fresh\n"));
        stop.store(true, Ordering::SeqCst);
        follower.join().unwrap().unwrap();
    });
    // One process alone has no prefix.
    let one = NodeSelection {
        nodes: vec![1],
        role: None,
    };
    let (mut writer, stopped) = (Shared::default(), || true);
    devnet::follow(&bench.root, &one, LogStream::Out, 5, &mut writer, &stopped).unwrap();
    assert_eq!(writer.text(), "fresh\n");
}

#[test]
fn keep_epoch_starts_without_the_re_stamp_a_near_epoch_would_get() {
    let bench = Bench::new("keep-epoch");
    let before = bench.init(&options(2)).unwrap();
    bench.api.every(2, &["genesis"]);
    let milestones = bench.file("genesis/crypto/milestones.json");
    // 5 s before the epoch, nothing running and no store: a plain start re-stamps.
    bench.clock.set(T0 + 25_000);
    let keep = StartOptions {
        keep_epoch: true,
        ..StartOptions::default()
    };
    bench.start(&keep).unwrap();
    assert_eq!(Manifest::read(&bench.root).unwrap(), before);
    assert_eq!(bench.file("genesis/crypto/milestones.json"), milestones);
    assert_eq!(
        bench.file("node2/config/crypto/milestones.json"),
        milestones
    );
    let all = NodeSelection::default();
    bench
        .with(|tools| devnet::stop(&bench.root, &all, false, tools))
        .unwrap();
    bench.start(&StartOptions::default()).unwrap();
    assert_ne!(Manifest::read(&bench.root).unwrap().epoch, before.epoch);
    // Past the epoch, keep-epoch keeps it too.
    bench
        .with(|tools| devnet::stop(&bench.root, &all, false, tools))
        .unwrap();
    let kept = Manifest::read(&bench.root).unwrap();
    bench.clock.set(T0 + 900_000);
    bench.start(&keep).unwrap();
    assert_eq!(Manifest::read(&bench.root).unwrap(), kept);
}

#[test]
fn no_wait_starts_the_forgers_without_waiting_for_the_relays_apis() {
    let bench = Bench::new("no-wait");
    bench.init(&options(2)).unwrap();
    // No relay's API answers.
    let quick = StartOptions {
        no_wait: true,
        ..StartOptions::default()
    };
    bench.start(&quick).unwrap();
    let started = [
        "node1-devnet-relay",
        "node2-devnet-relay",
        "node1-devnet-forger",
        "node2-devnet-forger",
    ];
    let calls: Vec<String> = started.iter().map(|name| format!("start {name}")).collect();
    assert_eq!(bench.ops.calls(), calls);
    assert!(bench.pauses.borrow().is_empty());
    assert_eq!(bench.api.polls.get(), 0);
    // Without the flag, the same devnet waits and gives up.
    let all = NodeSelection::default();
    bench
        .with(|tools| devnet::stop(&bench.root, &all, false, tools))
        .unwrap();
    bench.ops.calls();
    let timeout = bench.start(&StartOptions::default());
    assert!(
        matches!(timeout, Err(DevnetError::Timeout { seconds: 30, .. })),
        "{timeout:?}"
    );
    assert_eq!(
        bench.ops.calls(),
        ["start node1-devnet-relay", "start node2-devnet-relay"]
    );
}

#[test]
fn node_commands_refuse_roles_the_node_does_not_have() {
    let bench = Bench::new("missing-role");
    bench.init(&core(2, 1)).unwrap();
    for (node, role, roles) in [
        (1, Role::Forger, "core"),
        (2, Role::Relay, "core"),
        (3, Role::Core, "relay"),
    ] {
        for verb in ["start", "stop", "restart"] {
            let result = bench.with(|tools| match verb {
                "start" => devnet::start_node(&bench.root, node, Some(role), tools),
                "stop" => devnet::stop_node(&bench.root, node, Some(role), false, tools),
                _ => devnet::restart_node(&bench.root, node, Some(role), tools),
            });
            let error = result.unwrap_err().to_string();
            assert_eq!(
                error,
                format!("node {node} has no {role} role; its roles are {roles}")
            );
            assert!(bench.ops.calls().is_empty());
        }
    }
    drop(bench);
    let split = Bench::new("missing-core-role");
    split
        .init(&InitOptions {
            validators: 1,
            relays: 0,
            ..InitOptions::default()
        })
        .unwrap();
    let error = split.with(|tools| devnet::start_node(&split.root, 1, Some(Role::Core), tools));
    assert_eq!(
        error.unwrap_err().to_string(),
        "node 1 has no core role; its roles are relay and forger"
    );
}
