//! The doctor's boundary: [`World`], everything the doctor reads, as traits that tests replace with
//! fakes, and the facts that cross them.
//!
//! | Trait | Real implementation | What it reads |
//! |---|---|---|
//! | [`DoctorFs`] | `sources::fs::HostFs` | file metadata, bounded reads of [`Readable`] files, `statvfs` |
//! | [`DoctorApi`] | [`crate::doctor::HostApi`] | an HTTP GET on a loopback address ([`crate::http::get`]) |
//! | [`NodeView`] | `CoreNodeView` in `heartwood-core` | the node's own loaders, version floor and store window |
//! | [`ProcView`] | `sources::procfs::HostProc` | the fixed files of `/proc` and `/sys` that [`ProcFile`] names |
//! | [`Probes`] | `sources::probes::HostProbes` | the three fixed programs of [`Probe`] |
//! | [`ProcessView`] | [`crate::doctor::HostProcesses`] | [`crate::process::status`] and the supervisor's state file |
//! | [`Clock`] | [`heartwood_kernel::SystemClock`] | the time |
//!
//! Nothing here can write, signal, lock beyond the momentary probes of `status`, or connect
//! anywhere but a loopback address. [`ProcessView`] is deliberately not [`crate::ProcessOps`],
//! whose start, stop and kill the doctor must never be able to call. [`KeyFile`] has no method
//! that yields bytes, and [`DoctorFs::read`] and [`DoctorFs::tail`] take only a [`Readable`].
//!
//! [`subject_of`] and [`role_state`] are the only places in the doctor that match on
//! [`Role`] and [`SupervisedStatus`]; their matches have no wildcard, so a new role or status
//! fails to compile here and nowhere else.

use std::ffi::OsString;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub use heartwood_kernel::Clock;
use heartwood_kernel::log::Level;

pub use crate::doctor::sources::fs::{
    KEY_FILE_NAMES, KeyFile, Readable, ReadableRoots, RefusedPath,
};
use crate::errors::{HttpError, ProcessError};
use crate::http::Response;
use crate::seams::{Instance, Role, StatusRow, SupervisedStatus};

/// Everything the doctor reads.
pub trait World {
    /// Files, through the key-file rules.
    fn fs(&self) -> &dyn DoctorFs;
    /// The relay's API on a loopback address.
    fn api(&self) -> &dyn DoctorApi;
    /// The node's own view of its configuration and store.
    fn node(&self) -> &dyn NodeView;
    /// `/proc` and `/sys`.
    fn procfs(&self) -> &dyn ProcView;
    /// The fixed probes.
    fn probes(&self) -> &dyn Probes;
    /// The process table: status only.
    fn processes(&self) -> &dyn ProcessView;
    /// The clock.
    fn clock(&self) -> &dyn Clock;
}

/// What a file is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FileKind {
    /// A regular file.
    File,
    /// A directory.
    Dir,
    /// A symbolic link (only in [`FileFacts::own`], which does not follow it).
    Symlink,
    /// Anything else: a socket, a device, a pipe.
    Other,
}

/// One `lstat` or `stat` result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Meta {
    /// What the file is.
    pub kind: FileKind,
    /// The permission bits, `mode & 0o7777`.
    pub mode: u32,
    /// The user id the file belongs to (compared by number: the doctor looks up no user name).
    pub uid: u32,
    /// The group id the file belongs to.
    pub gid: u32,
    /// The size in bytes.
    pub size: u64,
    /// The device the file lives on.
    pub dev: u64,
    /// The inode number.
    pub ino: u64,
    /// The number of hard links.
    pub nlink: u64,
    /// The last modification, in Unix milliseconds.
    pub modified_ms: i64,
}

/// A file's metadata, read with `lstat`, `stat` and `readlink` only: its contents are never read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileFacts {
    /// The path asked for.
    pub path: PathBuf,
    /// `lstat` of the path: a symbolic link is [`FileKind::Symlink`] here.
    pub own: Meta,
    /// `stat` of the path when it is a symbolic link whose target exists; `None` otherwise.
    pub target: Option<Meta>,
    /// The link's target as `readlink` gives it, when the path is a symbolic link.
    pub link_target: Option<PathBuf>,
}

/// A filesystem's space and file entries, from `statvfs`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FsStats {
    /// The filesystem's size in bytes.
    pub total_bytes: u64,
    /// The bytes an unprivileged user can still use (`f_bavail`).
    pub available_bytes: u64,
    /// The filesystem's file entries (inodes).
    pub total_inodes: u64,
    /// The file entries an unprivileged user can still use (`f_favail`).
    pub available_inodes: u64,
}

/// The end of a file, as [`DoctorFs::tail`] reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tail {
    /// The last bytes of the file, at most the maximum asked for.
    pub bytes: Vec<u8>,
    /// The offset of `bytes`' first byte in the file.
    pub start: u64,
    /// The file's length when it was read.
    pub len: u64,
}

/// Why a read through [`DoctorFs`] or [`ProcView`] failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FsError {
    /// [`Readable::new`] refused the path.
    #[error("refused: {0}")]
    Refused(RefusedPath),
    /// The opened file is a key file (the same device and inode as one), whatever its name:
    /// it was closed unread.
    #[error("the file is a key file")]
    KeyFile,
    /// The file is longer than the maximum a whole read allows.
    #[error("the file is longer than {max} bytes")]
    TooLarge {
        /// The maximum.
        max: u64,
    },
    /// The system refused or failed the read.
    #[error("{0:?}")]
    Io(io::ErrorKind),
    /// This implementation is not built yet.
    #[error("not built")]
    NotBuilt,
}

/// Files, as the doctor may read them: metadata of any path, contents only of a [`Readable`].
/// Every open is made without following a final symbolic link, and a descriptor whose device and
/// inode are a key file's is closed unread ([`FsError::KeyFile`]).
pub trait DoctorFs {
    /// `lstat`, `stat` (when a link) and `readlink` of `path`; never opens it.
    fn facts(&self, path: &Path) -> io::Result<FileFacts>;
    /// The whole of `file`, refused with [`FsError::TooLarge`] when longer than `max` bytes.
    fn read(&self, file: &Readable, max: u64) -> Result<Vec<u8>, FsError>;
    /// The last `max` bytes of `file`.
    fn tail(&self, file: &Readable, max: u64) -> Result<Tail, FsError>;
    /// The names in the directory `dir`, sorted; nothing else about them.
    fn list(&self, dir: &Readable) -> Result<Vec<OsString>, FsError>;
    /// `statvfs` of the filesystem that holds `path`.
    fn stats(&self, path: &Path) -> Result<FsStats, FsError>;
}

/// An HTTP GET to a loopback address: the doctor's only network use.
pub trait DoctorApi {
    /// `GET path` from `address`, each step within `timeout`; refused for any address that is not
    /// loopback ([`HttpError::NotLoopback`]).
    fn get(
        &self,
        address: SocketAddr,
        path: &str,
        timeout: Duration,
    ) -> Result<Response, HttpError>;
}

/// This program's build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildFacts {
    /// The node's version, `4.3.1-heartwood.<crate version>`.
    pub version: &'static str,
    /// The enabled features that change what a node does, in a fixed order (`devnet`,
    /// `difftest`).
    pub features: Vec<&'static str>,
}

/// The chain definition's numbers, from the crypto files the node loads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChainFacts {
    /// The nethash, 64 lowercase hex digits.
    pub nethash: String,
    /// The network epoch, in Unix milliseconds: store timestamps count seconds from it.
    pub epoch_ms: i64,
    /// The block time, in seconds, at the tip's milestone.
    pub block_time_s: u32,
    /// The active delegates, at the tip's milestone.
    pub active_delegates: u32,
}

/// One `remoteAccess` entry, as the relay matches it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccessEntry {
    /// A plain IP address: the only form the relay matches.
    Address(IpAddr),
    /// An address with a prefix length (`10.0.0.0/24`): the relay matches it against nothing.
    Prefix {
        /// The address part.
        address: IpAddr,
        /// The prefix length.
        bits: u8,
    },
    /// Anything else, such as a host name: the relay matches it against nothing. The text is not
    /// kept.
    Other,
}

/// Why a node loader or the store refused, as a fixed word (never text from a file).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadRefusal {
    /// Neither the flags nor `config.json` name a network.
    NoNetwork,
    /// A token or network name is not valid.
    InvalidName,
    /// A `.env` file cannot be read.
    EnvFile,
    /// `app.json` or the role's profile in it is refused.
    AppProfile,
    /// `peers.json` is refused.
    Peers,
    /// The crypto files are missing or refused.
    Crypto,
    /// A component's options are refused.
    Options,
    /// Another refusal.
    Other,
}

/// Why a [`NodeView`] call gave no facts.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum NodeViewError {
    /// The implementation is not built yet.
    #[error("not built")]
    NotBuilt,
    /// The node's loader refused its configuration; `file` is the file it names, if any.
    #[error("the configuration is refused ({kind:?})")]
    Refused {
        /// The refusal's kind.
        kind: LoadRefusal,
        /// The file the loader names.
        file: Option<PathBuf>,
    },
    /// The store is read only while the relay's lock is held, and it is not.
    #[error("the relay does not run")]
    RelayNotRunning,
    /// The store did not answer within its 1 s statement timeout.
    #[error("the store timed out")]
    StoreTimedOut,
    /// The store could not be opened or read, or its schema is not this build's.
    #[error("the store cannot be read")]
    Store,
}

/// The relay's configuration, as the relay's own loaders compute it. Values of `.env` never
/// cross this seam: variables appear by name only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayFacts {
    /// The P2P server's resolved listening address and port.
    pub p2p: SocketAddr,
    /// The API's resolved listening address and port; `None` when the API is disabled.
    pub api: Option<SocketAddr>,
    /// `remoteAccess`, entry by entry.
    pub remote_access: Vec<AccessEntry>,
    /// `minimumNetworkReach`.
    pub minimum_network_reach: u32,
    /// The file logger's level: lines less severe than it are not written.
    pub file_log_level: Level,
    /// What the relay's start would warn about, as its loader words them. They can hold option
    /// keys and variable names from the operator's own files (never values); checks carry them
    /// as untrusted text only.
    pub warnings: Vec<String>,
    /// The names of the variables the relay's `.env` files set (`[A-Z0-9_]`), never their values.
    pub env_names: Vec<String>,
    /// The chain definition.
    pub chain: ChainFacts,
    /// The store's database file.
    pub store_file: PathBuf,
}

/// The forger's configuration, as the forger's own loader computes it. The key file is never
/// loaded for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForgerFacts {
    /// The relays, in the order the forger tries them.
    pub hosts: Vec<SocketAddr>,
    /// `localAddress`, the address the forger connects from, when set.
    pub local_address: Option<IpAddr>,
    /// Whether `aux` is set (its value is never read).
    pub aux_set: bool,
    /// The file logger's level.
    pub file_log_level: Level,
    /// The key file's path: metadata only.
    pub key_file: PathBuf,
    /// The signing record's path.
    pub signing_record: PathBuf,
    /// The chain definition.
    pub chain: ChainFacts,
}

/// What the version floor says of a version at a height.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FloorFacts {
    /// The floor in force at the height refuses the version.
    pub refused_now: bool,
    /// The heights of the later milestones whose floors refuse it, in order.
    pub refused_from: Vec<u32>,
}

/// One missed slot, from the store's `missed_blocks`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissedSlot {
    /// The missed slot's first second, in seconds since the network epoch (not the Unix epoch).
    pub timestamp: u32,
    /// The height of the block forged after the missed slots.
    pub height: u32,
    /// The delegate that missed it.
    pub username: String,
}

/// One block, from the store's `blocks`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredBlock {
    /// The height.
    pub height: u32,
    /// The slot's first second, in seconds since the network epoch.
    pub timestamp: u32,
    /// The block id, 64 lowercase hex digits.
    pub id: String,
    /// The generator's public key, 66 lowercase hex digits.
    pub generator_public_key: String,
    /// The generator's username, when the store has it.
    pub username: Option<String>,
}

/// One read transaction on the relay's store: our delegates' missed rows and the blocks from a
/// height on, each at most 5,000 rows, in height order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StoreWindow {
    /// The missed slots of the named usernames.
    pub missed: Vec<MissedSlot>,
    /// The blocks.
    pub blocks: Vec<StoredBlock>,
    /// Whether a row cap cut either list.
    pub capped: bool,
}

/// The node's own code, implemented by `heartwood-core` with the node's loaders: the doctor sees
/// exactly what the node would. No method may run while the configuration directory can be
/// written by others (the view's loader gate), and none reads the key file.
pub trait NodeView {
    /// This program's build.
    fn build(&self) -> BuildFacts;
    /// The relay's configuration.
    fn relay(&self, relay: &Instance) -> Result<RelayFacts, NodeViewError>;
    /// The forger's configuration.
    fn forger(&self, forger: &Instance) -> Result<ForgerFacts, NodeViewError>;
    /// The version floor's verdict on `version` at `height`, with the node's own version gate.
    fn floor(
        &self,
        relay: &Instance,
        version: &str,
        height: u32,
    ) -> Result<FloorFacts, NodeViewError>;
    /// One read transaction on the relay's store, only while the relay's lock is held: the
    /// missed rows of `usernames` and every block from `from_height` on. Opened read-only; a
    /// stopped relay gives [`NodeViewError::RelayNotRunning`] and nothing is opened.
    fn store_window(
        &self,
        relay: &Instance,
        usernames: &[String],
        from_height: u32,
    ) -> Result<StoreWindow, NodeViewError>;
}

/// A fixed file of `/proc` or `/sys`. `Self*` is the doctor's own process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProcFile {
    /// `/proc/net/tcp`.
    NetTcp,
    /// `/proc/net/tcp6`.
    NetTcp6,
    /// `/proc/meminfo`.
    Meminfo,
    /// `/proc/self/mountinfo`.
    SelfMountinfo,
    /// `/proc/self/status`: the doctor's own user.
    SelfStatus,
    /// `/proc/sys/kernel/osrelease`.
    OsRelease,
    /// `/proc/sys/kernel/hostname`: read only to redact it.
    Hostname,
    /// `/proc/<pid>/status`.
    PidStatus(u32),
    /// `/proc/<pid>/stat`.
    PidStat(u32),
    /// `/proc/<pid>/cmdline`: read only by the MCP server's count of its own servers.
    PidCmdline(u32),
    /// `/sys/dev/block/<major>:<minor>/queue/rotational`.
    BlockRotational {
        /// The device's major number.
        major: u32,
        /// The device's minor number.
        minor: u32,
    },
    /// `/sys/dev/block/<major>:<minor>/partition`, present for a partition.
    BlockPartition {
        /// The device's major number.
        major: u32,
        /// The device's minor number.
        minor: u32,
    },
    /// `/sys/dev/block/<major>:<minor>/../queue/rotational`: a partition's parent device.
    BlockParentRotational {
        /// The partition's major number.
        major: u32,
        /// The partition's minor number.
        minor: u32,
    },
}

impl ProcFile {
    /// The file's path.
    pub fn path(self) -> PathBuf {
        match self {
            ProcFile::NetTcp => "/proc/net/tcp".into(),
            ProcFile::NetTcp6 => "/proc/net/tcp6".into(),
            ProcFile::Meminfo => "/proc/meminfo".into(),
            ProcFile::SelfMountinfo => "/proc/self/mountinfo".into(),
            ProcFile::SelfStatus => "/proc/self/status".into(),
            ProcFile::OsRelease => "/proc/sys/kernel/osrelease".into(),
            ProcFile::Hostname => "/proc/sys/kernel/hostname".into(),
            ProcFile::PidStatus(pid) => format!("/proc/{pid}/status").into(),
            ProcFile::PidStat(pid) => format!("/proc/{pid}/stat").into(),
            ProcFile::PidCmdline(pid) => format!("/proc/{pid}/cmdline").into(),
            ProcFile::BlockRotational { major, minor } => {
                format!("/sys/dev/block/{major}:{minor}/queue/rotational").into()
            }
            ProcFile::BlockPartition { major, minor } => {
                format!("/sys/dev/block/{major}:{minor}/partition").into()
            }
            ProcFile::BlockParentRotational { major, minor } => {
                format!("/sys/dev/block/{major}:{minor}/../queue/rotational").into()
            }
        }
    }
}

/// `/proc` and `/sys`, file by file: nothing else can be named.
pub trait ProcView {
    /// The contents of `file`, at most 4 MiB.
    fn read(&self, file: ProcFile) -> Result<Vec<u8>, FsError>;
    /// The targets of `/proc/<pid>/fd/*` (`readlink` only), in descriptor order.
    fn fd_targets(&self, pid: u32) -> Result<Vec<OsString>, FsError>;
    /// The pids listed in `/proc`, in order.
    fn pids(&self) -> Result<Vec<u32>, FsError>;
}

/// One of the three fixed probes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Probe {
    /// `timedatectl show --property=NTPSynchronized --property=NTP`.
    Timedatectl,
    /// `chronyc -n tracking`.
    ChronyTracking,
    /// `systemctl is-active ufw nftables firewalld`.
    FirewallServices,
}

impl Probe {
    /// Every probe.
    pub const ALL: [Probe; 3] = [
        Probe::Timedatectl,
        Probe::ChronyTracking,
        Probe::FirewallServices,
    ];

    /// The absolute paths the program is looked for at, in order; no search path is used.
    pub const fn programs(self) -> &'static [&'static str] {
        match self {
            Probe::Timedatectl => &["/usr/bin/timedatectl", "/bin/timedatectl"],
            Probe::ChronyTracking => &["/usr/bin/chronyc", "/usr/sbin/chronyc"],
            Probe::FirewallServices => &["/usr/bin/systemctl", "/bin/systemctl"],
        }
    }

    /// The fixed arguments. Nothing from a command line, a file, a log or a client is ever added.
    pub const fn args(self) -> &'static [&'static str] {
        match self {
            Probe::Timedatectl => &["show", "--property=NTPSynchronized", "--property=NTP"],
            Probe::ChronyTracking => &["-n", "tracking"],
            Probe::FirewallServices => &["is-active", "ufw", "nftables", "firewalld"],
        }
    }

    /// The whole environment a probe runs with.
    pub const ENV: [(&'static str, &'static str); 1] = [("LANG", "C")];
    /// How long a probe may run before it is killed.
    pub const TIMEOUT: Duration = Duration::from_secs(2);
    /// The most output read from a probe.
    pub const MAX_OUTPUT: usize = 64 * 1024;
}

/// A probe's result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeOutput {
    /// The exit code; `None` when a signal ended it.
    pub status: Option<i32>,
    /// Standard output, at most [`Probe::MAX_OUTPUT`] bytes.
    pub stdout: Vec<u8>,
}

/// Why a probe gave no output.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProbeError {
    /// None of the program's paths exists.
    #[error("the program is not installed")]
    Missing,
    /// It ran longer than [`Probe::TIMEOUT`] and was killed.
    #[error("the program timed out")]
    TimedOut,
    /// It could not be started or read.
    #[error("the program failed: {0:?}")]
    Failed(io::ErrorKind),
    /// This implementation is not built yet.
    #[error("not built")]
    NotBuilt,
}

/// The fixed probes.
pub trait Probes {
    /// Run `probe`.
    fn run(&self, probe: Probe) -> Result<ProbeOutput, ProbeError>;
}

/// The supervisor's state file, without the node's command line (`argv` is never read into the
/// doctor).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupervisorFacts {
    /// When the node last started, in Unix milliseconds.
    pub started_at_ms: Option<i64>,
    /// Restarts since the supervisor started.
    pub restarts: u32,
    /// Unstable runs in the first seconds.
    pub unstable: u32,
    /// The version the supervisor recorded.
    pub version: String,
}

/// The process table: status only, never start, stop or kill.
pub trait ProcessView {
    /// The role's row of the process table (`heartwood <role> status`), with the momentary
    /// shared lock probes that `status` takes.
    fn status(&self, instance: &Instance) -> Result<StatusRow, ProcessError>;
    /// The supervisor's state file; `None` when there is none.
    fn supervisor(&self, instance: &Instance) -> Result<Option<SupervisorFacts>, ProcessError>;
}

/// What a finding is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Subject {
    /// The host.
    Host,
    /// The relay.
    Relay,
    /// The forger.
    Forger,
}

impl Subject {
    /// The subject's name in the report.
    pub const fn name(self) -> &'static str {
        match self {
            Subject::Host => "host",
            Subject::Relay => "relay",
            Subject::Forger => "forger",
        }
    }
}

/// The subject of a role. One of the two functions that match on [`Role`].
pub const fn subject_of(role: Role) -> Subject {
    match role {
        Role::Relay => Subject::Relay,
        Role::Forger => Subject::Forger,
    }
}

/// A role's state, as the doctor judges it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RoleState {
    /// Its supervisor is starting it.
    Starting,
    /// It runs; `supervised` is false for `online (unsupervised)`.
    Running {
        /// Whether a supervisor keeps it running.
        supervised: bool,
    },
    /// A stop was asked for.
    Stopping,
    /// Nothing runs.
    Stopped,
    /// The supervisor gave up on it.
    Errored,
}

/// The state of a supervised status. The one function that matches on [`SupervisedStatus`].
pub const fn role_state(status: SupervisedStatus) -> RoleState {
    match status {
        SupervisedStatus::Launching => RoleState::Starting,
        SupervisedStatus::Online => RoleState::Running { supervised: true },
        SupervisedStatus::OnlineUnsupervised => RoleState::Running { supervised: false },
        SupervisedStatus::Stopping => RoleState::Stopping,
        SupervisedStatus::Stopped => RoleState::Stopped,
        SupervisedStatus::Errored => RoleState::Errored,
    }
}
