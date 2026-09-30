//! Support for the doctor's tests in this crate: a node's directories with both roles' profiles, a
//! world over the real files' metadata with a node view that records its calls, the doctor's
//! binary, and the address block of these tests.

#![allow(dead_code, reason = "each test file uses part of the support")]

use std::cell::RefCell;
use std::ffi::OsString;
use std::io;
use std::net::SocketAddr;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::Duration;

use heartwood_cli::doctor::HostProcesses;
use heartwood_cli::doctor::seams::{
    BuildFacts, Clock, DoctorApi, DoctorFs, FileFacts, FileKind, FloorFacts, ForgerFacts, FsError,
    FsStats, Meta, NodeView, NodeViewError, Probe, ProbeError, ProbeOutput, Probes, ProcFile,
    ProcView, ProcessView, Readable, RelayFacts, StoreWindow, Tail, WindowStart, World,
};
use heartwood_cli::http::Response;
use heartwood_cli::{HttpError, Instance, Role};
use heartwood_core::doctor_view::CoreNodeView;
use heartwood_kernel::SystemClock;
use heartwood_kernel::paths::Paths;
use serde_json::{Value, json};

/// A value no output of the doctor may ever hold: it is set in the node's `.env`.
pub const ENV_CANARY: &str = "env-canary-5f1c0de7";
/// The forger's `aux`, which no output may hold either.
pub const AUX: &str = "a0c4c7f3a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c";

/// The committed one-node corpus's crypto files.
fn corpus() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/difftest/corpus/txgen-one-node/crypto")
}

/// An address of these tests' loopback block (`HEARTWOOD_TEST_LOOPBACK`, else `127.24.0`).
pub fn loopback(last: u8) -> String {
    let block = std::env::var("HEARTWOOD_TEST_LOOPBACK").unwrap_or_default();
    let block = if block.is_empty() { "127.24.0" } else { &block };
    format!("{block}.{last}")
}

/// A node's five directories under a scratch root, with both roles' profiles.
pub struct Node {
    /// The scratch root.
    pub root: PathBuf,
}

impl Node {
    /// A fresh node named `name` whose `.env` is `env` and the canary.
    pub fn new(name: &str, env: &str) -> Node {
        let root = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join("doctor")
            .join(name);
        let _ = std::fs::remove_dir_all(&root);
        let node = Node { root };
        let crypto = node.config().join("crypto");
        std::fs::create_dir_all(&crypto).unwrap();
        std::fs::create_dir_all(node.home()).unwrap();
        for file in [
            "network.json",
            "milestones.json",
            "genesisBlock.json",
            "exceptions.json",
        ] {
            std::fs::copy(corpus().join(file), crypto.join(file)).unwrap();
        }
        node.write(".env", &format!("{env}DOCTOR_CANARY={ENV_CANARY}\n"));
        node.write(
            "peers.json",
            r#"{"list": [{"ip": "192.0.2.1", "port": 6002}]}"#,
        );
        let forger = json!({"hosts": [{"hostname": "127.0.0.1", "port": 6102}], "aux": AUX});
        node.app(json!(["127.0.0.1", "::1", "10.0.0.0/24"]), forger);
        node.chmod(&node.config(), 0o700);
        node
    }

    /// The configuration directory.
    pub fn config(&self) -> PathBuf {
        self.root.join("config")
    }

    /// The home directory.
    pub fn home(&self) -> PathBuf {
        self.root.join("home")
    }

    /// The five directories.
    pub fn paths(&self) -> Paths {
        let dir = |name: &str| self.root.join(name);
        Paths {
            config: self.config(),
            data: dir("data"),
            cache: dir("cache"),
            log: dir("log"),
            temp: dir("temp"),
        }
    }

    /// The instance of `role` on the network `testnet`.
    pub fn instance(&self, role: Role) -> Instance {
        Instance {
            token: "solar".into(),
            network: "testnet".into(),
            role,
            paths: self.paths(),
        }
    }

    /// The node's view with no process variable of its own.
    pub fn view(&self) -> CoreNodeView {
        CoreNodeView {
            process: Vec::new(),
            home: self.home(),
            cwd: self.root.clone(),
        }
    }

    /// The process variables the roles' starts read: the five `CORE_PATH_*`.
    pub fn process(&self) -> Vec<(String, String)> {
        let paths = self.paths();
        let dirs = [
            ("CONFIG", paths.config),
            ("DATA", paths.data),
            ("CACHE", paths.cache),
        ];
        let dirs = dirs
            .into_iter()
            .chain([("LOG", paths.log), ("TEMP", paths.temp)]);
        dirs.map(|(kind, dir)| (format!("CORE_PATH_{kind}"), dir.display().to_string()))
            .collect()
    }

    /// Write `text` to the configuration file `file`.
    pub fn write(&self, file: &str, text: &str) {
        std::fs::write(self.config().join(file), text).unwrap();
    }

    /// Write an `app.json` whose relay's p2p entry has `remote_access` and whose forger has the
    /// options `forger`.
    pub fn app(&self, remote_access: Value, forger: Value) {
        let package = |name: &str| json!({"package": format!("@solar-network/{name}")});
        let p2p =
            json!({"package": "@solar-network/p2p", "options": {"remoteAccess": remote_access}});
        let relay = json!([
            package("logger"),
            package("state"),
            package("database"),
            package("transactions"),
            package("pool"),
            p2p,
            package("blockchain"),
            package("api"),
        ]);
        let forger =
            json!([package("logger"), {"package": "@solar-network/forger", "options": forger}]);
        let app = json!({"core": {"plugins": []}, "relay": {"plugins": relay}, "forger": {"plugins": forger}});
        self.write("app.json", &app.to_string());
    }

    /// Set the permission bits of `path`.
    pub fn chmod(&self, path: &Path, mode: u32) {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    }
}

/// A node view that records the name of each call and answers with `inner`.
pub struct Recording<'a> {
    /// The view answering.
    pub inner: &'a dyn NodeView,
    /// The calls, in order.
    pub calls: RefCell<Vec<&'static str>>,
}

impl<'a> Recording<'a> {
    /// A recording of `inner`.
    pub fn new(inner: &'a dyn NodeView) -> Recording<'a> {
        let calls = RefCell::default();
        Recording { inner, calls }
    }
}

impl NodeView for Recording<'_> {
    fn build(&self) -> BuildFacts {
        self.inner.build()
    }
    fn relay(&self, relay: &Instance) -> Result<RelayFacts, NodeViewError> {
        self.calls.borrow_mut().push("relay");
        self.inner.relay(relay)
    }
    fn forger(&self, forger: &Instance) -> Result<ForgerFacts, NodeViewError> {
        self.calls.borrow_mut().push("forger");
        self.inner.forger(forger)
    }
    fn floor(
        &self,
        relay: &Instance,
        version: &str,
        height: u32,
    ) -> Result<FloorFacts, NodeViewError> {
        self.calls.borrow_mut().push("floor");
        self.inner.floor(relay, version, height)
    }
    fn store_window(
        &self,
        relay: &Instance,
        usernames: &[String],
        start: WindowStart,
    ) -> Result<StoreWindow, NodeViewError> {
        self.calls.borrow_mut().push("store_window");
        self.inner.store_window(relay, usernames, start)
    }
}

/// Files' metadata from the real filesystem (`lstat`, `stat`, `readlink`); nothing is ever
/// opened.
pub struct MetadataFs;

fn meta(metadata: &std::fs::Metadata) -> Meta {
    let kind = match metadata.file_type() {
        t if t.is_symlink() => FileKind::Symlink,
        t if t.is_dir() => FileKind::Dir,
        t if t.is_file() => FileKind::File,
        _ => FileKind::Other,
    };
    Meta {
        kind,
        mode: metadata.mode() & 0o7777,
        uid: metadata.uid(),
        gid: metadata.gid(),
        size: metadata.size(),
        dev: metadata.dev(),
        ino: metadata.ino(),
        nlink: metadata.nlink(),
        modified_ms: metadata.mtime() * 1000,
    }
}

impl DoctorFs for MetadataFs {
    fn facts(&self, path: &Path) -> io::Result<FileFacts> {
        let own = meta(&std::fs::symlink_metadata(path)?);
        let link = own.kind == FileKind::Symlink;
        let target = link
            .then(|| std::fs::metadata(path).ok().map(|m| meta(&m)))
            .flatten();
        let link_target = link.then(|| std::fs::read_link(path).ok()).flatten();
        let path = path.to_path_buf();
        Ok(FileFacts {
            path,
            own,
            target,
            link_target,
        })
    }
    fn read(&self, _file: &Readable, _max: u64) -> Result<Vec<u8>, FsError> {
        Err(FsError::NotBuilt)
    }
    fn tail(&self, _file: &Readable, _max: u64) -> Result<Tail, FsError> {
        Err(FsError::NotBuilt)
    }
    fn list(&self, _dir: &Readable) -> Result<Vec<OsString>, FsError> {
        Err(FsError::NotBuilt)
    }
    fn stats(&self, _path: &Path) -> Result<FsStats, FsError> {
        Err(FsError::NotBuilt)
    }
}

/// No API, no `/proc`, no probe: each refuses.
pub struct Nothing;

impl DoctorApi for Nothing {
    fn get(
        &self,
        address: SocketAddr,
        _path: &str,
        _timeout: Duration,
    ) -> Result<Response, HttpError> {
        Err(HttpError::NotLoopback { address })
    }
}

impl ProcView for Nothing {
    fn read(&self, _file: ProcFile) -> Result<Vec<u8>, FsError> {
        Err(FsError::NotBuilt)
    }
    fn fd_targets(&self, _pid: u32) -> Result<Vec<OsString>, FsError> {
        Err(FsError::NotBuilt)
    }
    fn pids(&self) -> Result<Vec<u32>, FsError> {
        Err(FsError::NotBuilt)
    }
}

impl Probes for Nothing {
    fn run(&self, _probe: Probe) -> Result<ProbeOutput, ProbeError> {
        Err(ProbeError::Missing)
    }
}

/// A world of the real files' metadata, the real process table and `node`.
pub struct MetadataWorld<'a> {
    /// The node's view.
    pub node: &'a dyn NodeView,
}

impl World for MetadataWorld<'_> {
    fn fs(&self) -> &dyn DoctorFs {
        &MetadataFs
    }
    fn api(&self) -> &dyn DoctorApi {
        &Nothing
    }
    fn node(&self) -> &dyn NodeView {
        self.node
    }
    fn procfs(&self) -> &dyn ProcView {
        &Nothing
    }
    fn probes(&self) -> &dyn Probes {
        &Nothing
    }
    fn processes(&self) -> &dyn ProcessView {
        &HostProcesses
    }
    fn clock(&self) -> &dyn Clock {
        &SystemClock
    }
}

/// `heartwood <args>` with only `vars` in its environment (and `PATH`).
pub fn heartwood(args: &[&str], vars: &[(String, String)]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_heartwood"));
    command.env_clear().env("PATH", "/usr/bin:/bin").args(args);
    command.envs(vars.iter().map(|(k, v)| (k, v)));
    command.output().unwrap()
}

/// The path of `program` in `/usr/bin` or `/bin`, if installed.
pub fn program(program: &str) -> Option<PathBuf> {
    let paths = ["/usr/bin", "/bin"].map(|dir| Path::new(dir).join(program));
    paths.into_iter().find(|path| path.is_file())
}
