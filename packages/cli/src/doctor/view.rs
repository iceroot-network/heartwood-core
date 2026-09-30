//! The parsed view: the typed facts the checks read, and [`Gathered`], the real [`View`] over a
//! [`World`].
//!
//! A check reads only a `&dyn View` and its options, never the world. Each source is read at
//! most once a run, lazily, the first time a check asks, through the parsers of
//! [`crate::doctor::sources`]; a source that cannot be read gives an [`Unknown`] with its
//! [`Source`] and [`Reason`], which the check reports as `unknown`. [`Budget`] holds the run's
//! 5 s deadline, its 12 API requests and its 32 MiB of log: once a limit is spent, the sources
//! not yet read give [`Reason::RunBudget`], [`Reason::ApiBudget`] or a capped log.
//!
//! The unknown reasons each source can give:
//!
//! | Source | Reasons |
//! |---|---|
//! | [`Source::Process`] | `Denied` (another user's run directory), `Unreadable`, `NotBuilt` |
//! | [`Source::Api`] | `ApiOff`, `ApiNotLoopback`, `NotRunning`, `TimedOut`, `RateLimited`, `ApiBudget`, `Unreadable`, `Unparsed`, `RunBudget` |
//! | [`Source::Store`] | `NotRunning`, `TimedOut`, `Unreadable`, `KeysDirUnsafe`, `LoaderRefused` |
//! | [`Source::RelayLog`], [`Source::ForgerLog`] | `Missing`, `Denied`, `Unreadable`, `LogLevel`, `RunBudget` |
//! | [`Source::ErrorCapture`] | `Missing`, `Denied`, `Unreadable` |
//! | [`Source::SigningRecord`] | `Denied`, `TooLarge`, `Unparsed`, `KeyFile` |
//! | [`Source::Config`] | `KeysDirUnsafe`, `LoaderRefused`, `NotConfigured` |
//! | [`Source::Files`], [`Source::KeyFiles`] | `Denied`, `Unreadable` |
//! | [`Source::Proc`], [`Source::Sys`] | `NotLinux`, `Denied`, `Unreadable`, `Unparsed` |
//! | [`Source::Sockets`] | as `Proc`, and `NotRunning` for a role's own sockets |
//! | [`Source::Probes`] | `ProbeMissing`, `TimedOut`, `Unreadable`, `Unparsed` |
//!
//! Every source also gives `NotBuilt` while its parser is not built.

use std::cell::{Cell, OnceCell, RefCell};
use std::collections::BTreeMap;
use std::io::ErrorKind;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use crate::doctor::checks::config::loader_gate;
use crate::doctor::report::HostFacts;
use crate::doctor::seams::{
    BuildFacts, FileFacts, ForgerFacts, FsError, FsStats, NodeViewError, ProbeError, ProcFile,
    RelayFacts, RoleState, StoreWindow, SupervisorFacts, World, role_state,
};
use crate::doctor::sources::api::{
    self, ApiBlock, ApiConfiguration, ApiContext, ApiPeers, ApiStatus, ApiSyncing, ApiWallet,
};
use crate::doctor::sources::fs::{ReadableRoots, SigningRecordFacts};
use crate::doctor::sources::logs::{self, CaptureWindow, LogContext, LogFiles, LogWindow};
use crate::doctor::sources::probes::{ChronyTracking, FirewallServices, NtpState, UfwConf};
use crate::doctor::sources::procfs::{BlockDevice, MemInfo, MountRow, ProcStatus, SocketRow};
use crate::doctor::sources::{ParseError, probes, procfs};
use crate::doctor::{DoctorOptions, DoctorScope};
use crate::seams::{Role, StatusRow};

/// Where a fact comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Source {
    /// The process table and the supervisors' state files.
    Process,
    /// The relay's API.
    Api,
    /// The relay's store, read-only.
    Store,
    /// The relay's log.
    RelayLog,
    /// The forger's log.
    ForgerLog,
    /// The roles' error captures.
    ErrorCapture,
    /// The forger's signing record.
    SigningRecord,
    /// Key files: metadata only.
    KeyFiles,
    /// Other files' metadata and `statvfs`.
    Files,
    /// The node's configuration, through its own loaders.
    Config,
    /// `/proc`.
    Proc,
    /// `/sys`.
    Sys,
    /// The socket table (`/proc/net/tcp`, `tcp6`, `/proc/<pid>/fd`).
    Sockets,
    /// The fixed probes.
    Probes,
    /// This program's build.
    Build,
}

impl Source {
    /// The source's name in advice and the check documentation.
    pub const fn name(self) -> &'static str {
        match self {
            Source::Process => "the process table",
            Source::Api => "the relay's API",
            Source::Store => "the relay's store",
            Source::RelayLog => "the relay's log",
            Source::ForgerLog => "the forger's log",
            Source::ErrorCapture => "the error capture",
            Source::SigningRecord => "the signing record",
            Source::KeyFiles => "the key files",
            Source::Files => "the node's files",
            Source::Config => "the configuration",
            Source::Proc => "/proc",
            Source::Sys => "/sys",
            Source::Sockets => "the socket table",
            Source::Probes => "the system probes",
            Source::Build => "this build",
        }
    }
}

/// Why a source gave no facts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Reason {
    /// The source's parser, or the implementation behind it, is not built yet.
    NotBuilt,
    /// The platform has no `/proc` or `/sys`.
    NotLinux,
    /// The file does not exist.
    Missing,
    /// The doctor may not read it (another user's file, a refused path, a key file).
    Denied,
    /// It could not be read.
    Unreadable,
    /// It is longer than the doctor reads.
    TooLarge,
    /// It is not what the source writes.
    Unparsed,
    /// It did not answer in time.
    TimedOut,
    /// The API answered 429: its rate limit.
    RateLimited,
    /// The API is disabled.
    ApiOff,
    /// The API is bound to an address that is not loopback, so the doctor does not read it.
    ApiNotLoopback,
    /// The run's 12 API requests are spent.
    ApiBudget,
    /// The run's 5 s are spent.
    RunBudget,
    /// The role it comes from does not run.
    NotRunning,
    /// The file log level hides the lines the check needs.
    LogLevel,
    /// The probe's program is not installed.
    ProbeMissing,
    /// Others can write to the configuration directory, so no node loader runs.
    KeysDirUnsafe,
    /// The node's loader refused the configuration.
    LoaderRefused,
    /// The role is not configured on this host.
    NotConfigured,
    /// No delegate of ours is known: name them with `--delegate`.
    NoDelegates,
}

impl Reason {
    /// The reason's name in advice.
    pub const fn name(self) -> &'static str {
        match self {
            Reason::NotBuilt => "not built",
            Reason::NotLinux => "not available on this platform",
            Reason::Missing => "missing",
            Reason::Denied => "not readable by this user",
            Reason::Unreadable => "unreadable",
            Reason::TooLarge => "too large",
            Reason::Unparsed => "not in the expected form",
            Reason::TimedOut => "timed out",
            Reason::RateLimited => "rate limited (429)",
            Reason::ApiOff => "the API is disabled",
            Reason::ApiNotLoopback => "the API is not on a loopback address",
            Reason::ApiBudget => "the request budget is spent",
            Reason::RunBudget => "the time budget is spent",
            Reason::NotRunning => "the role does not run",
            Reason::LogLevel => "the log level hides the lines needed",
            Reason::ProbeMissing => "the program is not installed",
            Reason::KeysDirUnsafe => "the configuration directory is writable by others",
            Reason::LoaderRefused => "the configuration is refused",
            Reason::NotConfigured => "the role is not configured here",
            Reason::NoDelegates => "no delegate of ours is known",
        }
    }
}

/// A source that gave no facts, and why.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Unknown {
    /// The source.
    pub source: Source,
    /// Why.
    pub reason: Reason,
}

impl Unknown {
    /// The unknown of `source` for `reason`.
    pub const fn new(source: Source, reason: Reason) -> Unknown {
        Unknown { source, reason }
    }

    /// The unknown of `source` for a failed read.
    pub fn from_fs(source: Source, error: &FsError) -> Unknown {
        let reason = match error {
            FsError::Refused(_) | FsError::KeyFile => Reason::Denied,
            FsError::TooLarge { .. } => Reason::TooLarge,
            FsError::Io(ErrorKind::NotFound) => Reason::Missing,
            FsError::Io(ErrorKind::PermissionDenied) => Reason::Denied,
            FsError::Io(_) => Reason::Unreadable,
            FsError::NotBuilt => Reason::NotBuilt,
        };
        Unknown::new(source, reason)
    }

    /// The unknown of `source` for input its parser refused.
    pub fn from_parse(source: Source, error: ParseError) -> Unknown {
        let reason = match error {
            ParseError::NotBuilt => Reason::NotBuilt,
            ParseError::Malformed(_) => Reason::Unparsed,
        };
        Unknown::new(source, reason)
    }

    /// The unknown of `source` for a node view that gave nothing.
    pub fn from_node(source: Source, error: &NodeViewError) -> Unknown {
        let reason = match error {
            NodeViewError::NotBuilt => Reason::NotBuilt,
            NodeViewError::Refused { .. } => Reason::LoaderRefused,
            NodeViewError::RelayNotRunning => Reason::NotRunning,
            NodeViewError::StoreTimedOut => Reason::TimedOut,
            NodeViewError::Store => Reason::Unreadable,
        };
        Unknown::new(source, reason)
    }

    /// The unknown of a probe that gave no output.
    pub fn from_probe(error: &ProbeError) -> Unknown {
        let reason = match error {
            ProbeError::Missing => Reason::ProbeMissing,
            ProbeError::TimedOut => Reason::TimedOut,
            ProbeError::Failed(_) => Reason::Unreadable,
            ProbeError::NotBuilt => Reason::NotBuilt,
        };
        Unknown::new(Source::Probes, reason)
    }
}

/// The run's limits: its deadline, its API requests and its log bytes.
#[derive(Debug)]
pub struct Budget {
    deadline_ms: i64,
    requests_left: Cell<u32>,
    log_bytes_left: Cell<u64>,
}

impl Budget {
    /// The budget of a run that started at `start_ms` and may take `run`.
    pub fn new(start_ms: i64, run: Duration) -> Budget {
        let run_ms = i64::try_from(run.as_millis()).unwrap_or(i64::MAX);
        Budget {
            deadline_ms: start_ms.saturating_add(run_ms),
            requests_left: Cell::new(api::MAX_REQUESTS),
            log_bytes_left: Cell::new(logs::MAX_RUN),
        }
    }

    /// Whether time is left at `now_ms`; [`Reason::RunBudget`] of `source` when not.
    pub fn check(&self, source: Source, now_ms: i64) -> Result<(), Unknown> {
        match now_ms < self.deadline_ms {
            true => Ok(()),
            false => Err(Unknown::new(source, Reason::RunBudget)),
        }
    }

    /// Take one API request at `now_ms`.
    pub fn take_request(&self, now_ms: i64) -> Result<(), Unknown> {
        self.check(Source::Api, now_ms)?;
        let left = self.requests_left.get();
        let left = left
            .checked_sub(1)
            .ok_or(Unknown::new(Source::Api, Reason::ApiBudget))?;
        self.requests_left.set(left);
        Ok(())
    }

    /// The API requests taken.
    pub fn requests_taken(&self) -> u32 {
        api::MAX_REQUESTS.saturating_sub(self.requests_left.get())
    }

    /// Take up to `wanted` bytes of the run's log budget; the bytes granted (0 once spent).
    pub fn take_log_bytes(&self, wanted: u64) -> u64 {
        let granted = wanted.min(self.log_bytes_left.get());
        self.log_bytes_left
            .set(self.log_bytes_left.get().saturating_sub(granted));
        granted
    }
}

/// The time a run looks back over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Window {
    /// Its start, in Unix milliseconds.
    pub from_ms: i64,
    /// Its end: the run's start.
    pub to_ms: i64,
}

/// One of the instance's five directories.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum NodeDir {
    /// Configuration.
    Config,
    /// Data: the store, the run directory, the signing record.
    Data,
    /// Cache.
    Cache,
    /// Logs.
    Log,
    /// Temporary files.
    Temp,
}

/// A file or directory of the instance whose metadata the checks read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum NodeFile {
    /// One of the five directories.
    Dir(NodeDir),
    /// The configuration directory's parent.
    ConfigParent,
    /// The forger's key file, `<config>/delegates.json`: metadata only.
    KeyFile,
    /// `<data>/run`.
    RunDir,
    /// `<data>/forger/signing-record.json`.
    SigningRecord,
    /// `<data>/forger`.
    SigningRecordDir,
    /// `<data>/database/blockchain.db`.
    Store,
    /// `/run/systemd/timesync/synchronized`.
    TimesyncMarker,
}

impl NodeFile {
    /// The file's path in `scope`; `None` for a directory with no parent.
    pub fn path(self, scope: &DoctorScope) -> Option<PathBuf> {
        let paths = &scope.paths;
        Some(match self {
            NodeFile::Dir(NodeDir::Config) => paths.config.clone(),
            NodeFile::Dir(NodeDir::Data) => paths.data.clone(),
            NodeFile::Dir(NodeDir::Cache) => paths.cache.clone(),
            NodeFile::Dir(NodeDir::Log) => paths.log.clone(),
            NodeFile::Dir(NodeDir::Temp) => paths.temp.clone(),
            NodeFile::ConfigParent => paths.config.parent()?.to_path_buf(),
            NodeFile::KeyFile => paths.config.join("delegates.json"),
            NodeFile::RunDir => paths.data.join("run"),
            NodeFile::SigningRecord => paths.data.join("forger").join("signing-record.json"),
            NodeFile::SigningRecordDir => paths.data.join("forger"),
            NodeFile::Store => paths.data.join("database").join("blockchain.db"),
            NodeFile::TimesyncMarker => "/run/systemd/timesync/synchronized".into(),
        })
    }
}

/// Which roles this host runs for the instance: a role is present when it runs, or when its run
/// files (its lock, pid or supervisor state) exist. A key file alone makes no forger present: on a
/// host where no forger ran, it is a stray key file (`keys.stray`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RolePresence {
    /// The relay.
    pub relay: bool,
    /// The forger.
    pub forger: bool,
}

impl RolePresence {
    /// Whether `role` is present.
    pub fn has(self, role: Role) -> bool {
        if role == Role::Relay {
            self.relay
        } else {
            self.forger
        }
    }
}

/// A role's process.
#[derive(Debug, Clone, PartialEq)]
pub struct ProcessFacts {
    /// Its row of the process table.
    pub row: StatusRow,
    /// Its state.
    pub state: RoleState,
    /// Its supervisor's state file, when there is one.
    pub supervisor: Option<SupervisorFacts>,
}

/// The facts the checks read. Every method but the first four reads a source, once a run.
pub trait View {
    /// The clock at the run's start, in Unix milliseconds.
    fn now_ms(&self) -> i64;
    /// What is examined.
    fn scope(&self) -> &DoctorScope;
    /// The window.
    fn window(&self) -> Window;
    /// This program's build.
    fn build(&self) -> BuildFacts;

    /// The host's operating system, architecture, kernel, CPUs and memory.
    fn host(&self) -> HostFacts;
    /// The doctor's own user id, from `/proc/self/status`.
    fn own_uid(&self) -> Result<u32, Unknown>;
    /// The host's name, only to redact it.
    fn host_name(&self) -> Result<&str, Unknown>;
    /// `/proc/meminfo`.
    fn meminfo(&self) -> Result<&MemInfo, Unknown>;
    /// `/proc/self/mountinfo`.
    fn mounts(&self) -> Result<&[MountRow], Unknown>;
    /// The block device `major:minor`, from `/sys/dev/block`.
    fn block_device(&self, major: u32, minor: u32) -> Result<BlockDevice, Unknown>;
    /// `statvfs` of one of the instance's directories.
    fn fs_stats(&self, dir: NodeDir) -> Result<FsStats, Unknown>;
    /// The metadata of one of the instance's files; `None` when it does not exist.
    fn file(&self, file: NodeFile) -> Result<Option<FileFacts>, Unknown>;
    /// Every TCP socket, IPv4 and IPv6.
    fn sockets(&self) -> Result<&[SocketRow], Unknown>;

    /// The roles this host runs for the instance.
    fn roles_present(&self) -> Result<RolePresence, Unknown>;
    /// A role's process.
    fn process(&self, role: Role) -> Result<&ProcessFacts, Unknown>;
    /// A running role's `/proc/<pid>/status`.
    fn role_status(&self, role: Role) -> Result<&ProcStatus, Unknown>;
    /// A running role's socket inodes, from `/proc/<pid>/fd`.
    fn role_socket_inodes(&self, role: Role) -> Result<&[u64], Unknown>;

    /// The relay's configuration, through its own loader (not while the configuration directory
    /// is writable by others).
    fn relay_config(&self) -> Result<&RelayFacts, Unknown>;
    /// The forger's configuration, likewise.
    fn forger_config(&self) -> Result<&ForgerFacts, Unknown>;
    /// The version floor's verdict on `version` at `height`.
    fn floor(
        &self,
        version: &str,
        height: u32,
    ) -> Result<crate::doctor::seams::FloorFacts, Unknown>;
    /// The relay's store from `from_height` on, with the missed rows of `usernames`.
    fn store_window(&self, usernames: &[String], from_height: u32) -> Result<StoreWindow, Unknown>;
    /// The forger's signing record; `None` when there is none.
    fn signing_record(&self) -> Result<Option<&SigningRecordFacts>, Unknown>;

    /// Where the relay's API is reached.
    fn api_address(&self) -> Result<SocketAddr, Unknown>;
    /// `/api/node/status`.
    fn api_status(&self) -> Result<&ApiStatus, Unknown>;
    /// `/api/node/syncing`.
    fn api_syncing(&self) -> Result<&ApiSyncing, Unknown>;
    /// `/api/node/configuration`.
    fn api_configuration(&self) -> Result<&ApiConfiguration, Unknown>;
    /// `/api/peers`.
    fn api_peers(&self) -> Result<&ApiPeers, Unknown>;
    /// `/api/blocks/last`.
    fn api_last_block(&self) -> Result<&ApiBlock, Unknown>;
    /// `/api/blocks/{height}`.
    fn api_block(&self, height: u32) -> Result<Option<ApiBlock>, Unknown>;
    /// `/api/wallets/{publicKey}`.
    fn api_wallet(&self, public_key: &str) -> Result<Option<ApiWallet>, Unknown>;

    /// A role's log in the window.
    fn log(&self, role: Role) -> Result<&LogWindow, Unknown>;
    /// A role's error capture since its last start.
    fn error_capture(&self, role: Role) -> Result<&CaptureWindow, Unknown>;

    /// `timedatectl`.
    fn ntp(&self) -> Result<&NtpState, Unknown>;
    /// `chronyc -n tracking`.
    fn chrony(&self) -> Result<&ChronyTracking, Unknown>;
    /// `systemctl is-active ufw nftables firewalld`.
    fn firewall(&self) -> Result<&FirewallServices, Unknown>;
    /// `/etc/ufw/ufw.conf`; `None` when there is none.
    fn ufw_conf(&self) -> Result<Option<&UfwConf>, Unknown>;
}

type Slot<T> = OnceCell<Result<T, Unknown>>;
type Pair<T> = [Slot<T>; 2];
type Memo<K, T> = RefCell<BTreeMap<K, Result<T, Unknown>>>;

/// The position of `role` in [`Role::ALL`].
fn at(role: Role) -> usize {
    Role::ALL
        .iter()
        .position(|r| *r == role)
        .unwrap_or_default()
}

/// A cached result as a reference.
fn get<T>(slot: &Result<T, Unknown>) -> Result<&T, Unknown> {
    slot.as_ref().map_err(|unknown| *unknown)
}

/// The sources read so far, each once.
#[derive(Default)]
struct Cells {
    host_name: Slot<String>,
    own_uid: Slot<u32>,
    meminfo: Slot<MemInfo>,
    mounts: Slot<Vec<MountRow>>,
    sockets: Slot<Vec<SocketRow>>,
    files: Memo<NodeFile, Option<FileFacts>>,
    presence: Slot<RolePresence>,
    processes: Pair<ProcessFacts>,
    statuses: Pair<ProcStatus>,
    inodes: Pair<Vec<u64>>,
    relay: Slot<RelayFacts>,
    forger: Slot<ForgerFacts>,
    floors: Memo<(String, u32), crate::doctor::seams::FloorFacts>,
    windows: Memo<(Vec<String>, u32), StoreWindow>,
    record: Slot<Option<SigningRecordFacts>>,
    address: Slot<SocketAddr>,
    status: Slot<ApiStatus>,
    syncing: Slot<ApiSyncing>,
    configuration: Slot<ApiConfiguration>,
    peers: Slot<ApiPeers>,
    last_block: Slot<ApiBlock>,
    blocks: Memo<u32, Option<ApiBlock>>,
    wallets: Memo<String, Option<ApiWallet>>,
    logs: Pair<LogWindow>,
    captures: Pair<CaptureWindow>,
    ntp: Slot<NtpState>,
    chrony: Slot<ChronyTracking>,
    firewall: Slot<FirewallServices>,
    ufw: Slot<Option<UfwConf>>,
}

/// The real [`View`]: each source of a [`World`], read once, lazily, within the run's [`Budget`].
pub struct Gathered<'w> {
    world: &'w dyn World,
    scope: &'w DoctorScope,
    now_ms: i64,
    window: Window,
    budget: Budget,
    roots: ReadableRoots,
    cells: Cells,
}

impl<'w> Gathered<'w> {
    /// The view of `scope` over `world` for a run with `options`, starting now.
    pub fn new(world: &'w dyn World, scope: &'w DoctorScope, options: &DoctorOptions) -> Self {
        let now_ms = world.clock().now_ms();
        let back = i64::try_from(options.window.as_millis()).unwrap_or(i64::MAX);
        Gathered {
            world,
            scope,
            now_ms,
            window: Window {
                from_ms: now_ms.saturating_sub(back),
                to_ms: now_ms,
            },
            budget: Budget::new(now_ms, options.budget),
            roots: ReadableRoots::for_paths(&scope.paths),
            cells: Cells::default(),
        }
    }

    /// The run's budget.
    pub fn budget(&self) -> &Budget {
        &self.budget
    }

    /// Whether time is left for `source`.
    fn time(&self, source: Source) -> Result<(), Unknown> {
        self.budget.check(source, self.world.clock().now_ms())
    }

    /// A `/proc` or `/sys` file, parsed.
    fn proc<T>(
        &self,
        file: ProcFile,
        source: Source,
        parse: fn(&[u8]) -> Result<T, ParseError>,
    ) -> Result<T, Unknown> {
        self.time(source)?;
        let bytes = self.world.procfs().read(file);
        let bytes = bytes.map_err(|error| Unknown::from_fs(source, &error))?;
        parse(&bytes).map_err(|error| Unknown::from_parse(source, error))
    }

    /// A probe's output, parsed.
    fn probe<T>(
        &self,
        probe: crate::doctor::seams::Probe,
        parse: fn(&[u8]) -> Result<T, ParseError>,
    ) -> Result<T, Unknown> {
        self.time(Source::Probes)?;
        let output = self.world.probes().run(probe);
        let output = output.map_err(|error| Unknown::from_probe(&error))?;
        parse(&output.stdout).map_err(|error| Unknown::from_parse(Source::Probes, error))
    }

    /// The running pid of `role`.
    fn pid(&self, role: Role, source: Source) -> Result<u32, Unknown> {
        let facts = self.process(role)?;
        let running = matches!(facts.state, RoleState::Running { .. } | RoleState::Stopping);
        let pid = facts.row.pid.filter(|_| running);
        pid.ok_or(Unknown::new(source, Reason::NotRunning))
    }

    /// Whether the node's loaders may run: not while others can write the configuration
    /// directory or its parent.
    fn loaders_allowed(&self) -> Result<(), Unknown> {
        let dir = self.file(NodeFile::Dir(NodeDir::Config))?;
        let parent = self.file(NodeFile::ConfigParent)?;
        loader_gate(dir.as_ref(), parent.as_ref())
    }

    fn api_context(&self) -> Result<ApiContext<'_>, Unknown> {
        self.time(Source::Api)?;
        Ok(ApiContext {
            api: self.world.api(),
            address: self.api_address()?,
            budget: &self.budget,
            clock: self.world.clock(),
        })
    }

    fn log_context(&self, role: Role) -> LogContext<'_> {
        let config = match role == Role::Relay {
            true => self.relay_config().ok().map(|facts| facts.file_log_level),
            false => self.forger_config().ok().map(|facts| facts.file_log_level),
        };
        LogContext {
            fs: self.world.fs(),
            roots: &self.roots,
            budget: &self.budget,
            clock: self.world.clock(),
            window: self.window,
            role,
            files: LogFiles {
                dir: self.scope.paths.log.clone(),
                stem: self.scope.instance(role).name(),
            },
            file_level: config,
        }
    }
}

impl View for Gathered<'_> {
    fn now_ms(&self) -> i64 {
        self.now_ms
    }

    fn scope(&self) -> &DoctorScope {
        self.scope
    }

    fn window(&self) -> Window {
        self.window
    }

    fn build(&self) -> BuildFacts {
        self.world.node().build()
    }

    fn host(&self) -> HostFacts {
        let kernel = self.proc(ProcFile::OsRelease, Source::Proc, procfs::parse_os_release);
        let cpus = std::thread::available_parallelism().ok();
        HostFacts {
            os: std::env::consts::OS,
            arch: std::env::consts::ARCH,
            kernel: kernel.ok(),
            cpus: cpus.and_then(|n| u32::try_from(n.get()).ok()),
            memory_bytes: self.meminfo().ok().map(|info| info.total),
        }
    }

    fn own_uid(&self) -> Result<u32, Unknown> {
        let slot = self.cells.own_uid.get_or_init(|| {
            let status = self.proc(ProcFile::SelfStatus, Source::Proc, procfs::parse_status);
            status.map(|status| status.uid)
        });
        get(slot).copied()
    }

    fn host_name(&self) -> Result<&str, Unknown> {
        let slot = self.cells.host_name.get_or_init(|| {
            let parse = |bytes: &[u8]| {
                let text =
                    std::str::from_utf8(bytes).map_err(|_| ParseError::Malformed("utf-8"))?;
                Ok(text.trim().to_owned())
            };
            self.proc(ProcFile::Hostname, Source::Proc, parse)
        });
        get(slot).map(String::as_str)
    }

    fn meminfo(&self) -> Result<&MemInfo, Unknown> {
        get(self
            .cells
            .meminfo
            .get_or_init(|| self.proc(ProcFile::Meminfo, Source::Proc, procfs::parse_meminfo)))
    }

    fn mounts(&self) -> Result<&[MountRow], Unknown> {
        let slot = self.cells.mounts.get_or_init(|| {
            self.proc(
                ProcFile::SelfMountinfo,
                Source::Proc,
                procfs::parse_mountinfo,
            )
        });
        get(slot).map(Vec::as_slice)
    }

    fn block_device(&self, major: u32, minor: u32) -> Result<BlockDevice, Unknown> {
        let partition = self
            .world
            .procfs()
            .read(ProcFile::BlockPartition { major, minor });
        let partition = partition.is_ok();
        let file = match partition {
            true => ProcFile::BlockParentRotational { major, minor },
            false => ProcFile::BlockRotational { major, minor },
        };
        let rotational = match self.proc(file, Source::Sys, procfs::parse_rotational) {
            Ok(rotational) => Some(rotational),
            Err(Unknown {
                reason: Reason::Missing,
                ..
            }) => None,
            Err(unknown) => return Err(unknown),
        };
        Ok(BlockDevice {
            partition,
            rotational,
        })
    }

    fn fs_stats(&self, dir: NodeDir) -> Result<FsStats, Unknown> {
        self.time(Source::Files)?;
        let path = NodeFile::Dir(dir).path(self.scope);
        let path = path.ok_or(Unknown::new(Source::Files, Reason::Missing))?;
        let stats = self.world.fs().stats(&path);
        stats.map_err(|error| Unknown::from_fs(Source::Files, &error))
    }

    fn file(&self, file: NodeFile) -> Result<Option<FileFacts>, Unknown> {
        if let Some(known) = self.cells.files.borrow().get(&file) {
            return known.clone();
        }
        let source = match file {
            NodeFile::KeyFile => Source::KeyFiles,
            _ => Source::Files,
        };
        let facts = self.time(source).and_then(|()| {
            let Some(path) = file.path(self.scope) else {
                return Ok(None);
            };
            match self.world.fs().facts(&path) {
                Ok(facts) => Ok(Some(facts)),
                Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
                Err(error) => Err(Unknown::from_fs(source, &FsError::Io(error.kind()))),
            }
        });
        self.cells.files.borrow_mut().insert(file, facts.clone());
        facts
    }

    fn sockets(&self) -> Result<&[SocketRow], Unknown> {
        let slot = self.cells.sockets.get_or_init(|| {
            let mut rows = self.proc(ProcFile::NetTcp, Source::Sockets, |b| {
                procfs::parse_net_tcp(b, false)
            })?;
            let v6 = self.proc(ProcFile::NetTcp6, Source::Sockets, |b| {
                procfs::parse_net_tcp(b, true)
            });
            match v6 {
                Ok(more) => rows.extend(more),
                // A kernel without IPv6 has no tcp6.
                Err(Unknown {
                    reason: Reason::Missing,
                    ..
                }) => {}
                Err(unknown) => return Err(unknown),
            }
            Ok(rows)
        });
        get(slot).map(Vec::as_slice)
    }

    fn roles_present(&self) -> Result<RolePresence, Unknown> {
        let slot = self.cells.presence.get_or_init(|| {
            let run = |role: Role| -> Result<bool, Unknown> {
                let running = self
                    .process(role)
                    .map(|facts| facts.state != RoleState::Stopped);
                if running? {
                    return Ok(true);
                }
                let instance = self.scope.instance(role);
                let files = [
                    instance.lock_path(),
                    instance.pid_path(),
                    instance.state_path(),
                ];
                for path in files {
                    match self.world.fs().facts(&path) {
                        Ok(_) => return Ok(true),
                        Err(error) if error.kind() == ErrorKind::NotFound => {}
                        Err(error) => {
                            return Err(Unknown::from_fs(
                                Source::Files,
                                &FsError::Io(error.kind()),
                            ));
                        }
                    }
                }
                Ok(false)
            };
            Ok(RolePresence {
                relay: run(Role::Relay)?,
                forger: run(Role::Forger)?,
            })
        });
        get(slot).copied()
    }

    fn process(&self, role: Role) -> Result<&ProcessFacts, Unknown> {
        let slot = self.cells.processes.get(at(role)).map(|slot| {
            slot.get_or_init(|| {
                self.time(Source::Process)?;
                let instance = self.scope.instance(role);
                let unreadable = |_| Unknown::new(Source::Process, Reason::Unreadable);
                let row = self
                    .world
                    .processes()
                    .status(&instance)
                    .map_err(unreadable)?;
                let supervisor = self
                    .world
                    .processes()
                    .supervisor(&instance)
                    .map_err(unreadable)?;
                Ok(ProcessFacts {
                    state: role_state(row.status),
                    row,
                    supervisor,
                })
            })
        });
        slot.map_or(Err(Unknown::new(Source::Process, Reason::Unreadable)), get)
    }

    fn role_status(&self, role: Role) -> Result<&ProcStatus, Unknown> {
        let slot = self.cells.statuses.get(at(role)).map(|slot| {
            slot.get_or_init(|| {
                let pid = self.pid(role, Source::Proc)?;
                self.proc(ProcFile::PidStatus(pid), Source::Proc, procfs::parse_status)
            })
        });
        slot.map_or(Err(Unknown::new(Source::Proc, Reason::Unreadable)), get)
    }

    fn role_socket_inodes(&self, role: Role) -> Result<&[u64], Unknown> {
        let slot = self.cells.inodes.get(at(role)).map(|slot| {
            slot.get_or_init(|| {
                let pid = self.pid(role, Source::Sockets)?;
                self.time(Source::Sockets)?;
                let targets = self.world.procfs().fd_targets(pid);
                let targets = targets.map_err(|error| Unknown::from_fs(Source::Sockets, &error))?;
                Ok(procfs::socket_inodes(&targets))
            })
        });
        slot.map_or(
            Err(Unknown::new(Source::Sockets, Reason::Unreadable)),
            |slot| get(slot).map(Vec::as_slice),
        )
    }

    fn relay_config(&self) -> Result<&RelayFacts, Unknown> {
        get(self.cells.relay.get_or_init(|| {
            self.time(Source::Config)?;
            self.loaders_allowed()?;
            let relay = self.world.node().relay(&self.scope.instance(Role::Relay));
            relay.map_err(|error| Unknown::from_node(Source::Config, &error))
        }))
    }

    fn forger_config(&self) -> Result<&ForgerFacts, Unknown> {
        get(self.cells.forger.get_or_init(|| {
            self.time(Source::Config)?;
            self.loaders_allowed()?;
            let forger = self.world.node().forger(&self.scope.instance(Role::Forger));
            forger.map_err(|error| Unknown::from_node(Source::Config, &error))
        }))
    }

    fn floor(
        &self,
        version: &str,
        height: u32,
    ) -> Result<crate::doctor::seams::FloorFacts, Unknown> {
        let key = (version.to_owned(), height);
        if let Some(known) = self.cells.floors.borrow().get(&key) {
            return known.clone();
        }
        let floor = self.time(Source::Config).and_then(|()| {
            self.loaders_allowed()?;
            let relay = self.scope.instance(Role::Relay);
            let floor = self.world.node().floor(&relay, version, height);
            floor.map_err(|error| Unknown::from_node(Source::Config, &error))
        });
        self.cells.floors.borrow_mut().insert(key, floor.clone());
        floor
    }

    fn store_window(&self, usernames: &[String], from_height: u32) -> Result<StoreWindow, Unknown> {
        let key = (usernames.to_vec(), from_height);
        if let Some(known) = self.cells.windows.borrow().get(&key) {
            return known.clone();
        }
        let window = self.time(Source::Store).and_then(|()| {
            self.loaders_allowed()?;
            let running = self.process(Role::Relay)?.state;
            if !matches!(running, RoleState::Running { .. }) {
                return Err(Unknown::new(Source::Store, Reason::NotRunning));
            }
            let relay = self.scope.instance(Role::Relay);
            let window = self
                .world
                .node()
                .store_window(&relay, usernames, from_height);
            window.map_err(|error| Unknown::from_node(Source::Store, &error))
        });
        self.cells.windows.borrow_mut().insert(key, window.clone());
        window
    }

    fn signing_record(&self) -> Result<Option<&SigningRecordFacts>, Unknown> {
        let slot = self.cells.record.get_or_init(|| {
            self.time(Source::SigningRecord)?;
            let Some(path) = NodeFile::SigningRecord.path(self.scope) else {
                return Ok(None);
            };
            let readable = crate::doctor::seams::Readable::new(&path, &self.roots);
            let readable = readable.map_err(|refused| {
                Unknown::from_fs(Source::SigningRecord, &FsError::Refused(refused))
            })?;
            let max = crate::doctor::sources::fs::MAX_SIGNING_RECORD;
            match self.world.fs().read(&readable, max) {
                Err(FsError::Io(ErrorKind::NotFound)) => Ok(None),
                Err(error) => Err(Unknown::from_fs(Source::SigningRecord, &error)),
                Ok(bytes) => crate::doctor::sources::fs::parse_signing_record(&bytes)
                    .map(Some)
                    .map_err(|error| Unknown::from_parse(Source::SigningRecord, error)),
            }
        });
        get(slot).map(Option::as_ref)
    }

    fn api_address(&self) -> Result<SocketAddr, Unknown> {
        get(self
            .cells
            .address
            .get_or_init(|| api::api_address(self.relay_config()?)))
        .copied()
    }

    fn api_status(&self) -> Result<&ApiStatus, Unknown> {
        get(self
            .cells
            .status
            .get_or_init(|| api::read_status(&self.api_context()?)))
    }

    fn api_syncing(&self) -> Result<&ApiSyncing, Unknown> {
        get(self
            .cells
            .syncing
            .get_or_init(|| api::read_syncing(&self.api_context()?)))
    }

    fn api_configuration(&self) -> Result<&ApiConfiguration, Unknown> {
        get(self
            .cells
            .configuration
            .get_or_init(|| api::read_configuration(&self.api_context()?)))
    }

    fn api_peers(&self) -> Result<&ApiPeers, Unknown> {
        get(self
            .cells
            .peers
            .get_or_init(|| api::read_peers(&self.api_context()?)))
    }

    fn api_last_block(&self) -> Result<&ApiBlock, Unknown> {
        get(self
            .cells
            .last_block
            .get_or_init(|| api::read_last_block(&self.api_context()?)))
    }

    fn api_block(&self, height: u32) -> Result<Option<ApiBlock>, Unknown> {
        if let Some(known) = self.cells.blocks.borrow().get(&height) {
            return known.clone();
        }
        let block = self
            .api_context()
            .and_then(|context| api::read_block(&context, height));
        self.cells.blocks.borrow_mut().insert(height, block.clone());
        block
    }

    fn api_wallet(&self, public_key: &str) -> Result<Option<ApiWallet>, Unknown> {
        if let Some(known) = self.cells.wallets.borrow().get(public_key) {
            return known.clone();
        }
        let wallet = self
            .api_context()
            .and_then(|context| api::read_wallet(&context, public_key));
        self.cells
            .wallets
            .borrow_mut()
            .insert(public_key.to_owned(), wallet.clone());
        wallet
    }

    fn log(&self, role: Role) -> Result<&LogWindow, Unknown> {
        let source = logs::source_of(role);
        let slot = self.cells.logs.get(at(role)).map(|slot| {
            slot.get_or_init(|| {
                self.time(source)?;
                logs::read_log(&self.log_context(role))
            })
        });
        slot.map_or(Err(Unknown::new(source, Reason::Unreadable)), get)
    }

    fn error_capture(&self, role: Role) -> Result<&CaptureWindow, Unknown> {
        let slot = self.cells.captures.get(at(role)).map(|slot| {
            slot.get_or_init(|| {
                self.time(Source::ErrorCapture)?;
                let since = self.process(role).ok();
                let since = since.and_then(|facts| facts.supervisor.as_ref()?.started_at_ms);
                logs::read_capture(&self.log_context(role), since)
            })
        });
        slot.map_or(
            Err(Unknown::new(Source::ErrorCapture, Reason::Unreadable)),
            get,
        )
    }

    fn ntp(&self) -> Result<&NtpState, Unknown> {
        get(self.cells.ntp.get_or_init(|| {
            self.probe(
                crate::doctor::seams::Probe::Timedatectl,
                probes::parse_timedatectl,
            )
        }))
    }

    fn chrony(&self) -> Result<&ChronyTracking, Unknown> {
        get(self.cells.chrony.get_or_init(|| {
            self.probe(
                crate::doctor::seams::Probe::ChronyTracking,
                probes::parse_chrony_tracking,
            )
        }))
    }

    fn firewall(&self) -> Result<&FirewallServices, Unknown> {
        get(self.cells.firewall.get_or_init(|| {
            self.probe(
                crate::doctor::seams::Probe::FirewallServices,
                probes::parse_is_active,
            )
        }))
    }

    fn ufw_conf(&self) -> Result<Option<&UfwConf>, Unknown> {
        let slot = self.cells.ufw.get_or_init(|| {
            self.time(Source::Files)?;
            let path = std::path::Path::new(crate::doctor::sources::fs::READABLE_FILES[0]);
            let readable = crate::doctor::seams::Readable::new(path, &self.roots);
            let readable = readable
                .map_err(|refused| Unknown::from_fs(Source::Files, &FsError::Refused(refused)))?;
            match self.world.fs().read(&readable, 64 * 1024) {
                Err(FsError::Io(ErrorKind::NotFound)) => Ok(None),
                Err(error) => Err(Unknown::from_fs(Source::Files, &error)),
                Ok(bytes) => probes::parse_ufw_conf(&bytes)
                    .map(Some)
                    .map_err(|error| Unknown::from_parse(Source::Files, error)),
            }
        });
        get(slot).map(Option::as_ref)
    }
}
