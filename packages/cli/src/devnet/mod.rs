//! The local devnet library, mode B: every node of a devnet on one host, each at its own loopback
//! address, as separate relay and forger processes under their supervisors.
//!
//! - **Addresses.** Node k's relay listens at `127.P.(10+k)`, P2P on 6002 and the API on 6003,
//!   where `P` is the address prefix (default `127.0.0`); its forger connects from the same
//!   address, or in a failover layout ([`Failover`]) from `127.P.(110+k)`, which each relay it
//!   may use lists in its `remoteAccess`.
//! - **[`init`]** checks the counts, test-binds every listener, has the chain generated into
//!   `genesis/`, writes each `node<k>/` (token `node<k>`, network `devnet`, its `.env`, `app.json`
//!   with the relay's `remoteAccess` and `devnet` block and the forger's `hosts`, `localAddress`
//!   and seed-derived `aux`, its share of the delegates' keys split 11/11/11/10/10 over five
//!   validators, `peers.json` of the other nodes, the crypto files) and the manifest
//!   `devnet.json` last.
//! - **[`start`]** checks the crypto files' hashes and the listeners, re-stamps the epoch when no
//!   node has a chain store, nothing of the devnet runs and the epoch is less than 10 s away or
//!   past, starts every relay, waits for their APIs (30 s at most), then starts the forgers.
//!   **[`stop`]** stops the forgers before the relays; `kill` sends SIGKILL, waits for the exit,
//!   then records the stop.
//! - **[`status`]** reads the supervisors and each relay's node status, last block and peers,
//!   compares the block ids at the lowest common height (`consistent` or `DIVERGED at h`), and
//!   counts the delegates whose forger is online against the quorum's 27.
//! - **[`reset`]** deletes a node's data, logs, temp and cache (the forger's signing record
//!   stays); **[`wait`]** polls the relays' heights every 2 s; **[`logs`]** merges the nodes'
//!   logs in time order with prefixes; **[`env()`]** gives a node's environment.
//! - **Guards.** A prefix outside 127.0.0.0/8, a listener address another process holds, a
//!   network whose name is not `devnet` (or `devnet-...`) and a node `.env` that binds a
//!   listener to an address that is not loopback are refused.

use std::fmt;
use std::fs;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;

use heartwood_crypto::utils::hex;
use heartwood_kernel::Clock;
use heartwood_kernel::log::Level;
use heartwood_kernel::paths::Paths;
use heartwood_kernel::timestamp::UtcDateTime;
use serde_json::{Value, json};
use zeroize::Zeroizing;

use crate::devnet::manifest::{crypto_files, hashes, io, pretty, write, write_private};
use crate::errors::DevnetError;
use crate::seams::{ApiBlock, ApiNodeStatus, ChainGenerator, GenerateOptions, GeneratedDelegate};
use crate::seams::{Instance, NodeApi, ProcessOps, Profile, Role, StatusRow};

mod control;
mod manifest;
mod report;

pub use crate::devnet::control::{reset, restart_node, start, start_node, stop, stop_node, wait};
pub use crate::devnet::report::{LogStream, env, logs, status};

type Result<T, E = DevnetError> = std::result::Result<T, E>;

/// Every node's peer-to-peer port.
pub const P2P_PORT: u16 = 6002;
/// Every node's API port.
pub const API_PORT: u16 = 6003;
/// Every node's webhooks port, reserved: webhooks never start.
pub const WEBHOOKS_PORT: u16 = 6004;
/// The manifest's file name in the devnet's directory.
pub const MANIFEST: &str = "devnet.json";
/// The default address prefix.
pub const DEFAULT_PREFIX: [u8; 3] = [127, 0, 0];
/// The default key-derivation seed, public, so that every machine builds the same chain.
pub const DEFAULT_SEED: &str = "solar-devnet";
/// Every devnet node's network name.
pub const NETWORK: &str = "devnet";
/// The quorum a forging devnet needs: at least 27 of the 53 delegates.
pub const QUORUM: u32 = 27;
/// The most nodes of one devnet, so that node addresses end at `127.P.110`.
pub const MAX_NODES: u32 = 100;
/// The crypto files of a network, in the `crypto/` directory of the chain and of every node.
pub const CRYPTO_FILES: [&str; 4] = [
    "network.json",
    "milestones.json",
    "genesisBlock.json",
    "exceptions.json",
];
/// The relay profile's components, in the reference's order, before and after `p2p`. The
/// webhooks entry stays, as the reference's profile has it; a relay never starts it.
const RELAY_PLUGINS: (&str, &str) = (
    "logger state database transactions pool",
    "blockchain api webhooks",
);

/// Node `k`'s address (1-based): `127.P.(10+k)`.
pub fn node_address(prefix: [u8; 3], k: u32) -> Option<IpAddr> {
    loopback(prefix, k, 10)
}

/// The address of node `k`'s forger in a failover layout: `127.P.(110+k)`.
pub fn forger_address(prefix: [u8; 3], k: u32) -> Option<IpAddr> {
    loopback(prefix, k, 110)
}

fn loopback([a, b, c]: [u8; 3], k: u32, offset: u8) -> Option<IpAddr> {
    let last = u8::try_from(k).ok()?.checked_add(offset)?;
    Some(IpAddr::V4(Ipv4Addr::new(a, b, c, last)))
}

/// What the devnet's commands act through: the process operations, the relays' APIs, the
/// clock, and the wait between two polls (`std::thread::sleep` in a command).
#[derive(Clone, Copy)]
pub struct Tools<'a> {
    /// The process operations on one role of one node.
    pub ops: &'a dyn ProcessOps,
    /// The reads of a relay's API.
    pub api: &'a dyn NodeApi,
    /// The time.
    pub clock: &'a dyn Clock,
    /// Waits for the given time.
    pub pause: &'a dyn Fn(Duration),
}

/// A forger that can fail over: node `node`'s forger uses the relays of the nodes `hosts`, in
/// that order, from its own address `127.P.(110+node)`, which each of those relays admits in its
/// `remoteAccess`. Written `node:host,host,...`, such as `1:1,6,7`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failover {
    /// The validator node whose forger fails over.
    pub node: u32,
    /// The nodes whose relays it uses, in order.
    pub hosts: Vec<u32>,
}

impl FromStr for Failover {
    type Err = String;

    fn from_str(text: &str) -> Result<Failover, String> {
        let invalid = || format!("{text:?} is not a failover such as 1:1,6,7");
        let (node, hosts) = text.split_once(':').ok_or_else(invalid)?;
        let number = |part: &str| part.trim().parse().map_err(|_| invalid());
        let hosts = hosts.split(',').map(number).collect::<Result<_, _>>()?;
        let node = number(node)?;
        Ok(Failover { node, hosts })
    }
}

impl fmt::Display for Failover {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let hosts: Vec<String> = self.hosts.iter().map(u32::to_string).collect();
        write!(f, "{}:{}", self.node, hosts.join(","))
    }
}

/// What `init` builds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitOptions {
    /// The nodes with a relay and a forger (1 to 53).
    pub validators: u32,
    /// The nodes with a relay only, after the validators.
    pub relays: u32,
    /// The chain's key-derivation seed.
    pub seed: String,
    /// The first three bytes of every node address, in 127.0.0.0/8.
    pub address_prefix: [u8; 3],
    /// The forgers that fail over to other nodes' relays.
    pub failover: Vec<Failover>,
    /// The seconds from `init` (or from a re-stamping `start`) to the epoch.
    pub epoch_delay: u32,
    /// Every relay's `CORE_P2P_MIN_NETWORK_REACH`.
    pub min_network_reach: u32,
    /// Every node's log level, on the console and in its log file.
    pub log_level: String,
    /// Replace a devnet the directory already holds, stopping its processes first.
    pub force: bool,
}

impl Default for InitOptions {
    /// Five validators, no relay-only node, the public seed, the prefix `127.0.0`, a 30 s epoch
    /// delay, a network reach of 1 and the `info` level.
    fn default() -> InitOptions {
        InitOptions {
            validators: 5,
            relays: 0,
            seed: DEFAULT_SEED.to_owned(),
            address_prefix: DEFAULT_PREFIX,
            failover: Vec::new(),
            epoch_delay: 30,
            min_network_reach: 1,
            log_level: "info".to_owned(),
            force: false,
        }
    }
}

/// One node of a devnet, as the manifest describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestNode {
    /// The node's number, from 1.
    pub index: u32,
    /// The relay's address.
    pub address: IpAddr,
    /// The forger's source address; `None` for a relay-only node.
    pub forger_address: Option<IpAddr>,
    /// The relays the node's forger uses, in order; for a relay-only node, its own relay, which
    /// its forger profile names although no forger runs.
    pub forger_hosts: Vec<SocketAddr>,
    /// The usernames of the delegates the node's forger holds, in genesis order.
    pub delegates: Vec<String>,
    /// The node's directory, `<root>/node<k>`.
    pub dir: PathBuf,
}

impl ManifestNode {
    /// The relay's peer-to-peer address.
    pub fn p2p(&self) -> SocketAddr {
        SocketAddr::new(self.address, P2P_PORT)
    }

    /// The relay's API address.
    pub fn api(&self) -> SocketAddr {
        SocketAddr::new(self.address, API_PORT)
    }

    /// The node's five directories.
    pub fn paths(&self) -> Paths {
        let dir = |name: &str| self.dir.join(name);
        let (config, data, cache, log) = (dir("config"), dir("data"), dir("cache"), dir("log"));
        let temp = dir("temp");
        Paths {
            config,
            data,
            cache,
            log,
            temp,
        }
    }

    /// The instance of `role` on this node: token `node<k>`, network `devnet`.
    pub fn instance(&self, role: Role) -> Instance {
        let (token, network) = (format!("node{}", self.index), NETWORK.to_owned());
        let paths = self.paths();
        Instance {
            token,
            network,
            role,
            paths,
        }
    }

    /// The node's roles, relay first, or only `role` when the node has it.
    pub fn roles(&self, role: Option<Role>) -> Vec<Role> {
        let has = |r: &Role| *r == Role::Relay || self.forger_address.is_some();
        let wanted = |r: &Role| role.is_none_or(|only| only == *r);
        Role::ALL
            .into_iter()
            .filter(|r| has(r) && wanted(r))
            .collect()
    }
}

/// The manifest `devnet.json`, written last by `init`: the layout, the chain's identity and the
/// crypto files' hashes. The node addresses follow from the prefix, the counts and the failover
/// layout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    /// The chain's seed.
    pub seed: String,
    /// The address prefix.
    pub address_prefix: [u8; 3],
    /// The nodes with a relay and a forger.
    pub validators: u32,
    /// The nodes with a relay only.
    pub relays: u32,
    /// The forgers that fail over.
    pub failover: Vec<Failover>,
    /// The seconds to the epoch that a re-stamp gives.
    pub epoch_delay: u32,
    /// The chain's nethash.
    pub nethash: String,
    /// The genesis block's id.
    pub genesis_id: String,
    /// The chain's epoch.
    pub epoch: String,
    /// The SHA-256 of each crypto file, by file name, in [`CRYPTO_FILES`] order.
    pub crypto_hashes: Vec<(String, String)>,
    /// The nodes, in order.
    pub nodes: Vec<ManifestNode>,
}

impl Manifest {
    /// Node `k`.
    pub fn node(&self, k: u32) -> Result<&ManifestNode> {
        let mut nodes = self.nodes.iter();
        nodes
            .find(|node| node.index == k)
            .ok_or(DevnetError::NoNode(k))
    }

    /// The numbers of the nodes `selection` names, in order; every node when it names none.
    pub fn select(&self, selection: &NodeSelection) -> Result<Vec<u32>> {
        let mut picked = selection.nodes.clone();
        if picked.is_empty() {
            picked = self.nodes.iter().map(|node| node.index).collect();
        }
        picked.sort_unstable();
        picked.dedup();
        picked.iter().try_for_each(|k| self.node(*k).map(drop))?;
        Ok(picked)
    }
}

/// The nodes and roles a command acts on.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NodeSelection {
    /// The nodes, by number; every node when empty.
    pub nodes: Vec<u32>,
    /// The role; both when `None`.
    pub role: Option<Role>,
}

/// Whether the relays agree on their chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Consistency {
    /// Every relay that answered has the same block at this height, the lowest common one.
    Consistent(u32),
    /// The relays' blocks differ at this height, the lowest common one.
    Diverged(u32),
    /// No relay's API answered.
    Unknown,
}

impl fmt::Display for Consistency {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Consistency::Consistent(height) => write!(f, "consistent at height {height}"),
            Consistency::Diverged(height) => write!(f, "DIVERGED at {height}"),
            Consistency::Unknown => f.write_str("unknown: no relay answered"),
        }
    }
}

/// A relay's API, as `status` read it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayView {
    /// `GET /api/node/status`.
    pub status: ApiNodeStatus,
    /// `GET /api/blocks/last`.
    pub last_block: ApiBlock,
    /// The number of peers `GET /api/peers` lists.
    pub peers: usize,
}

/// One node in `status`.
#[derive(Debug, Clone, PartialEq)]
pub struct NodeStatus {
    /// The node's number.
    pub node: u32,
    /// The relay's supervised status.
    pub relay: StatusRow,
    /// The forger's supervised status; `None` for a relay-only node.
    pub forger: Option<StatusRow>,
    /// The relay's API, when every read answered.
    pub api: Option<RelayView>,
    /// The delegates the node's forger holds.
    pub delegates: u32,
}

/// What `status` reports; its text is the chain line.
#[derive(Debug, Clone, PartialEq)]
pub struct DevnetStatus {
    /// Each node.
    pub nodes: Vec<NodeStatus>,
    /// The chain's consistency across the relays that answered.
    pub consistency: Consistency,
    /// The delegates whose forger is online.
    pub online_delegates: u32,
}

/// Create the devnet of `options` in `root`: its chain from `generator`, its epoch `epoch_delay`
/// seconds after the clock's time. With `force`, the devnet the directory holds is replaced, its
/// processes stopped first. A failed `init` leaves no devnet files behind.
pub fn init(
    root: &Path,
    options: &InitOptions,
    generator: &dyn ChainGenerator,
    tools: &Tools<'_>,
) -> Result<Manifest> {
    let root = std::path::absolute(root).map_err(io(root))?;
    let [a, b, c] = options.address_prefix;
    if a != 127 {
        return Err(guard(format!("{a}.{b}.{c} is not a loopback prefix")));
    }
    if Level::from_name(&options.log_level).is_none() {
        return Err(guard(format!("{:?} is not a log level", options.log_level)));
    }
    let (count, relays) = (options.validators, options.relays);
    let prefix = options.address_prefix;
    let mut nodes = plan(&root, prefix, count, relays, &options.failover)?;
    manifest::clear(&root, options.force, tools.ops)?;
    let forger = |n: &ManifestNode| n.forger_address.filter(|a| *a != n.address);
    let spare = |n: &ManifestNode| forger(n).map(|a| SocketAddr::new(a, 0));
    let bind = |n: &ManifestNode| [Some(n.p2p()), Some(n.api()), spare(n)];
    test_bind(nodes.iter().flat_map(bind).flatten())?;
    let built = build(&root, options, &mut nodes, generator, tools.clock);
    if built.is_err() {
        manifest::remove_all(&root);
    }
    built
}

/// Generate the chain, write the nodes' files, then the manifest.
fn build(
    root: &Path,
    options: &InitOptions,
    nodes: &mut [ManifestNode],
    generator: &dyn ChainGenerator,
    clock: &dyn Clock,
) -> Result<Manifest> {
    let genesis = root.join("genesis");
    let generate = GenerateOptions {
        network: NETWORK.to_owned(),
        seed: options.seed.clone(),
        epoch: Some(epoch_after(clock.now_ms(), options.epoch_delay)?),
        profile: Profile::Devnet,
        pub_key_hash: None,
        token: None,
        symbol: None,
        explorer: None,
        out: genesis.clone(),
        chain_only: true,
        force: false,
    };
    let generated = generator.generate(&generate, clock)?;
    let files = crypto_files(&genesis.join("crypto"))?;
    let json = |i: usize| {
        let bytes = files.get(i).map(|(_, bytes)| bytes.as_slice());
        serde_json::from_slice(bytes.unwrap_or_default()).unwrap_or(Value::Null)
    };
    let (network, milestones) = (json(0), json(1));
    let text = |value: Option<&Value>| value.and_then(Value::as_str).map(str::to_owned);
    let name = text(network.get("name")).unwrap_or_default();
    if name != NETWORK && !name.starts_with("devnet-") {
        return Err(guard(format!("the network is named {name:?}, not devnet")));
    }
    let nethash = text(network.get("nethash"));
    let epoch = text(milestones.get(0).and_then(|first| first.get("epoch")));
    let (Some(nethash), Some(epoch)) = (nethash, epoch) else {
        return Err(guard("the chain has no nethash or no epoch".to_owned()));
    };
    let keys = generated.delegates.as_slice();
    let validators = usize::try_from(options.validators).unwrap_or(1).max(1);
    let all = keys.len();
    if validators > all {
        let text = format!("the chain has {all} delegates only");
        return Err(DevnetError::Counts(text));
    }
    let mut rest = keys;
    for (i, node) in nodes.iter_mut().take(validators).enumerate() {
        let size = all / validators + usize::from(i < all % validators);
        let (share, after) = rest.split_at(size.min(rest.len()));
        node.delegates = share.iter().map(|key| key.username.clone()).collect();
        rest = after;
    }
    let mut rest = keys;
    for node in nodes.iter() {
        let (share, after) = rest.split_at(node.delegates.len().min(rest.len()));
        write_node(node, nodes, options, &files, share, &generated.forger_aux)?;
        rest = after;
    }
    let manifest = Manifest {
        seed: options.seed.clone(),
        address_prefix: options.address_prefix,
        validators: options.validators,
        relays: options.relays,
        failover: options.failover.clone(),
        epoch_delay: options.epoch_delay,
        nethash,
        genesis_id: generated.genesis_id,
        epoch,
        crypto_hashes: hashes(&files),
        nodes: nodes.to_vec(),
    };
    manifest.write(root)?;
    Ok(manifest)
}

/// The nodes of a layout, checked: 1 to 53 validators, at most [`MAX_NODES`] nodes, and each
/// failover naming a validator once and nodes of the devnet as its hosts, each once.
fn plan(
    root: &Path,
    prefix: [u8; 3],
    validators: u32,
    relays: u32,
    failover: &[Failover],
) -> Result<Vec<ManifestNode>> {
    let total = validators.saturating_add(relays);
    if !(1..=53).contains(&validators) || total > MAX_NODES {
        let text = format!("a devnet has 1 to 53 validators, {MAX_NODES} nodes at most");
        return Err(DevnetError::Counts(text));
    }
    let once = |k: &u32, list: &[u32]| list.iter().filter(|x| *x == k).count() == 1;
    let forgers: Vec<u32> = failover.iter().map(|entry| entry.node).collect();
    for entry in failover {
        let host = |h: &u32| (1..=total).contains(h) && once(h, &entry.hosts);
        let hosts = !entry.hosts.is_empty() && entry.hosts.iter().all(host);
        let node = (1..=validators).contains(&entry.node) && once(&entry.node, &forgers);
        if !hosts || !node {
            let text = format!("failover {entry}: a validator once, then nodes, each once");
            return Err(DevnetError::Counts(text));
        }
    }
    let at = |k: u32| node_address(prefix, k).ok_or(DevnetError::NoNode(k));
    let node = |k: u32| {
        let layout = failover.iter().find(|entry| entry.node == k);
        let hosts = layout.map_or_else(|| vec![k], |entry| entry.hosts.clone());
        let socket = |h: u32| Ok(SocketAddr::new(at(h)?, P2P_PORT));
        let hosts = hosts.into_iter().map(socket).collect::<Result<_>>()?;
        let forger = match layout {
            Some(_) => forger_address(prefix, k),
            None => Some(at(k)?),
        };
        Ok(ManifestNode {
            index: k,
            address: at(k)?,
            forger_address: forger.filter(|_| k <= validators),
            forger_hosts: hosts,
            delegates: Vec::new(),
            dir: root.join(format!("node{k}")),
        })
    };
    (1..=total).map(node).collect()
}

/// Write node `node`'s tree: its directories, `.env`, `app.json`, `config.json`, `peers.json`,
/// `delegates.json` (mode 0600) and the crypto files.
fn write_node(
    node: &ManifestNode,
    nodes: &[ManifestNode],
    options: &InitOptions,
    crypto: &[(&str, Vec<u8>)],
    keys: &[GeneratedDelegate],
    aux: &[u8; 32],
) -> Result<()> {
    let config = node.dir.join("config");
    for dir in ["data", "cache", "log", "temp", "config/crypto"] {
        let dir = node.dir.join(dir);
        fs::create_dir_all(&dir).map_err(io(&dir))?;
    }
    for (name, bytes) in crypto {
        write(&config.join("crypto").join(name), bytes)?;
    }
    let (address, level) = (node.address, &options.log_level);
    let reach = options.min_network_reach;
    let env = format!(
        "# A devnet node, written by `heartwood devnet init`. Keep comments on their own lines.\n\
         CORE_LOG_LEVEL={level}\nCORE_LOG_LEVEL_FILE={level}\n\n\
         CORE_P2P_HOST={address}\nCORE_P2P_PORT={P2P_PORT}\nCORE_P2P_MIN_NETWORK_REACH={reach}\n\n\
         CORE_API_HOST={address}\nCORE_API_PORT={API_PORT}\n\n\
         CORE_WEBHOOKS_HOST={address}\nCORE_WEBHOOKS_PORT={WEBHOOKS_PORT}\n"
    );
    write(&config.join(".env"), env.as_bytes())?;
    let (own, mut access) = (node.p2p(), vec![address.to_string()]);
    for other in nodes.iter().filter(|n| n.forger_hosts.contains(&own)) {
        let forger = other.forger_address.map(|forger| forger.to_string());
        access.extend(forger.filter(|forger| !access.contains(forger)));
    }
    let plugin = |name: &str| json!({ "package": format!("@solar-network/{name}") });
    let with = |name: &str, options: Value| {
        let package = format!("@solar-network/{name}");
        json!({ "package": package, "options": options })
    };
    let devnet = json!({ "addressing": "alias", "address": address.to_string() });
    let p2p = with("p2p", json!({ "remoteAccess": access, "devnet": devnet }));
    let plugins = |names: &str| names.split(' ').map(plugin).collect::<Vec<_>>();
    let relay = [
        plugins(RELAY_PLUGINS.0),
        vec![p2p],
        plugins(RELAY_PLUGINS.1),
    ]
    .concat();
    let host = |h: &SocketAddr| json!({ "hostname": h.ip().to_string(), "port": h.port() });
    let hosts: Vec<Value> = node.forger_hosts.iter().map(host).collect();
    let local = node.forger_address.unwrap_or(address).to_string();
    let forger = json!({ "hosts": hosts, "localAddress": local, "aux": hex::encode(aux) });
    // The reference's schema requires the three profiles, the `core` one too.
    let app = json!({ "core": { "plugins": [] }, "relay": { "plugins": relay }, "forger": {
        "plugins": [plugin("logger"), plugin("transactions"), with("forger", forger)] } });
    let save = |name: &str, value: Value| write(&config.join(name), pretty(&value).as_bytes());
    let token = format!("node{}", node.index);
    let peer = |n: &ManifestNode| json!({ "ip": n.address.to_string(), "port": P2P_PORT });
    let one = nodes.len() == 1;
    let others: Vec<Value> = nodes
        .iter()
        .filter(|n| n.index != node.index || one)
        .map(peer)
        .collect();
    save("app.json", app)?;
    save("config.json", json!({ "token": token, "network": NETWORK }))?;
    save("peers.json", json!({ "list": others }))?;
    let keys: Vec<&str> = keys.iter().map(|key| key.secret_key.as_str()).collect();
    let list = Zeroizing::new(keys.join("\",\n        \""));
    let list = match keys.is_empty() {
        true => Zeroizing::new(String::new()),
        false => Zeroizing::new(format!("\n        \"{}\"\n    ", list.as_str())),
    };
    let text = Zeroizing::new(format!("{{\n    \"keys\": [{}]\n}}\n", list.as_str()));
    write_private(&config.join("delegates.json"), text.as_bytes())
}

/// Bind each of `addresses` and let it go at once; the first that cannot be bound is the error.
fn test_bind(addresses: impl IntoIterator<Item = SocketAddr>) -> Result<()> {
    for address in addresses {
        if let Err(error) = TcpListener::bind(address) {
            let message = error.to_string();
            return Err(DevnetError::Address { address, message });
        }
    }
    Ok(())
}

/// The epoch `delay` seconds after the whole second of `now_ms`, as the milestones write it.
fn epoch_after(now_ms: i64, delay: u32) -> Result<String> {
    let seconds = now_ms.div_euclid(1_000).checked_add(i64::from(delay));
    let time = seconds.and_then(|s| UtcDateTime::from_unix_ms(s.checked_mul(1_000)?));
    let time = time.ok_or_else(|| guard(format!("the time {now_ms} ms gives no epoch")))?;
    Ok(time.to_iso_string())
}

fn guard(text: String) -> DevnetError {
    DevnetError::Guard(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nodes_and_forgers_have_their_loopback_addresses() {
        let address = |text: &str| Some(text.parse::<IpAddr>().unwrap());
        assert_eq!(node_address(DEFAULT_PREFIX, 1), address("127.0.0.11"));
        assert_eq!(forger_address([127, 2, 0], 1), address("127.2.0.111"));
        assert_eq!(node_address(DEFAULT_PREFIX, 246), None);
    }

    #[test]
    fn a_failover_reads_and_writes_its_text() {
        let failover: Failover = "1:1,6,7".parse().unwrap();
        assert_eq!(
            failover,
            Failover {
                node: 1,
                hosts: vec![1, 6, 7]
            }
        );
        assert_eq!(failover.to_string(), "1:1,6,7");
        assert!("1".parse::<Failover>().is_err());
        assert!("1:a".parse::<Failover>().is_err());
    }

    #[test]
    fn the_epoch_is_a_whole_second_after_the_delay() {
        let epoch = epoch_after(1_767_225_599_900, 30).unwrap();
        assert_eq!(epoch, "2026-01-01T00:00:29.000Z");
    }
}
