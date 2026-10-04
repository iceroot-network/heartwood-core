//! Every check, in the report's order: its id, area, subjects, sources, the statuses it can give,
//! the profiles it belongs to, and its function. A check reads only its [`CheckInput`]'s view and
//! options, and gives one finding per subject it judges. Until its group builds it, a check's
//! function gives [`not_built`]: `unknown`, "not built", reading nothing.

use crate::doctor::checks;
use crate::doctor::report::{Advice, Basis, Finding, Status, keys};
use crate::doctor::seams::Subject;
use crate::doctor::view::{Source, View};
use crate::doctor::{DoctorOptions, DoctorProfile};

// Short names for the table of [`ENTRIES`].
use Source as S;
use Status::{Fail as F, Pass as P, Skipped as K, Unknown as U, Warn as W};
use Subject::{Forger as FORGER, Host as HOST, Relay as RELAY};

/// A check's id, such as `keys.file`: only the registry's ids exist.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CheckId(&'static str);

impl CheckId {
    /// The registry's id spelled `text`.
    pub fn parse(text: &str) -> Option<CheckId> {
        ENTRIES
            .iter()
            .find(|entry| entry.id == text)
            .map(CheckEntry::check_id)
    }

    /// The id's text.
    pub const fn as_str(self) -> &'static str {
        self.0
    }

    /// The anchor of its page on the documentation site: the id with `-` for `.`.
    pub fn anchor(self) -> String {
        self.0.replace('.', "-")
    }
}

/// A check's area: the check file, and the documentation file, it lives in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Area {
    /// Processes and start errors.
    Process,
    /// Peers.
    Peers,
    /// Sync.
    Sync,
    /// Slots.
    Slots,
    /// The forger's link and failover.
    Forger,
    /// Attestation and quorum.
    Quorum,
    /// Network exposure.
    Net,
    /// The clock.
    Clock,
    /// Disks.
    Disk,
    /// Configuration and key files.
    Config,
    /// Versions.
    Version,
    /// The host.
    Host,
    /// The onboarding profile.
    Onboarding,
}

impl Area {
    /// Every area, in the registry's order.
    pub const ALL: [Area; 13] = [
        Area::Process,
        Area::Peers,
        Area::Sync,
        Area::Slots,
        Area::Forger,
        Area::Quorum,
        Area::Net,
        Area::Clock,
        Area::Disk,
        Area::Config,
        Area::Version,
        Area::Host,
        Area::Onboarding,
    ];

    /// The area's name: its check file's and documentation file's stem.
    pub const fn name(self) -> &'static str {
        match self {
            Area::Process => "process",
            Area::Peers => "peers",
            Area::Sync => "sync",
            Area::Slots => "slots",
            Area::Forger => "forger",
            Area::Quorum => "quorum",
            Area::Net => "net",
            Area::Clock => "clock",
            Area::Disk => "disk",
            Area::Config => "config",
            Area::Version => "version",
            Area::Host => "host",
            Area::Onboarding => "onboarding",
        }
    }
}

/// What a check reads and is judged against.
pub struct CheckInput<'a> {
    /// Its registry entry.
    pub entry: &'static CheckEntry,
    /// The parsed view.
    pub view: &'a dyn View,
    /// The run's options.
    pub options: &'a DoctorOptions,
}

impl CheckInput<'_> {
    /// The check's id.
    pub fn id(&self) -> CheckId {
        self.entry.check_id()
    }
}

/// A check's function.
pub type CheckFn = fn(&CheckInput<'_>) -> Vec<Finding>;

/// One check.
pub struct CheckEntry {
    /// Its id.
    pub id: &'static str,
    /// Its area.
    pub area: Area,
    /// What its findings are about.
    pub subjects: &'static [Subject],
    /// What it reads.
    pub sources: &'static [Source],
    /// The statuses it can give; the registry walk requires a case for each.
    pub statuses: &'static [Status],
    /// The profiles it runs in.
    pub profiles: &'static [DoctorProfile],
    /// Its function.
    pub run: CheckFn,
}

impl CheckEntry {
    /// Its id.
    pub const fn check_id(&self) -> CheckId {
        CheckId(self.id)
    }
}

impl std::fmt::Debug for CheckEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CheckEntry")
            .field("id", &self.id)
            .field("area", &self.area)
            .finish_non_exhaustive()
    }
}

/// The finding of a check that is not built yet: `unknown`, "not built", about its first
/// subject. It reads nothing.
pub fn not_built(input: &CheckInput<'_>) -> Vec<Finding> {
    let subject = input
        .entry
        .subjects
        .first()
        .copied()
        .unwrap_or(Subject::Host);
    let advice = Advice::key(keys::NOT_BUILT);
    vec![Finding::new(
        input.id(),
        subject,
        Status::Unknown,
        Basis::System,
        advice,
    )]
}

/// Whether `findings`, a check's answer, is [`not_built`]'s.
pub fn is_not_built(findings: &[Finding]) -> bool {
    !findings.is_empty() && findings.iter().all(|f| f.advice.key == keys::NOT_BUILT)
}

const DEFAULT: &[DoctorProfile] = &[DoctorProfile::Default, DoctorProfile::Onboarding];
const ONBOARDING: &[DoctorProfile] = &[DoctorProfile::Onboarding];

/// Every check, in the report's order.
#[rustfmt::skip]
pub static ENTRIES: [CheckEntry; 47] = [
    // Processes.
    CheckEntry { id: "process.roles", area: Area::Process, subjects: &[RELAY, FORGER], sources: &[S::Process, S::Files], statuses: &[P, W, F, U], profiles: DEFAULT, run: checks::process::roles },
    CheckEntry { id: "process.core-role", area: Area::Process, subjects: &[HOST], sources: &[S::Process], statuses: &[K], profiles: DEFAULT, run: checks::process::core_role },
    CheckEntry { id: "relay.start-errors", area: Area::Process, subjects: &[RELAY], sources: &[S::ErrorCapture, S::RelayLog, S::Process], statuses: &[P, F, U, K], profiles: DEFAULT, run: checks::process::relay_start_errors },
    // Peers and sync.
    CheckEntry { id: "peers.count", area: Area::Peers, subjects: &[RELAY], sources: &[S::Api, S::Config], statuses: &[P, W, F, U, K], profiles: DEFAULT, run: checks::peers::count },
    CheckEntry { id: "peers.height", area: Area::Peers, subjects: &[RELAY], sources: &[S::Api, S::RelayLog], statuses: &[P, W, F, U, K], profiles: DEFAULT, run: checks::peers::height },
    CheckEntry { id: "peers.diversity", area: Area::Peers, subjects: &[RELAY], sources: &[S::Api], statuses: &[P, W, U, K], profiles: DEFAULT, run: checks::peers::diversity },
    CheckEntry { id: "sync.state", area: Area::Sync, subjects: &[RELAY], sources: &[S::Api, S::RelayLog], statuses: &[P, W, F, U, K], profiles: DEFAULT, run: checks::sync::state },
    CheckEntry { id: "sync.forks", area: Area::Sync, subjects: &[RELAY], sources: &[S::RelayLog, S::Process], statuses: &[P, W, F, U, K], profiles: DEFAULT, run: checks::sync::forks },
    // Slots.
    CheckEntry { id: "slots.active", area: Area::Slots, subjects: &[FORGER], sources: &[S::ForgerLog], statuses: &[P, W, U, K], profiles: DEFAULT, run: checks::slots::active },
    CheckEntry { id: "slots.missed", area: Area::Slots, subjects: &[FORGER], sources: &[S::Store, S::ForgerLog, S::Api, S::SigningRecord, S::Config, S::Process], statuses: &[P, W, F, U, K], profiles: DEFAULT, run: checks::slots::missed },
    // The forger's link and failover.
    CheckEntry { id: "forger.hosts", area: Area::Forger, subjects: &[FORGER], sources: &[S::Config], statuses: &[P, W, U, K], profiles: DEFAULT, run: checks::forger::hosts },
    CheckEntry { id: "forger.link", area: Area::Forger, subjects: &[FORGER], sources: &[S::Sockets, S::Config], statuses: &[P, W, F, U, K], profiles: DEFAULT, run: checks::forger::link },
    CheckEntry { id: "forger.link-history", area: Area::Forger, subjects: &[FORGER], sources: &[S::ForgerLog, S::Sockets], statuses: &[P, W, F, U, K], profiles: DEFAULT, run: checks::forger::link_history },
    CheckEntry { id: "forger.one-key-one-forger", area: Area::Forger, subjects: &[FORGER], sources: &[S::ForgerLog, S::Store, S::Api, S::SigningRecord, S::Config], statuses: &[P, W, F, U, K], profiles: DEFAULT, run: checks::forger::one_key_one_forger },
    CheckEntry { id: "forger.signing-record", area: Area::Forger, subjects: &[FORGER], sources: &[S::SigningRecord, S::Files, S::ForgerLog], statuses: &[P, W, F, U, K], profiles: DEFAULT, run: checks::forger::signing_record },
    CheckEntry { id: "forger.start-errors", area: Area::Forger, subjects: &[FORGER], sources: &[S::ErrorCapture, S::Process], statuses: &[P, F, U, K], profiles: DEFAULT, run: checks::forger::start_errors },
    // Attestation and quorum.
    CheckEntry { id: "quorum.forging-allowed", area: Area::Quorum, subjects: &[FORGER], sources: &[S::ForgerLog, S::Api, S::Config, S::Process], statuses: &[P, W, F, U, K], profiles: DEFAULT, run: checks::quorum::forging_allowed },
    CheckEntry { id: "quorum.attestation", area: Area::Quorum, subjects: &[FORGER, RELAY], sources: &[S::ForgerLog, S::RelayLog, S::Sockets, S::Process, S::Config], statuses: &[P, W, F, U, K], profiles: DEFAULT, run: checks::quorum::attestation },
    CheckEntry { id: "quorum.reach", area: Area::Quorum, subjects: &[RELAY], sources: &[S::Files], statuses: &[K], profiles: DEFAULT, run: checks::quorum::reach },
    // Network exposure.
    CheckEntry { id: "net.p2p-listen", area: Area::Net, subjects: &[RELAY], sources: &[S::Config, S::Sockets], statuses: &[P, W, F, U, K], profiles: DEFAULT, run: checks::net::p2p_listen },
    CheckEntry { id: "net.api-exposure", area: Area::Net, subjects: &[RELAY], sources: &[S::Config, S::Sockets], statuses: &[P, W, U, K], profiles: DEFAULT, run: checks::net::api_exposure },
    CheckEntry { id: "net.forger-no-listener", area: Area::Net, subjects: &[FORGER], sources: &[S::Sockets], statuses: &[P, F, U, K], profiles: DEFAULT, run: checks::net::forger_no_listener },
    CheckEntry { id: "net.remote-access", area: Area::Net, subjects: &[RELAY], sources: &[S::Config, S::Sockets], statuses: &[P, W, F, U, K], profiles: DEFAULT, run: checks::net::remote_access },
    CheckEntry { id: "net.firewall", area: Area::Net, subjects: &[HOST], sources: &[S::Probes, S::Files], statuses: &[P, W, U], profiles: DEFAULT, run: checks::net::firewall },
    CheckEntry { id: "net.reachable", area: Area::Net, subjects: &[RELAY], sources: &[S::Config], statuses: &[U, K], profiles: DEFAULT, run: checks::net::reachable },
    // The clock.
    CheckEntry { id: "clock.sync", area: Area::Clock, subjects: &[HOST], sources: &[S::Probes, S::Files], statuses: &[P, W, F, U], profiles: DEFAULT, run: checks::clock::sync },
    CheckEntry { id: "clock.vs-chain", area: Area::Clock, subjects: &[RELAY], sources: &[S::RelayLog, S::Store], statuses: &[P, W, F, U, K], profiles: DEFAULT, run: checks::clock::vs_chain },
    CheckEntry { id: "clock.symptoms", area: Area::Clock, subjects: &[RELAY, FORGER], sources: &[S::RelayLog, S::ForgerLog], statuses: &[P, W, F, U, K], profiles: DEFAULT, run: checks::clock::symptoms },
    // Disks.
    CheckEntry { id: "disk.space", area: Area::Disk, subjects: &[HOST], sources: &[S::Files, S::Api, S::Config], statuses: &[P, W, F, U], profiles: DEFAULT, run: checks::disk::space },
    CheckEntry { id: "disk.other", area: Area::Disk, subjects: &[HOST], sources: &[S::Files, S::Proc], statuses: &[P, W, F, U, K], profiles: DEFAULT, run: checks::disk::other },
    CheckEntry { id: "disk.inodes", area: Area::Disk, subjects: &[HOST], sources: &[S::Files], statuses: &[P, W, F, U], profiles: DEFAULT, run: checks::disk::inodes },
    CheckEntry { id: "disk.type", area: Area::Disk, subjects: &[HOST], sources: &[S::Proc, S::Sys], statuses: &[P, W, F, U], profiles: DEFAULT, run: checks::disk::disk_type },
    CheckEntry { id: "disk.slow-writes", area: Area::Disk, subjects: &[FORGER], sources: &[S::ForgerLog], statuses: &[P, W, F, U, K], profiles: DEFAULT, run: checks::disk::slow_writes },
    // Configuration and key files.
    CheckEntry { id: "config.load", area: Area::Config, subjects: &[RELAY, FORGER], sources: &[S::Config], statuses: &[P, W, F, U, K], profiles: DEFAULT, run: checks::config::load },
    CheckEntry { id: "config.log-level", area: Area::Config, subjects: &[RELAY, FORGER], sources: &[S::Config], statuses: &[P, W, U, K], profiles: DEFAULT, run: checks::config::log_level },
    CheckEntry { id: "config.run-dir", area: Area::Config, subjects: &[HOST], sources: &[S::Files, S::Proc], statuses: &[P, W, F, U], profiles: DEFAULT, run: checks::config::run_dir },
    CheckEntry { id: "keys.file", area: Area::Config, subjects: &[FORGER], sources: &[S::KeyFiles, S::Proc], statuses: &[P, W, F, U, K], profiles: DEFAULT, run: checks::config::keys_file },
    CheckEntry { id: "keys.dir", area: Area::Config, subjects: &[HOST], sources: &[S::Files], statuses: &[P, F, U], profiles: DEFAULT, run: checks::config::keys_dir },
    CheckEntry { id: "keys.stray", area: Area::Config, subjects: &[RELAY], sources: &[S::KeyFiles, S::Process, S::Files], statuses: &[P, W, U, K], profiles: DEFAULT, run: checks::config::keys_stray },
    // Versions.
    CheckEntry { id: "version.floor", area: Area::Version, subjects: &[RELAY], sources: &[S::Api, S::Store, S::Config, S::Process, S::Build], statuses: &[P, W, F, U, K], profiles: DEFAULT, run: checks::version::floor },
    CheckEntry { id: "version.installed", area: Area::Version, subjects: &[RELAY, FORGER], sources: &[S::Process, S::Api, S::Build], statuses: &[P, W, U, K], profiles: DEFAULT, run: checks::version::installed },
    CheckEntry { id: "version.build", area: Area::Version, subjects: &[HOST], sources: &[S::Build], statuses: &[P, F], profiles: DEFAULT, run: checks::version::build },
    CheckEntry { id: "version.peers", area: Area::Version, subjects: &[RELAY], sources: &[S::Api, S::Build], statuses: &[P, W, U, K], profiles: DEFAULT, run: checks::version::peers },
    // The host.
    CheckEntry { id: "host.resources", area: Area::Host, subjects: &[HOST], sources: &[S::Proc, S::Process], statuses: &[P, W, U], profiles: DEFAULT, run: checks::host::resources },
    CheckEntry { id: "host.memory", area: Area::Host, subjects: &[HOST], sources: &[S::Proc], statuses: &[P, W, F, U], profiles: DEFAULT, run: checks::host::memory },
    CheckEntry { id: "host.user", area: Area::Host, subjects: &[HOST], sources: &[S::Proc, S::Files], statuses: &[P, W, U], profiles: DEFAULT, run: checks::host::user },
    // Onboarding.
    CheckEntry { id: "onboarding.sync-proof", area: Area::Onboarding, subjects: &[RELAY], sources: &[S::Api, S::Sockets], statuses: &[P, W, F, U, K], profiles: ONBOARDING, run: checks::onboarding::sync_proof },
];

/// The entry of `id`.
pub fn entry(id: CheckId) -> Option<&'static CheckEntry> {
    ENTRIES.iter().find(|entry| entry.id == id.as_str())
}
