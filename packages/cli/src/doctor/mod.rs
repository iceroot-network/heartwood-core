//! `heartwood doctor`: read-only diagnostics of one instance on this host.
//!
//! The doctor reads the node's configuration (through the node's own loaders, [`seams::NodeView`]),
//! its files' metadata, its logs, the process table, its own relay's API on a loopback address,
//! `/proc`, `/sys` and three fixed probes; it applies fixed rules and gives each check's findings
//! as pass, warn, fail, unknown or skipped, with plain advice. It writes no file, sends no signal,
//! holds no lock beyond the momentary shared probes of `status`, never reads a key file, never
//! connects to a P2P port, and its only network use is an HTTP GET to its own relay's API on a
//! loopback address. The same inputs give the same report, byte for byte.
//!
//! - [`seams`]: [`seams::World`], everything the doctor reads, as traits; [`HostWorld`] is the
//!   real one.
//! - [`view`]: the parsed facts the checks read ([`view::View`]) and [`view::Gathered`], the real
//!   view, with the run's budget.
//! - [`sources`]: the real sources and their parsers; [`checks`]: the checks, by area;
//!   [`registry`]: every check in the report's order.
//! - [`report`], [`render`], [`redact`]: the report, its text and JSON, and redaction;
//!   [`docs`]: the check documentation, as data.
//! - [`mcp`]: the read-only MCP server over standard input and output.

pub mod checks;
pub mod docs;
pub mod mcp;
pub mod redact;
pub mod registry;
pub mod render;
pub mod report;
pub mod seams;
pub mod sources;
pub mod view;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use heartwood_kernel::SystemClock;
use heartwood_kernel::paths::Paths;

use crate::doctor::registry::{CheckId, CheckInput, ENTRIES};
use crate::doctor::report::{Finding, HostIdentity, Report, ReportScope, keys};
use crate::doctor::seams::{
    DoctorApi, DoctorFs, KEY_FILE_NAMES, KeyFile, NodeView, Probes, ProcView, ProcessView, Subject,
    SupervisorFacts, World,
};
use crate::doctor::sources::fs::HostFs;
use crate::doctor::sources::probes::HostProbes;
use crate::doctor::sources::procfs::HostProc;
use crate::doctor::view::{Gathered, View};
use crate::errors::{HttpError, ProcessError};
use crate::http::Response;
use crate::seams::{Instance, Role, StatusRow};

/// The shortest window.
pub const MIN_WINDOW: Duration = Duration::from_secs(5 * 60);
/// The longest window.
pub const MAX_WINDOW: Duration = Duration::from_secs(24 * 60 * 60);
/// The window when none is given.
pub const DEFAULT_WINDOW: Duration = Duration::from_secs(60 * 60);
/// The most a run may take.
pub const RUN_BUDGET: Duration = Duration::from_secs(5);

/// Which roles to examine.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum RoleSelection {
    /// The relay only.
    Relay,
    /// The forger only.
    Forger,
    /// Every role present on this host (the default).
    #[default]
    All,
}

impl RoleSelection {
    /// Whether findings about `subject` are wanted: host findings always are.
    pub fn wants(self, subject: Subject) -> bool {
        match self {
            RoleSelection::All => true,
            RoleSelection::Relay => subject != Subject::Forger,
            RoleSelection::Forger => subject != Subject::Relay,
        }
    }
}

/// A set of checks.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum DoctorProfile {
    /// Every check of a running node (the default).
    #[default]
    Default,
    /// The default checks and the onboarding sync check.
    Onboarding,
}

impl DoctorProfile {
    /// The profile's name.
    pub const fn name(self) -> &'static str {
        match self {
            DoctorProfile::Default => "default",
            DoctorProfile::Onboarding => "onboarding",
        }
    }
}

/// The redactions asked for. Addresses are always redacted in the JSON report and the MCP server;
/// `addresses: false` (`--show-addresses`) shows them in the local text only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Redaction {
    /// Replace IP addresses and host names by labels.
    pub addresses: bool,
    /// Replace delegate names and public keys by labels (`--hide-identity`).
    pub identity: bool,
}

impl Default for Redaction {
    fn default() -> Redaction {
        Redaction {
            addresses: true,
            identity: false,
        }
    }
}

/// How a run is made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DoctorOptions {
    /// The roles examined.
    pub roles: RoleSelection,
    /// The checks run (`--check`); empty: every check of the profile.
    pub checks: Vec<CheckId>,
    /// The profile.
    pub profile: DoctorProfile,
    /// How far the log and store checks look back, from [`MIN_WINDOW`] to [`MAX_WINDOW`].
    pub window: Duration,
    /// Our delegates' names (`--delegate`), before those the logs and the signing record give.
    pub delegates: Vec<String>,
    /// The redactions.
    pub redaction: Redaction,
    /// The most the run may take.
    pub budget: Duration,
}

impl Default for DoctorOptions {
    fn default() -> DoctorOptions {
        DoctorOptions {
            roles: RoleSelection::All,
            checks: Vec::new(),
            profile: DoctorProfile::Default,
            window: DEFAULT_WINDOW,
            delegates: Vec::new(),
            redaction: Redaction::default(),
            budget: RUN_BUDGET,
        }
    }
}

/// The instance examined: its token, network and directories, resolved as every role command
/// resolves them, and what the renderer redacts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DoctorScope {
    /// The token.
    pub token: String,
    /// The network.
    pub network: String,
    /// The five directories.
    pub paths: Paths,
    /// The home directory, redacted as `~`.
    pub home: Option<PathBuf>,
    /// The user's name, from the environment, redacted as `user`.
    pub user: Option<String>,
}

impl DoctorScope {
    /// The scope of `instance`'s token, network and directories.
    pub fn of(instance: &Instance, home: Option<PathBuf>, user: Option<String>) -> DoctorScope {
        DoctorScope {
            token: instance.token.clone(),
            network: instance.network.clone(),
            paths: instance.paths.clone(),
            home,
            user,
        }
    }

    /// The instance of `role`.
    pub fn instance(&self, role: Role) -> Instance {
        Instance {
            token: self.token.clone(),
            network: self.network.clone(),
            role,
            paths: self.paths.clone(),
        }
    }
}

/// Run the checks `options` selects on `scope` over `world`. Every registry entry appears in the
/// report once: a check the profile or `--role` leaves out is `skipped` with the reason, except
/// when `--check` names the checks, which then appear alone.
pub fn run(world: &dyn World, scope: &DoctorScope, options: &DoctorOptions) -> Report {
    let view = Gathered::new(world, scope, options);
    let mut findings = Vec::new();
    for entry in &ENTRIES {
        let id = entry.check_id();
        let named = options.checks.is_empty() || options.checks.contains(&id);
        let subject = entry.subjects.first().copied().unwrap_or(Subject::Host);
        let wanted = entry.subjects.iter().any(|s| options.roles.wants(*s));
        let skip = match (named, entry.profiles.contains(&options.profile), wanted) {
            (false, _, _) => continue,
            (true, false, _) if options.checks.is_empty() => Some(keys::SKIPPED_PROFILE),
            (true, _, false) => Some(keys::SKIPPED_ROLE),
            _ => None,
        };
        if let Some(key) = skip {
            findings.push(Finding::skipped(id, subject, key));
            continue;
        }
        let input = CheckInput {
            entry,
            view: &view,
            options,
        };
        let found = (entry.run)(&input);
        findings.extend(found.into_iter().filter(|f| options.roles.wants(f.subject)));
    }
    let presence = view.roles_present().unwrap_or_default();
    let roles = Role::ALL.into_iter().filter(|role| presence.has(*role));
    let window_minutes = options.window.as_secs() / 60;
    Report {
        created_at_ms: view.now_ms(),
        tool: view.build(),
        scope: ReportScope {
            token: scope.token.clone(),
            network: scope.network.clone(),
            roles: roles.collect(),
            profile: options.profile,
            window_minutes: u32::try_from(window_minutes).unwrap_or(u32::MAX),
        },
        redaction: options.redaction,
        host: view.host(),
        identity: HostIdentity {
            home: scope.home.clone(),
            user: scope.user.clone(),
            host_name: view.host_name().ok().map(str::to_owned),
        },
        findings,
    }
}

/// The real [`DoctorApi`]: [`crate::http::get`], which refuses every address that is not loopback.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HostApi;

impl DoctorApi for HostApi {
    fn get(
        &self,
        address: SocketAddr,
        path: &str,
        timeout: Duration,
    ) -> Result<Response, HttpError> {
        crate::http::get(address, path, timeout)
    }
}

/// The real [`ProcessView`]: the process table's row, and the supervisor's state file without the
/// node's command line.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HostProcesses;

impl ProcessView for HostProcesses {
    fn status(&self, instance: &Instance) -> Result<StatusRow, ProcessError> {
        crate::process::status(instance)
    }

    fn supervisor(&self, instance: &Instance) -> Result<Option<SupervisorFacts>, ProcessError> {
        let state = crate::supervisor::state::SupervisorState::read(&instance.state_path())?;
        Ok(state.map(|state| SupervisorFacts {
            started_at_ms: state.started_at_ms,
            restarts: state.restarts,
            unstable: state.unstable,
            version: state.version,
        }))
    }
}

/// The real [`World`] of one instance, over the host and `node`, the node's own view.
pub struct HostWorld<'a> {
    node: &'a dyn NodeView,
    fs: HostFs,
    procfs: HostProc,
    probes: HostProbes,
    clock: SystemClock,
}

impl<'a> HostWorld<'a> {
    /// The world of `scope`, whose key files every open is checked against, with `node`.
    pub fn new(scope: &DoctorScope, node: &'a dyn NodeView) -> HostWorld<'a> {
        let key_files = KEY_FILE_NAMES.iter();
        let key_files = key_files.map(|name| KeyFile::new(scope.paths.config.join(name)));
        HostWorld {
            node,
            fs: HostFs::new(key_files.collect()),
            procfs: HostProc,
            probes: HostProbes,
            clock: SystemClock,
        }
    }
}

impl World for HostWorld<'_> {
    fn fs(&self) -> &dyn DoctorFs {
        &self.fs
    }

    fn api(&self) -> &dyn DoctorApi {
        &HostApi
    }

    fn node(&self) -> &dyn NodeView {
        self.node
    }

    fn procfs(&self) -> &dyn ProcView {
        &self.procfs
    }

    fn probes(&self) -> &dyn Probes {
        &self.probes
    }

    fn processes(&self) -> &dyn ProcessView {
        &HostProcesses
    }

    fn clock(&self) -> &dyn heartwood_kernel::Clock {
        &self.clock
    }
}
