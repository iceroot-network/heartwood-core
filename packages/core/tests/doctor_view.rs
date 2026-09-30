//! `CoreNodeView`, the doctor's view of the node through the node's own code: the relay's and the
//! forger's facts are those their starts compute, field by field, with no `.env` value and no
//! `aux` among them; a refused configuration names its kind and file; the version floor is the
//! node's; and the store window is read only while the relay's lock is held, never opening a
//! store that no writer holds, and reads a live relay's store while it forges.

mod doctor_support;

use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::process::Command;

use doctor_support::{AUX, ENV_CANARY, Node};
use heartwood_cli::doctor::seams::{
    AccessEntry, LoadRefusal, NodeView, NodeViewError, WindowStart,
};
use heartwood_cli::{Instance, Role};
use heartwood_core::app::{api_options, check_floor};
use heartwood_core::commands::forger::ForgerRun;
use heartwood_core::commands::relay::RelayRun;
use heartwood_core::config::{ForgerConfig, Package, RelayConfig};
use heartwood_core::doctor_view::CoreNodeView;
use heartwood_core::lock::ProcessLock;
use heartwood_core::{VERSION, options};
use heartwood_database::{Database, Settings};
use heartwood_kernel::Clock;
use heartwood_kernel::paths::Paths;
use heartwood_logger::Level;
use heartwood_p2p::server::ServerOptions;
use serde_json::json;

const ENV: &str = "CORE_P2P_HOST=127.0.0.1\nCORE_P2P_PORT=6102\nCORE_API_HOST=127.0.0.1\n\
                   CORE_API_PORT=6103\nCORE_LOG_LEVEL_FILE=info\nCORE_P2P_MIN_NETWORK_REACH=3\n";

fn relay_run() -> RelayRun {
    let (token, network) = (Some("solar".into()), Some("testnet".into()));
    RelayRun {
        token,
        network,
        ..RelayRun::default()
    }
}

#[test]
fn the_relays_facts_are_those_its_start_computes() {
    let node = Node::new("view-relay", ENV);
    let facts = node.view().relay(&node.instance(Role::Relay)).unwrap();
    // What the relay's start reads, step by step.
    let run = relay_run();
    let config = RelayConfig::load(&run, &node.process(), &node.home(), &node.root).unwrap();
    let p2p = options::p2p(&config.entry(Package::P2p), &config.env).unwrap();
    let server = ServerOptions::from_p2p(&p2p);
    let logger = options::logger(&config.entry(Package::Logger), &config.env).unwrap();
    let api = api_options(&config).unwrap().unwrap();
    assert_eq!(facts.p2p, SocketAddr::new(server.host, server.port));
    assert_eq!(facts.p2p, "127.0.0.1:6102".parse().unwrap());
    assert_eq!(facts.api, Some(api.address));
    assert_eq!(facts.api, Some("127.0.0.1:6103".parse().unwrap()));
    let exact: Vec<IpAddr> = (facts.remote_access.iter())
        .filter_map(|entry| match entry {
            AccessEntry::Address(address) => Some(address.to_canonical()),
            _ => None,
        })
        .collect();
    assert_eq!(exact, server.remote_access);
    let prefix = AccessEntry::Prefix {
        address: "10.0.0.0".parse().unwrap(),
        bits: 24,
    };
    assert_eq!(facts.remote_access.get(2), Some(&prefix));
    assert_eq!(facts.minimum_network_reach, 3);
    assert_eq!(
        usize::try_from(facts.minimum_network_reach).unwrap(),
        p2p.minimum_network_reach
    );
    assert_eq!(Some(facts.file_log_level), logger.file);
    assert_eq!(facts.file_log_level, Level::Info);
    assert_eq!(facts.warnings, config.warnings);
    assert_eq!(facts.chain.nethash, config.crypto.network.nethash);
    let milestone = config.crypto.milestones.at(1);
    assert_eq!(u64::from(facts.chain.block_time_s), milestone.block_time());
    assert_eq!(
        u64::from(facts.chain.active_delegates),
        milestone.active_delegates()
    );
    // The corpus's epoch, 2026-09-26T17:27:54Z, in Unix milliseconds.
    assert_eq!(facts.chain.epoch_ms, 1_790_443_674_000);
    assert_eq!(
        facts.store_file,
        Settings::in_data_dir(&node.paths().data).path
    );
    // Variables by name only: no value of the `.env` crosses the seam.
    assert!(facts.env_names.contains(&"DOCTOR_CANARY".to_owned()));
    assert!(facts.env_names.contains(&"CORE_P2P_PORT".to_owned()));
    assert!(!format!("{facts:?}").contains(ENV_CANARY));
}

#[test]
fn a_warning_and_a_disabled_api_are_the_relays_own() {
    let env = format!("{ENV}CORE_API_CACHE=true\nCORE_API_DISABLED=true\n");
    let node = Node::new("view-relay-warns", &env);
    let facts = node.view().relay(&node.instance(Role::Relay)).unwrap();
    assert_eq!(facts.warnings, ["CORE_API_CACHE is not honoured"]);
    assert_eq!(facts.api, None);
    // A host name is never looked up: the API counts as not served.
    let node = Node::new(
        "view-relay-host-name",
        &ENV.replace("CORE_API_HOST=127.0.0.1", "CORE_API_HOST=api.invalid"),
    );
    let facts = node.view().relay(&node.instance(Role::Relay)).unwrap();
    assert_eq!(facts.api, None);
    // The file level off counts as the most severe level.
    let node = Node::new(
        "view-relay-no-file-log",
        &format!("{ENV}CORE_LOG_LEVEL_FILE=off\n"),
    );
    let facts = node.view().relay(&node.instance(Role::Relay)).unwrap();
    assert_eq!(facts.file_log_level, Level::Emergency);
}

#[test]
fn the_forgers_facts_are_those_its_start_computes_without_its_aux_or_key_file() {
    let node = Node::new("view-forger", ENV);
    // The key file cannot even be read: the loader never opens it.
    let key = node.config().join(heartwood_forger::keys::FILE);
    std::fs::write(&key, "{}").unwrap();
    node.chmod(&key, 0o000);
    let facts = node.view().forger(&node.instance(Role::Forger)).unwrap();
    let run = ForgerRun {
        token: Some("solar".into()),
        network: Some("testnet".into()),
        env: None,
    };
    let config = ForgerConfig::load(&run, &node.process(), &node.home(), &node.root).unwrap();
    let env = |name: &str| config.base.env.get(name).map(str::to_owned);
    let options = heartwood_forger::Options::from_json(&config.forger, &env).unwrap();
    let logger = options::logger(&config.logger, &config.base.env).unwrap();
    let hosts: Vec<SocketAddr> = (options.hosts.iter())
        .map(|host| SocketAddr::new(host.address, host.port))
        .collect();
    assert_eq!(facts.hosts, hosts);
    assert_eq!(facts.hosts, ["127.0.0.1:6102".parse().unwrap()]);
    assert_eq!(facts.local_address, options.local_address);
    assert!(facts.aux_set && options.aux.is_some());
    assert_eq!(Some(facts.file_log_level), logger.file);
    assert_eq!(facts.key_file, heartwood_forger::keys::path(&node.config()));
    assert_eq!(facts.key_file, key);
    assert_eq!(
        facts.signing_record,
        heartwood_forger::signing_record::path(&node.paths().data)
    );
    assert_eq!(facts.chain.nethash, config.crypto.network.nethash);
    let printed = format!("{facts:?}");
    assert!(
        !printed.contains(AUX) && !printed.contains(ENV_CANARY),
        "{printed}"
    );
    // Without `aux`, it says so.
    node.app(json!(["127.0.0.1"]), json!({}));
    let facts = node.view().forger(&node.instance(Role::Forger)).unwrap();
    assert!(!facts.aux_set);
    assert_eq!(facts.hosts, ["127.0.0.1:6102".parse().unwrap()]);
}

#[test]
fn a_refused_configuration_names_its_kind_and_the_file() {
    let refused = |node: &Node, role: Role| {
        let view = node.view();
        let found = match role {
            Role::Relay => view.relay(&node.instance(role)).map(drop),
            Role::Forger => view.forger(&node.instance(role)).map(drop),
            Role::Core => panic!("the doctor reads the split roles only"),
        };
        match found {
            Err(NodeViewError::Refused { kind, file }) => (kind, file.unwrap()),
            other => panic!("{other:?}"),
        }
    };
    let node = Node::new("view-refused", ENV);
    let config = node.config();
    node.write("peers.json", "not json");
    assert_eq!(
        refused(&node, Role::Relay),
        (LoadRefusal::Peers, config.join("peers.json"))
    );
    // The forger does not read peers.json.
    assert!(node.view().forger(&node.instance(Role::Forger)).is_ok());
    node.write(
        "peers.json",
        r#"{"list": [], "sources": ["https://seeds.example/"]}"#,
    );
    assert_eq!(refused(&node, Role::Relay).0, LoadRefusal::Peers);
    node.write("peers.json", r#"{"list": []}"#);
    std::fs::remove_file(config.join("crypto/exceptions.json")).unwrap();
    for role in Role::ALL {
        assert_eq!(
            refused(&node, role),
            (LoadRefusal::Crypto, config.join("crypto"))
        );
    }
    let node = Node::new("view-refused-app", ENV);
    node.write("app.json", "{");
    for role in Role::ALL {
        assert_eq!(
            refused(&node, role),
            (LoadRefusal::AppProfile, node.config().join("app.json"))
        );
    }
    let node = Node::new("view-refused-profile", ENV);
    node.write(
        "app.json",
        r#"{"relay": {"plugins": [{"package": "@solar-network/p2p"}]}, "forger": {"plugins": []}}"#,
    );
    for role in Role::ALL {
        assert_eq!(refused(&node, role).0, LoadRefusal::AppProfile);
    }
    let node = Node::new("view-refused-env", "CORE_P2P_PORT=abc\n");
    assert_eq!(
        refused(&node, Role::Relay),
        (LoadRefusal::Options, node.config().join("app.json"))
    );
    std::fs::remove_file(node.config().join(".env")).unwrap();
    std::fs::create_dir(node.config().join(".env")).unwrap();
    assert_eq!(
        refused(&node, Role::Forger),
        (LoadRefusal::EnvFile, node.config().join(".env"))
    );
    // No home directory: the roles' starts refuse to run, and so do their loaders here.
    let homeless = CoreNodeView {
        home: PathBuf::new(),
        ..node.view()
    };
    let other = NodeViewError::Refused {
        kind: LoadRefusal::Other,
        file: None,
    };
    for role in Role::ALL {
        let found = match role {
            Role::Relay => homeless.relay(&node.instance(role)).map(drop),
            Role::Forger => homeless.forger(&node.instance(role)).map(drop),
            Role::Core => panic!("Role::ALL holds the split roles only"),
        };
        assert_eq!(found, Err(other.clone()));
    }
    let node = Node::new("view-refused-options", ENV);
    node.app(json!(["127.0.0.1"]), json!({"aux": "not hex"}));
    assert_eq!(refused(&node, Role::Forger).0, LoadRefusal::Options);
}

#[test]
fn the_version_floor_is_the_nodes() {
    let node = Node::new("view-floor", ENV);
    let relay = node.instance(Role::Relay);
    let view = node.view();
    let config =
        RelayConfig::load(&relay_run(), &node.process(), &node.home(), &node.root).unwrap();
    let (milestones, network) = (&config.crypto.milestones, &config.crypto.network.name);
    let own = view.floor(&relay, VERSION, 1).unwrap();
    assert_eq!(
        own.refused_now,
        check_floor(milestones, network, 1, &[]).is_err()
    );
    assert!(!own.refused_now && own.refused_from.is_empty());
    // The corpus's floor is >=4.3.0 from height 1.
    let old = view.floor(&relay, "4.2.9", 5).unwrap();
    assert!(old.refused_now && old.refused_from.is_empty());
    // A later milestone that raises the floor refuses this version from its height on.
    let path = node.config().join("crypto/milestones.json");
    let mut all: Vec<serde_json::Value> =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    all.push(json!({"height": 500, "p2p": {"minimumVersions": [">=9.0.0"]}}));
    std::fs::write(&path, serde_json::Value::Array(all).to_string()).unwrap();
    let raised = view.floor(&relay, VERSION, 100).unwrap();
    assert_eq!(
        (raised.refused_now, raised.refused_from),
        (false, vec![500])
    );
    let past = view.floor(&relay, VERSION, 600).unwrap();
    assert_eq!((past.refused_now, past.refused_from.len()), (true, 0));
}

/// Every file of `dir` and its subdirectories with its bytes.
fn tree(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        match path.is_dir() {
            true => out.extend(tree(&path)),
            false => out.push((path.clone(), std::fs::read(&path).unwrap())),
        }
    }
    out.sort();
    out
}

#[test]
fn a_stopped_relays_store_is_never_opened_and_stays_byte_for_byte() {
    let node = Node::new("view-store-stopped", ENV);
    let settings = Settings::in_data_dir(&node.paths().data);
    drop(Database::open(&settings).unwrap());
    let relay = node.instance(Role::Relay);
    let before = tree(&node.paths().data);
    let names: Vec<_> = before
        .iter()
        .map(|(path, _)| path.file_name().unwrap().to_owned())
        .collect();
    assert_eq!(names, ["blockchain.db"]);
    let window = node
        .view()
        .store_window(&relay, &["genesis_1".into()], WindowStart::Height(1));
    assert_eq!(window, Err(NodeViewError::RelayNotRunning));
    assert_eq!(tree(&node.paths().data), before);
    // The lock held, the store not yet opened by the relay: still nothing is opened.
    let lock = ProcessLock::acquire(&relay).unwrap();
    let before = tree(&node.paths().data);
    let window = node
        .view()
        .store_window(&relay, &["genesis_1".into()], WindowStart::Height(1));
    assert_eq!(window, Err(NodeViewError::RelayNotRunning));
    assert_eq!(tree(&node.paths().data), before);
    // The lock held and a writer: the window is read.
    let writer = Database::open(&settings).unwrap();
    let window = node
        .view()
        .store_window(&relay, &["genesis_1".into()], WindowStart::Height(1))
        .unwrap();
    assert!(window.blocks.is_empty() && window.missed.is_empty() && !window.capped);
    drop((writer, lock));
}

/// `heartwood devnet <args> --dir <root>`, which must succeed; its standard output.
fn devnet(root: &Path, args: &[&str]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_heartwood"))
        .arg("devnet")
        .args(args)
        .arg("--dir")
        .arg(root)
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "devnet {args:?}: {text}");
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// Stops the devnet of its root when dropped, the test failed or not.
struct Stop<'a>(&'a Path);

impl Drop for Stop<'_> {
    fn drop(&mut self) {
        let _ = Command::new(env!("CARGO_BIN_EXE_heartwood"))
            .args(["devnet", "stop", "--dir"])
            .arg(self.0)
            .output();
    }
}

#[test]
#[ignore = "devnet"]
fn the_store_window_reads_a_forging_relays_store_while_it_commits() {
    let prefix = std::env::var("HEARTWOOD_DEVNET_PREFIX").unwrap_or_else(|_| "127.24.0".into());
    let root = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("doctor")
        .join("view-devnet");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let init = [
        "init",
        "--validators",
        "1",
        "--address-prefix",
        &prefix,
        "--epoch-delay",
        "5",
    ];
    devnet(&root, &init);
    let _stop = Stop(&root);
    devnet(&root, &["start"]);
    devnet(&root, &["wait", "--height", "4", "--timeout", "150"]);
    let paths = Paths {
        config: root.join("node1/config"),
        data: root.join("node1/data"),
        cache: root.join("node1/cache"),
        log: root.join("node1/log"),
        temp: root.join("node1/temp"),
    };
    let instance = |role| Instance {
        token: "node1".into(),
        network: "devnet".into(),
        role,
        paths: paths.clone(),
    };
    let home = root.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let view = CoreNodeView {
        process: Vec::new(),
        home,
        cwd: root.clone(),
    };
    let relay = view.relay(&instance(Role::Relay)).unwrap();
    assert_eq!(relay.p2p.ip().to_string(), format!("{prefix}.11"));
    let forger = view.forger(&instance(Role::Forger)).unwrap();
    assert_eq!(forger.hosts, [relay.p2p]);
    // The store is read in a loop, about 30 s, while the relay forges and commits.
    let clock = heartwood_kernel::SystemClock;
    let (start, mut tips, mut reads) = (clock.now_ms(), Vec::new(), 0);
    while clock.now_ms() - start < 30_000 {
        let window = view
            .store_window(
                &instance(Role::Relay),
                &["genesis_1".into()],
                WindowStart::Height(1),
            )
            .unwrap();
        let tip = window.blocks.last().unwrap();
        // The same read transaction gives the store's own tip.
        assert_eq!(window.tip, Some(tip.height));
        // Network-epoch seconds, against the milestones' epoch: the tip is from the last minute.
        let unix_ms = relay.chain.epoch_ms + i64::from(tip.timestamp) * 1000;
        assert!(
            (clock.now_ms() - 60_000..=clock.now_ms() + 8_000).contains(&unix_ms),
            "{unix_ms}"
        );
        let heights: Vec<u32> = window.blocks.iter().map(|block| block.height).collect();
        assert_eq!(heights, (1..=tip.height).collect::<Vec<_>>());
        if tips.last() != Some(&tip.height) {
            tips.push(tip.height);
        }
        reads += 1;
    }
    // The relay kept committing a block per slot while it was read.
    assert!(tips.len() >= 3, "{tips:?} in {reads} reads");
    // Two blocks below the store's own tip: the tip and the two before it, whatever the API says.
    let below = view.store_window(&instance(Role::Relay), &[], WindowStart::BelowTip(2));
    let below = below.unwrap();
    let heights: Vec<u32> = below.blocks.iter().map(|block| block.height).collect();
    let tip = below.tip.unwrap();
    assert_eq!(heights, [tip - 2, tip - 1, tip]);
    let last = *tips.last().unwrap();
    devnet(
        &root,
        &[
            "wait",
            "--height",
            &(last + 1).to_string(),
            "--timeout",
            "30",
        ],
    );
    let floor = view.floor(&instance(Role::Relay), VERSION, last).unwrap();
    assert!(!floor.refused_now);
    devnet(&root, &["stop"]);
    // Stopped, the store is not opened again.
    let stopped = view.store_window(&instance(Role::Relay), &[], WindowStart::Height(1));
    assert_eq!(stopped, Err(NodeViewError::RelayNotRunning));
}
