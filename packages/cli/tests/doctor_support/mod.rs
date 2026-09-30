//! Fakes of the doctor's seams, for its tests: [`FakeView`], a view whose every fact a test sets,
//! and [`FakeWorld`], a world of maps from paths, requests, pids and probes to canned answers, with
//! a manual clock and a key-file list. A fact or answer a test does not set reads as `unknown` with
//! [`Reason::NotBuilt`] (for the view) or as missing (for the world).

#![allow(dead_code, reason = "each test file uses part of the support")]

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use heartwood_cli::doctor::DoctorScope;
use heartwood_cli::doctor::report::HostFacts;
use heartwood_cli::doctor::seams::{
    BuildFacts, Clock, DoctorApi, DoctorFs, FileFacts, FloorFacts, ForgerFacts, FsError, FsStats,
    NodeView, NodeViewError, Probe, ProbeError, ProbeOutput, Probes, ProcFile, ProcView,
    ProcessView, Readable, RelayFacts, StoreWindow, SupervisorFacts, Tail, WindowStart, World,
};
use heartwood_cli::doctor::sources::api::{
    ApiBlock, ApiConfiguration, ApiPeers, ApiStatus, ApiSyncing, ApiWallet,
};
use heartwood_cli::doctor::sources::fs::SigningRecordFacts;
use heartwood_cli::doctor::sources::logs::{CaptureWindow, LogWindow};
use heartwood_cli::doctor::sources::probes::{ChronyTracking, FirewallServices, NtpState, UfwConf};
use heartwood_cli::doctor::sources::procfs::{
    BlockDevice, MemInfo, MountRow, ProcStatus, SocketRow,
};
use heartwood_cli::doctor::view::{
    NodeDir, NodeFile, ProcessFacts, Reason, RolePresence, Source, Unknown, View, Window,
};
use heartwood_cli::http::Response;
use heartwood_cli::{HttpError, Instance, ProcessError, Role, StatusRow};
use heartwood_kernel::clock::ManualClock;
use heartwood_kernel::paths::Paths;

/// The clock of every fake: 2026-10-02 14:03:11 UTC.
pub const NOW_MS: i64 = 1_790_949_791_000;

/// The scope of a node whose directories are under `root`.
pub fn scope(root: &str) -> DoctorScope {
    let root = Path::new(root);
    DoctorScope {
        token: "solar".into(),
        network: "testnet".into(),
        paths: Paths {
            config: root.join("config"),
            data: root.join("data"),
            cache: root.join("cache"),
            log: root.join("log"),
            temp: root.join("temp"),
        },
        home: Some(PathBuf::from("/users/user")),
        user: Some("user".into()),
    }
}

/// An unset fact of the fake view.
fn unset<T>(source: Source) -> Result<T, Unknown> {
    Err(Unknown::new(source, Reason::NotBuilt))
}

/// A view whose every fact a test sets; an unset one is `unknown`, not built.
pub struct FakeView {
    pub now_ms: i64,
    pub scope: DoctorScope,
    pub window: Window,
    pub build: BuildFacts,
    pub host: HostFacts,
    pub own_uid: Result<u32, Unknown>,
    pub host_name: Result<String, Unknown>,
    pub meminfo: Result<MemInfo, Unknown>,
    pub mounts: Result<Vec<MountRow>, Unknown>,
    pub block_devices: BTreeMap<(u32, u32), Result<BlockDevice, Unknown>>,
    pub fs_stats: BTreeMap<NodeDir, Result<FsStats, Unknown>>,
    pub files: BTreeMap<NodeFile, Result<Option<FileFacts>, Unknown>>,
    pub sockets: Result<Vec<SocketRow>, Unknown>,
    pub roles_present: Result<RolePresence, Unknown>,
    pub processes: BTreeMap<Role, Result<ProcessFacts, Unknown>>,
    pub role_statuses: BTreeMap<Role, Result<ProcStatus, Unknown>>,
    pub role_inodes: BTreeMap<Role, Result<Vec<u64>, Unknown>>,
    pub relay_config: Result<RelayFacts, Unknown>,
    pub forger_config: Result<ForgerFacts, Unknown>,
    pub floors: BTreeMap<(String, u32), Result<FloorFacts, Unknown>>,
    /// The store window, whatever the usernames and start; each call's arguments are recorded.
    pub store_window: Result<StoreWindow, Unknown>,
    pub store_calls: RefCell<Vec<(Vec<String>, WindowStart)>>,
    pub signing_record: Result<Option<SigningRecordFacts>, Unknown>,
    pub api_address: Result<SocketAddr, Unknown>,
    pub api_status: Result<ApiStatus, Unknown>,
    pub api_syncing: Result<ApiSyncing, Unknown>,
    pub api_configuration: Result<ApiConfiguration, Unknown>,
    pub api_peers: Result<ApiPeers, Unknown>,
    pub api_last_block: Result<ApiBlock, Unknown>,
    pub api_blocks: BTreeMap<u32, Result<Option<ApiBlock>, Unknown>>,
    pub api_wallets: BTreeMap<String, Result<Option<ApiWallet>, Unknown>>,
    pub logs: BTreeMap<Role, Result<LogWindow, Unknown>>,
    pub captures: BTreeMap<Role, Result<CaptureWindow, Unknown>>,
    pub ntp: Result<NtpState, Unknown>,
    pub chrony: Result<ChronyTracking, Unknown>,
    pub firewall: Result<FirewallServices, Unknown>,
    pub ufw_conf: Result<Option<UfwConf>, Unknown>,
}

impl FakeView {
    /// A view of a node under `/nodes/node1` at [`NOW_MS`] with a 60-minute window and every
    /// source unset.
    pub fn new() -> FakeView {
        FakeView {
            now_ms: NOW_MS,
            scope: scope("/nodes/node1"),
            window: Window {
                from_ms: NOW_MS - 3_600_000,
                to_ms: NOW_MS,
            },
            build: BuildFacts {
                version: "4.3.1-heartwood.0.1.0",
                features: vec!["devnet"],
            },
            host: HostFacts::default(),
            own_uid: unset(Source::Proc),
            host_name: unset(Source::Proc),
            meminfo: unset(Source::Proc),
            mounts: unset(Source::Proc),
            block_devices: BTreeMap::new(),
            fs_stats: BTreeMap::new(),
            files: BTreeMap::new(),
            sockets: unset(Source::Sockets),
            roles_present: unset(Source::Process),
            processes: BTreeMap::new(),
            role_statuses: BTreeMap::new(),
            role_inodes: BTreeMap::new(),
            relay_config: unset(Source::Config),
            forger_config: unset(Source::Config),
            floors: BTreeMap::new(),
            store_window: unset(Source::Store),
            store_calls: RefCell::default(),
            signing_record: unset(Source::SigningRecord),
            api_address: unset(Source::Api),
            api_status: unset(Source::Api),
            api_syncing: unset(Source::Api),
            api_configuration: unset(Source::Api),
            api_peers: unset(Source::Api),
            api_last_block: unset(Source::Api),
            api_blocks: BTreeMap::new(),
            api_wallets: BTreeMap::new(),
            logs: BTreeMap::new(),
            captures: BTreeMap::new(),
            ntp: unset(Source::Probes),
            chrony: unset(Source::Probes),
            firewall: unset(Source::Probes),
            ufw_conf: unset(Source::Files),
        }
    }
}

impl Default for FakeView {
    fn default() -> FakeView {
        FakeView::new()
    }
}

/// The reference to a set fact, or a copy of its unknown.
fn get<T>(fact: &Result<T, Unknown>) -> Result<&T, Unknown> {
    fact.as_ref().map_err(|unknown| *unknown)
}

/// The fact of `key` in `map`, or unset.
fn keyed<'a, K: Ord, T>(
    map: &'a BTreeMap<K, Result<T, Unknown>>,
    key: &K,
    source: Source,
) -> Result<&'a T, Unknown> {
    map.get(key).map_or(unset(source), get)
}

impl View for FakeView {
    fn now_ms(&self) -> i64 {
        self.now_ms
    }
    fn scope(&self) -> &DoctorScope {
        &self.scope
    }
    fn window(&self) -> Window {
        self.window
    }
    fn build(&self) -> BuildFacts {
        self.build.clone()
    }
    fn host(&self) -> HostFacts {
        self.host.clone()
    }
    fn own_uid(&self) -> Result<u32, Unknown> {
        self.own_uid
    }
    fn host_name(&self) -> Result<&str, Unknown> {
        get(&self.host_name).map(String::as_str)
    }
    fn meminfo(&self) -> Result<&MemInfo, Unknown> {
        get(&self.meminfo)
    }
    fn mounts(&self) -> Result<&[MountRow], Unknown> {
        get(&self.mounts).map(Vec::as_slice)
    }
    fn block_device(&self, major: u32, minor: u32) -> Result<BlockDevice, Unknown> {
        keyed(&self.block_devices, &(major, minor), Source::Sys).copied()
    }
    fn fs_stats(&self, dir: NodeDir) -> Result<FsStats, Unknown> {
        keyed(&self.fs_stats, &dir, Source::Files).copied()
    }
    fn file(&self, file: NodeFile) -> Result<Option<FileFacts>, Unknown> {
        keyed(&self.files, &file, Source::Files).cloned()
    }
    fn sockets(&self) -> Result<&[SocketRow], Unknown> {
        get(&self.sockets).map(Vec::as_slice)
    }
    fn roles_present(&self) -> Result<RolePresence, Unknown> {
        self.roles_present
    }
    fn process(&self, role: Role) -> Result<&ProcessFacts, Unknown> {
        keyed(&self.processes, &role, Source::Process)
    }
    fn role_status(&self, role: Role) -> Result<&ProcStatus, Unknown> {
        keyed(&self.role_statuses, &role, Source::Proc)
    }
    fn role_socket_inodes(&self, role: Role) -> Result<&[u64], Unknown> {
        keyed(&self.role_inodes, &role, Source::Sockets).map(Vec::as_slice)
    }
    fn relay_config(&self) -> Result<&RelayFacts, Unknown> {
        get(&self.relay_config)
    }
    fn forger_config(&self) -> Result<&ForgerFacts, Unknown> {
        get(&self.forger_config)
    }
    fn floor(&self, version: &str, height: u32) -> Result<FloorFacts, Unknown> {
        keyed(&self.floors, &(version.to_owned(), height), Source::Config).cloned()
    }
    fn store_window(
        &self,
        usernames: &[String],
        start: WindowStart,
    ) -> Result<StoreWindow, Unknown> {
        self.store_calls
            .borrow_mut()
            .push((usernames.to_vec(), start));
        self.store_window.clone()
    }
    fn signing_record(&self) -> Result<Option<&SigningRecordFacts>, Unknown> {
        get(&self.signing_record).map(Option::as_ref)
    }
    fn api_address(&self) -> Result<SocketAddr, Unknown> {
        self.api_address
    }
    fn api_status(&self) -> Result<&ApiStatus, Unknown> {
        get(&self.api_status)
    }
    fn api_syncing(&self) -> Result<&ApiSyncing, Unknown> {
        get(&self.api_syncing)
    }
    fn api_configuration(&self) -> Result<&ApiConfiguration, Unknown> {
        get(&self.api_configuration)
    }
    fn api_peers(&self) -> Result<&ApiPeers, Unknown> {
        get(&self.api_peers)
    }
    fn api_last_block(&self) -> Result<&ApiBlock, Unknown> {
        get(&self.api_last_block)
    }
    fn api_block(&self, height: u32) -> Result<Option<ApiBlock>, Unknown> {
        keyed(&self.api_blocks, &height, Source::Api).cloned()
    }
    fn api_wallet(&self, public_key: &str) -> Result<Option<ApiWallet>, Unknown> {
        keyed(&self.api_wallets, &public_key.to_owned(), Source::Api).cloned()
    }
    fn log(&self, role: Role) -> Result<&LogWindow, Unknown> {
        let source = heartwood_cli::doctor::sources::logs::source_of(role);
        keyed(&self.logs, &role, source)
    }
    fn error_capture(&self, role: Role) -> Result<&CaptureWindow, Unknown> {
        keyed(&self.captures, &role, Source::ErrorCapture)
    }
    fn ntp(&self) -> Result<&NtpState, Unknown> {
        get(&self.ntp)
    }
    fn chrony(&self) -> Result<&ChronyTracking, Unknown> {
        get(&self.chrony)
    }
    fn firewall(&self) -> Result<&FirewallServices, Unknown> {
        get(&self.firewall)
    }
    fn ufw_conf(&self) -> Result<Option<&UfwConf>, Unknown> {
        get(&self.ufw_conf).map(Option::as_ref)
    }
}

/// Files: metadata by path, contents by path, `statvfs` by path, and the key files' identities,
/// which every read refuses whatever the path's name.
#[derive(Default)]
pub struct FakeFs {
    pub facts: BTreeMap<PathBuf, Result<FileFacts, io::ErrorKind>>,
    pub contents: BTreeMap<PathBuf, Vec<u8>>,
    pub stats: BTreeMap<PathBuf, FsStats>,
    /// The key files' `(device, inode)`.
    pub key_files: Vec<(u64, u64)>,
    /// Every path opened, in order.
    pub opened: RefCell<Vec<PathBuf>>,
}

impl FakeFs {
    /// The contents of `file`, refused when its metadata names a key file's identity.
    fn open(&self, file: &Readable) -> Result<&Vec<u8>, FsError> {
        self.opened.borrow_mut().push(file.path().to_path_buf());
        if let Some(Ok(facts)) = self.facts.get(file.path()) {
            let own = (facts.own.dev, facts.own.ino);
            if self.key_files.contains(&own) {
                return Err(FsError::KeyFile);
            }
        }
        let contents = self.contents.get(file.path());
        contents.ok_or(FsError::Io(io::ErrorKind::NotFound))
    }
}

impl DoctorFs for FakeFs {
    fn facts(&self, path: &Path) -> io::Result<FileFacts> {
        match self.facts.get(path) {
            Some(Ok(facts)) => Ok(facts.clone()),
            Some(Err(kind)) => Err(io::Error::from(*kind)),
            None => Err(io::Error::from(io::ErrorKind::NotFound)),
        }
    }
    fn read(&self, file: &Readable, max: u64) -> Result<Vec<u8>, FsError> {
        let bytes = self.open(file)?;
        match u64::try_from(bytes.len()).unwrap_or(u64::MAX) > max {
            true => Err(FsError::TooLarge { max }),
            false => Ok(bytes.clone()),
        }
    }
    fn tail(&self, file: &Readable, max: u64) -> Result<Tail, FsError> {
        let bytes = self.open(file)?;
        let len = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        let start = len.saturating_sub(max);
        let from = usize::try_from(start).unwrap_or(usize::MAX);
        Ok(Tail {
            bytes: bytes.get(from..).unwrap_or_default().to_vec(),
            start,
            len,
        })
    }
    fn list(&self, dir: &Readable) -> Result<Vec<OsString>, FsError> {
        let names = self.facts.keys().chain(self.contents.keys());
        let mut names: Vec<OsString> = names
            .filter(|path| path.parent() == Some(dir.path()))
            .filter_map(|path| path.file_name().map(OsString::from))
            .collect();
        names.sort();
        names.dedup();
        Ok(names)
    }
    fn stats(&self, path: &Path) -> Result<FsStats, FsError> {
        let stats = self.stats.get(path).copied();
        stats.ok_or(FsError::Io(io::ErrorKind::NotFound))
    }
}

/// The API: an answer by request path, whatever the address; every request is recorded, and an
/// address that is not loopback is refused as the real one refuses it.
#[derive(Default)]
pub struct FakeApi {
    pub answers: BTreeMap<String, Result<Response, HttpError>>,
    pub requests: RefCell<Vec<(SocketAddr, String)>>,
}

impl DoctorApi for FakeApi {
    fn get(
        &self,
        address: SocketAddr,
        path: &str,
        _timeout: Duration,
    ) -> Result<Response, HttpError> {
        self.requests.borrow_mut().push((address, path.to_owned()));
        if !address.ip().is_loopback() {
            return Err(HttpError::NotLoopback { address });
        }
        match self.answers.get(path) {
            Some(answer) => answer.clone(),
            None => Ok(Response {
                status: 404,
                body: Vec::new(),
            }),
        }
    }
}

/// The node's view: canned answers, and a record of every call by name.
pub struct FakeNode {
    pub build: BuildFacts,
    pub relay: Result<RelayFacts, NodeViewError>,
    pub forger: Result<ForgerFacts, NodeViewError>,
    pub floor: Result<FloorFacts, NodeViewError>,
    pub store_window: Result<StoreWindow, NodeViewError>,
    pub calls: RefCell<Vec<&'static str>>,
}

impl Default for FakeNode {
    fn default() -> FakeNode {
        FakeNode {
            build: BuildFacts {
                version: "4.3.1-heartwood.0.1.0",
                features: vec!["devnet"],
            },
            relay: Err(NodeViewError::NotBuilt),
            forger: Err(NodeViewError::NotBuilt),
            floor: Err(NodeViewError::NotBuilt),
            store_window: Err(NodeViewError::NotBuilt),
            calls: RefCell::default(),
        }
    }
}

impl NodeView for FakeNode {
    fn build(&self) -> BuildFacts {
        self.build.clone()
    }
    fn relay(&self, _relay: &Instance) -> Result<RelayFacts, NodeViewError> {
        self.calls.borrow_mut().push("relay");
        self.relay.clone()
    }
    fn forger(&self, _forger: &Instance) -> Result<ForgerFacts, NodeViewError> {
        self.calls.borrow_mut().push("forger");
        self.forger.clone()
    }
    fn floor(
        &self,
        _relay: &Instance,
        _version: &str,
        _height: u32,
    ) -> Result<FloorFacts, NodeViewError> {
        self.calls.borrow_mut().push("floor");
        self.floor.clone()
    }
    fn store_window(
        &self,
        _relay: &Instance,
        _usernames: &[String],
        _start: WindowStart,
    ) -> Result<StoreWindow, NodeViewError> {
        self.calls.borrow_mut().push("store_window");
        self.store_window.clone()
    }
}

/// `/proc` and `/sys`: contents by file, descriptor targets by pid, and the pid list.
#[derive(Default)]
pub struct FakeProc {
    pub files: BTreeMap<PathBuf, Vec<u8>>,
    pub fd_targets: BTreeMap<u32, Vec<OsString>>,
    pub pids: Vec<u32>,
}

impl ProcView for FakeProc {
    fn read(&self, file: ProcFile) -> Result<Vec<u8>, FsError> {
        let bytes = self.files.get(&file.path()).cloned();
        bytes.ok_or(FsError::Io(io::ErrorKind::NotFound))
    }
    fn fd_targets(&self, pid: u32) -> Result<Vec<OsString>, FsError> {
        let targets = self.fd_targets.get(&pid).cloned();
        targets.ok_or(FsError::Io(io::ErrorKind::NotFound))
    }
    fn pids(&self) -> Result<Vec<u32>, FsError> {
        Ok(self.pids.clone())
    }
}

/// The probes: an answer by probe (missing when unset); every run is recorded.
#[derive(Default)]
pub struct FakeProbes {
    pub answers: BTreeMap<Probe, Result<ProbeOutput, ProbeError>>,
    pub runs: RefCell<Vec<Probe>>,
}

impl Probes for FakeProbes {
    fn run(&self, probe: Probe) -> Result<ProbeOutput, ProbeError> {
        self.runs.borrow_mut().push(probe);
        self.answers
            .get(&probe)
            .cloned()
            .unwrap_or(Err(ProbeError::Missing))
    }
}

/// The process table: a row and a supervisor state by role; an unset role is stopped.
#[derive(Default)]
pub struct FakeProcesses {
    pub rows: BTreeMap<Role, (StatusRow, Option<SupervisorFacts>)>,
}

impl FakeProcesses {
    fn stopped(instance: &Instance) -> StatusRow {
        StatusRow {
            pid: None,
            name: instance.name(),
            version: "4.3.1-heartwood.0.1.0".into(),
            status: heartwood_cli::SupervisedStatus::Stopped,
            uptime: None,
            restarts: 0,
            cpu_percent: None,
            rss_bytes: None,
        }
    }
}

impl ProcessView for FakeProcesses {
    fn status(&self, instance: &Instance) -> Result<StatusRow, ProcessError> {
        let row = self.rows.get(&instance.role).map(|(row, _)| row.clone());
        Ok(row.unwrap_or_else(|| FakeProcesses::stopped(instance)))
    }
    fn supervisor(&self, instance: &Instance) -> Result<Option<SupervisorFacts>, ProcessError> {
        Ok(self
            .rows
            .get(&instance.role)
            .and_then(|(_, state)| state.clone()))
    }
}

/// A world of fakes with a manual clock at [`NOW_MS`].
pub struct FakeWorld {
    pub fs: FakeFs,
    pub api: FakeApi,
    pub node: FakeNode,
    pub procfs: FakeProc,
    pub probes: FakeProbes,
    pub processes: FakeProcesses,
    pub clock: ManualClock,
}

impl Default for FakeWorld {
    fn default() -> FakeWorld {
        FakeWorld {
            fs: FakeFs::default(),
            api: FakeApi::default(),
            node: FakeNode::default(),
            procfs: FakeProc::default(),
            probes: FakeProbes::default(),
            processes: FakeProcesses::default(),
            clock: ManualClock::new(NOW_MS),
        }
    }
}

impl World for FakeWorld {
    fn fs(&self) -> &dyn DoctorFs {
        &self.fs
    }
    fn api(&self) -> &dyn DoctorApi {
        &self.api
    }
    fn node(&self) -> &dyn NodeView {
        &self.node
    }
    fn procfs(&self) -> &dyn ProcView {
        &self.procfs
    }
    fn probes(&self) -> &dyn Probes {
        &self.probes
    }
    fn processes(&self) -> &dyn ProcessView {
        &self.processes
    }
    fn clock(&self) -> &dyn Clock {
        &self.clock
    }
}
