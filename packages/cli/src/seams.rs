//! The instance, the seams between the CLI's parts, and the types they share. Each trait is
//! implemented by one part and called by another, so that each part can be tested against fakes
//! of its neighbours.
//!
//! | Trait | Implemented by | Called by |
//! |---|---|---|
//! | [`ProcessOps`] | [`crate::process::Supervisors`] (a recording fake in tests) | the devnet library |
//! | [`NodeApi`] | [`crate::http::LoopbackApi`] (a fake in tests) | the devnet library |
//! | [`ChainGenerator`] | [`crate::network::NetworkGenerator`] (a fake in tests) | `devnet init` |

use std::ffi::OsString;
use std::fmt;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::str::FromStr;
use std::time::Duration;

use heartwood_kernel::Clock;
use heartwood_kernel::paths::Paths;
use zeroize::Zeroizing;

use crate::errors::{HttpError, NetworkError, ProcessError};

/// The environment a node process runs with, as name and value pairs.
pub type Env = Vec<(OsString, OsString)>;

/// A node process's role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Role {
    /// The relay: the node's chain, pool, peer-to-peer network and API.
    Relay,
    /// The forger: the delegates' keys, driven by its relays.
    Forger,
}

impl Role {
    /// Every role, relays first, as a devnet starts them.
    pub const ALL: [Role; 2] = [Role::Relay, Role::Forger];

    /// The role's name in commands and file names.
    pub const fn name(self) -> &'static str {
        match self {
            Role::Relay => "relay",
            Role::Forger => "forger",
        }
    }
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl FromStr for Role {
    type Err = String;

    fn from_str(text: &str) -> Result<Role, String> {
        Role::ALL
            .into_iter()
            .find(|role| role.name() == text)
            .ok_or_else(|| format!("unknown role {text:?}: relay or forger"))
    }
}

/// One role of one node: its token, network and role, and its five directories.
///
/// The files of an instance: under `<data>/run` (mode 0700) the role's lock `<role>.lock`, pid
/// `<role>.pid`, supervisor lock `<role>.supervisor.lock` and supervisor state
/// `<role>.state.json`; under `<log>` the node's log `<name>-current.log` and the supervisor's
/// captures `<name>-out.log` and `<name>-error.log`, where `<name>` is [`Instance::name`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Instance {
    /// The token, such as `solar` or `node1`.
    pub token: String,
    /// The network, such as `testnet` or `devnet`.
    pub network: String,
    /// The role.
    pub role: Role,
    /// The config, data, cache, log and temp directories.
    pub paths: Paths,
}

impl Instance {
    /// The instance's name: `<token>-<network>-<role>`.
    pub fn name(&self) -> String {
        format!("{}-{}-{}", self.token, self.network, self.role)
    }

    /// The directory of the role's lock, pid and supervisor files: `<data>/run`.
    pub fn run_dir(&self) -> PathBuf {
        self.paths.data.join("run")
    }

    /// The node process's lock, held while it runs.
    pub fn lock_path(&self) -> PathBuf {
        self.run_dir().join(format!("{}.lock", self.role))
    }

    /// The node process's pid file.
    pub fn pid_path(&self) -> PathBuf {
        self.run_dir().join(format!("{}.pid", self.role))
    }

    /// The supervisor's lock, held while it runs.
    pub fn supervisor_lock_path(&self) -> PathBuf {
        self.run_dir()
            .join(format!("{}.supervisor.lock", self.role))
    }

    /// The supervisor's state file.
    pub fn state_path(&self) -> PathBuf {
        self.run_dir().join(format!("{}.state.json", self.role))
    }

    /// The node's own log file.
    pub fn log_path(&self) -> PathBuf {
        self.paths.log.join(format!("{}-current.log", self.name()))
    }

    /// The capture of the node's standard output.
    pub fn out_log_path(&self) -> PathBuf {
        self.paths.log.join(format!("{}-out.log", self.name()))
    }

    /// The capture of the node's standard error.
    pub fn error_log_path(&self) -> PathBuf {
        self.paths.log.join(format!("{}-error.log", self.name()))
    }
}

/// A supervised role's status, as `status` prints it and the supervisor's state file holds it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SupervisedStatus {
    /// The supervisor is starting the node.
    Launching,
    /// The node runs under its supervisor.
    Online,
    /// The node runs without a supervisor (`<role> run` in a terminal).
    OnlineUnsupervised,
    /// A stop was asked for and the node has not exited yet.
    Stopping,
    /// Nothing runs.
    Stopped,
    /// The node exited within 1 s five times in a row, and the supervisor gave up.
    Errored,
}

impl SupervisedStatus {
    /// The status's name in the table and the state file.
    pub const fn name(self) -> &'static str {
        match self {
            SupervisedStatus::Launching => "launching",
            SupervisedStatus::Online => "online",
            SupervisedStatus::OnlineUnsupervised => "online (unsupervised)",
            SupervisedStatus::Stopping => "stopping",
            SupervisedStatus::Stopped => "stopped",
            SupervisedStatus::Errored => "errored",
        }
    }

    /// Whether a node process runs.
    pub const fn is_running(self) -> bool {
        matches!(
            self,
            SupervisedStatus::Launching
                | SupervisedStatus::Online
                | SupervisedStatus::OnlineUnsupervised
                | SupervisedStatus::Stopping
        )
    }
}

/// One row of the process table.
#[derive(Debug, Clone, PartialEq)]
pub struct StatusRow {
    /// The node process's pid, while it runs.
    pub pid: Option<u32>,
    /// The instance's name.
    pub name: String,
    /// The node's version.
    pub version: String,
    /// The status.
    pub status: SupervisedStatus,
    /// How long the node has run, while it runs.
    pub uptime: Option<Duration>,
    /// How often the supervisor restarted it.
    pub restarts: u32,
    /// Its CPU use, in percent of one core, while it runs.
    pub cpu_percent: Option<f64>,
    /// Its resident memory, in bytes, while it runs.
    pub rss_bytes: Option<u64>,
}

/// The process operations on one role of one node, as the role commands run them.
pub trait ProcessOps {
    /// Start the role under its supervisor, with `env` as the node's environment; refused when it
    /// runs.
    fn start(&self, instance: &Instance, env: &Env) -> Result<(), ProcessError>;

    /// Stop the role gracefully and wait until it has exited; refused when it does not run.
    fn stop(&self, instance: &Instance) -> Result<(), ProcessError>;

    /// Kill the role's supervisor and node with SIGKILL, as a crash does, wait for them to exit,
    /// then record the stop.
    fn kill(&self, instance: &Instance) -> Result<(), ProcessError>;

    /// The role's row of the process table.
    fn status(&self, instance: &Instance) -> Result<StatusRow, ProcessError>;
}

/// `GET /api/node/status`: the relay's view of its sync.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiNodeStatus {
    /// Whether the relay considers itself synced.
    pub synced: bool,
    /// The relay's height.
    pub now: u32,
    /// How many blocks it is behind the network.
    pub blocks_count: i64,
}

/// A block as `GET /api/blocks/last` or `GET /api/blocks/{height}` gives it: what the devnet
/// compares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiBlock {
    /// The block's id.
    pub id: String,
    /// The block's height.
    pub height: u32,
}

/// A peer as `GET /api/peers` lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiPeer {
    /// The peer's address.
    pub ip: String,
    /// The peer's peer-to-peer port.
    pub port: u16,
    /// The peer's height, when it reported one.
    pub height: Option<u32>,
}

/// The reads of a relay's API that the devnet makes, on its loopback API address.
pub trait NodeApi {
    /// `GET /api/node/status`.
    fn node_status(&self, api: SocketAddr) -> Result<ApiNodeStatus, HttpError>;

    /// `GET /api/blocks/last`.
    fn last_block(&self, api: SocketAddr) -> Result<ApiBlock, HttpError>;

    /// `GET /api/blocks/{height}`; `None` when the relay has no block at that height.
    fn block_at(&self, api: SocketAddr, height: u32) -> Result<Option<ApiBlock>, HttpError>;

    /// `GET /api/peers`.
    fn peers(&self, api: SocketAddr) -> Result<Vec<ApiPeer>, HttpError>;
}

/// The chain profile `network generate` builds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Profile {
    /// The devnet profile built into the CLI: the chain definition the genesis vectors record.
    Devnet,
    /// A profile file.
    File(PathBuf),
}

/// What `network generate` builds and where it writes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenerateOptions {
    /// The network's name.
    pub network: String,
    /// The key-derivation seed.
    pub seed: String,
    /// The epoch, a whole second such as `2026-01-01T00:00:00.000Z`; the clock's current second
    /// when absent.
    pub epoch: Option<String>,
    /// The chain profile.
    pub profile: Profile,
    /// The address byte, when it replaces the profile's.
    pub pub_key_hash: Option<u8>,
    /// The token's name, when it replaces the profile's.
    pub token: Option<String>,
    /// The token's symbol, when it replaces the profile's.
    pub symbol: Option<String>,
    /// The explorer's URL, when it replaces the profile's.
    pub explorer: Option<String>,
    /// The output directory.
    pub out: PathBuf,
    /// Write the chain definition and keys only, not the single-node instance files.
    pub chain_only: bool,
    /// Overwrite the files of a directory that is not empty.
    pub force: bool,
}

/// A genesis delegate of a generated chain, with its key.
pub struct GeneratedDelegate {
    /// The delegate's username.
    pub username: String,
    /// The delegate's public key, in hex.
    pub public_key: String,
    /// The delegate's secret key, in hex, wiped when dropped.
    pub secret_key: Zeroizing<String>,
}

impl fmt::Debug for GeneratedDelegate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GeneratedDelegate")
            .field("username", &self.username)
            .field("public_key", &self.public_key)
            .finish_non_exhaustive()
    }
}

/// What `network generate` wrote.
#[derive(Debug)]
pub struct Generated {
    /// The output directory.
    pub out: PathBuf,
    /// The network's nethash, in hex.
    pub nethash: String,
    /// The genesis block's id.
    pub genesis_id: String,
    /// The epoch the chain was built with.
    pub epoch: String,
    /// The genesis delegates, in the genesis block's order.
    pub delegates: Vec<GeneratedDelegate>,
    /// The forger aux derived from the seed, as the devnet generator derives it.
    pub forger_aux: [u8; 32],
    /// Every file written, relative to `out`, in the order written.
    pub files: Vec<PathBuf>,
}

/// Builds and writes a chain definition, as `devnet init` has one written into its `genesis/`.
pub trait ChainGenerator {
    /// Write the chain of `options`, the epoch from `clock` when the options give none.
    fn generate(
        &self,
        options: &GenerateOptions,
        clock: &dyn Clock,
    ) -> Result<Generated, NetworkError>;
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    #[test]
    fn an_instance_names_its_files_after_its_token_network_and_role() {
        let root = Path::new("/nodes/node1");
        let instance = Instance {
            token: "node1".into(),
            network: "devnet".into(),
            role: Role::Forger,
            paths: Paths {
                config: root.join("config"),
                data: root.join("data"),
                cache: root.join("cache"),
                log: root.join("log"),
                temp: root.join("temp"),
            },
        };
        assert_eq!(instance.name(), "node1-devnet-forger");
        assert_eq!(instance.lock_path(), root.join("data/run/forger.lock"));
        assert_eq!(instance.pid_path(), root.join("data/run/forger.pid"));
        assert_eq!(
            instance.supervisor_lock_path(),
            root.join("data/run/forger.supervisor.lock")
        );
        assert_eq!(
            instance.state_path(),
            root.join("data/run/forger.state.json")
        );
        assert_eq!(
            instance.log_path(),
            root.join("log/node1-devnet-forger-current.log")
        );
        assert_eq!(
            instance.out_log_path(),
            root.join("log/node1-devnet-forger-out.log")
        );
        assert_eq!(
            instance.error_log_path(),
            root.join("log/node1-devnet-forger-error.log")
        );
    }

    #[test]
    fn roles_and_statuses_have_their_names() {
        assert_eq!("relay".parse::<Role>(), Ok(Role::Relay));
        assert_eq!("forger".parse::<Role>(), Ok(Role::Forger));
        assert!("core".parse::<Role>().is_err());
        assert!(SupervisedStatus::Stopping.is_running());
        assert!(!SupervisedStatus::Errored.is_running());
        assert_eq!(
            SupervisedStatus::OnlineUnsupervised.name(),
            "online (unsupervised)"
        );
    }
}
