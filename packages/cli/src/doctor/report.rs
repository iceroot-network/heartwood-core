//! The report: findings with a status, a basis, typed facts, advice by key and untrusted text,
//! and the exit code.
//!
//! No text from a log, a peer or a file reaches a title, a status, a fact or an advice line:
//! [`Advice`] holds a fixed key into the check documentation and typed values, and the only
//! text-bearing fact, [`Fact::Id`], takes only values that pass their grammar ([`Id::new`]).
//! Everything else from outside the host goes to [`Untrusted`], which the renderer cleans, redacts
//! and caps.

use std::net::IpAddr;
use std::path::PathBuf;

use crate::doctor::registry::CheckId;
use crate::doctor::seams::{BuildFacts, Subject};
use crate::doctor::view::{Source, Unknown};
use crate::doctor::{DoctorProfile, Redaction};
use crate::seams::Role;

/// The report format, in JSON's `format`.
pub const FORMAT: &str = "heartwood-doctor-report/1";

/// A finding's status. The order is the text's: fails first, then warns, unknowns, passes and
/// skips.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Status {
    /// Broken now, or at the next start.
    Fail,
    /// Works now, needs attention.
    Warn,
    /// A source could not be read or timed out; the advice says which and why.
    Unknown,
    /// Fine.
    Pass,
    /// The check does not apply here.
    Skipped,
}

impl Status {
    /// Every status, in the report's order.
    pub const ALL: [Status; 5] = [
        Status::Fail,
        Status::Warn,
        Status::Unknown,
        Status::Pass,
        Status::Skipped,
    ];

    /// The status's name in the report.
    pub const fn name(self) -> &'static str {
        match self {
            Status::Pass => "pass",
            Status::Warn => "warn",
            Status::Fail => "fail",
            Status::Unknown => "unknown",
            Status::Skipped => "skipped",
        }
    }
}

/// What a finding rests on. A finding whose basis is [`Basis::Log`] rests on log lines only,
/// which other parties can write into, so it never gives disruptive advice as a fail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Basis {
    /// The node's log lines.
    Log,
    /// The chain: the store or the API's blocks.
    Chain,
    /// The process table.
    Process,
    /// The socket table.
    Socket,
    /// The node's configuration.
    Config,
    /// The system: file metadata, `/proc`, `/sys`, the probes.
    System,
}

impl Basis {
    /// The basis's name in the report.
    pub const fn name(self) -> &'static str {
        match self {
            Basis::Log => "log",
            Basis::Chain => "chain",
            Basis::Process => "process",
            Basis::Socket => "socket",
            Basis::Config => "config",
            Basis::System => "system",
        }
    }
}

/// The grammar an [`Id`] passed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum IdKind {
    /// 64 lowercase hex digits: a block id or a nethash.
    Hex64,
    /// 66 lowercase hex digits starting `02` or `03`: a compressed public key.
    PublicKey,
    /// A delegate name: 1 to 20 of `a-z`, `0-9` and `!@$&_.`, not starting with `_`, not only
    /// digits (the chain's rule).
    Username,
    /// A variable name: 1 to 64 of `A-Z`, `0-9` and `_`.
    Variable,
    /// A version: `<major>.<minor>.<patch>` of at most 9 digits each, then optionally `-` and at
    /// most 64 of `0-9A-Za-z.-`.
    Version,
    /// A token or network name: 1 to 64 letters, digits, `-` and `_`.
    Name,
}

/// A text value that passed its grammar: the only text a fact can hold.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Id {
    kind: IdKind,
    text: String,
}

impl Id {
    /// `text` as an id of `kind`; `None` when it does not pass the kind's grammar.
    pub fn new(kind: IdKind, text: &str) -> Option<Id> {
        let bytes = text.as_bytes();
        let hex = |len: usize| {
            bytes.len() == len && bytes.iter().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        };
        let valid = match kind {
            IdKind::Hex64 => hex(64),
            IdKind::PublicKey => hex(66) && (text.starts_with("02") || text.starts_with("03")),
            IdKind::Username => {
                (1..=20).contains(&bytes.len())
                    && !text.starts_with('_')
                    && bytes.iter().all(|b| {
                        matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'!' | b'@' | b'$' | b'&' | b'_' | b'.')
                    })
                    && bytes.iter().any(|b| !b.is_ascii_digit())
            }
            IdKind::Variable => {
                (1..=64).contains(&bytes.len())
                    && bytes.iter().all(|b| matches!(b, b'A'..=b'Z' | b'0'..=b'9' | b'_'))
            }
            IdKind::Version => valid_version(text),
            IdKind::Name => {
                (1..=64).contains(&bytes.len())
                    && bytes.iter().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
            }
        };
        valid.then(|| Id {
            kind,
            text: text.to_owned(),
        })
    }

    /// The grammar it passed.
    pub fn kind(&self) -> IdKind {
        self.kind
    }

    /// The text.
    pub fn as_str(&self) -> &str {
        &self.text
    }
}

/// Whether `text` is `<major>.<minor>.<patch>[-<pre>]` as [`IdKind::Version`] says.
fn valid_version(text: &str) -> bool {
    let (core, pre) = match text.split_once('-') {
        Some((core, pre)) => (core, Some(pre)),
        None => (text, None),
    };
    let parts: Vec<&str> = core.split('.').collect();
    let numbers = parts.len() == 3
        && parts
            .iter()
            .all(|p| (1..=9).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_digit()));
    let pre_ok = pre.is_none_or(|pre| {
        (1..=64).contains(&pre.len())
            && pre
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-'))
    });
    numbers && pre_ok
}

/// A typed value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fact {
    /// A count or another integer.
    Int(i64),
    /// A yes or no.
    Bool(bool),
    /// A fixed word from the doctor's own code.
    Word(&'static str),
    /// A text that passed its grammar.
    Id(Id),
    /// An IP address: redacted to a label and its class in the report.
    Address(IpAddr),
    /// A port.
    Port(u16),
    /// A path: the home directory, user and host names are redacted in the report.
    Path(PathBuf),
    /// A permission mode, printed in octal.
    Mode(u32),
    /// A duration in milliseconds.
    Millis(i64),
    /// A size in bytes.
    Bytes(u64),
    /// A time, in Unix milliseconds, printed in UTC.
    Time(i64),
}

/// A finding's advice: a key into its check's documentation (or a shared key of this module) and
/// the typed values its template names. The renderer fills the template; no runtime text is ever
/// advice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Advice {
    /// The template's key.
    pub key: &'static str,
    /// The template's values, by name.
    pub values: Vec<(&'static str, Fact)>,
}

impl Advice {
    /// The advice `key` with no values.
    pub const fn key(key: &'static str) -> Advice {
        Advice {
            key,
            values: Vec::new(),
        }
    }
}

/// The shared advice keys, whose templates are in `docs/common.json`.
pub mod keys {
    /// The check is not built yet.
    pub const NOT_BUILT: &str = "not-built";
    /// A source could not be read: values `source` and `reason` (words).
    pub const UNKNOWN: &str = "unknown";
    /// The profile leaves the check out.
    pub const SKIPPED_PROFILE: &str = "skipped-profile";
    /// `--role` leaves the check out.
    pub const SKIPPED_ROLE: &str = "skipped-role";
    /// The role the check is about does not run on this host.
    pub const SKIPPED_ABSENT: &str = "skipped-absent";
    /// The check comes with a later release.
    pub const SKIPPED_RELEASE: &str = "skipped-release";
}

/// Where untrusted text came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum UntrustedSource {
    /// A peer's version, from the API's peer list.
    PeerVersion,
    /// A line of a role's log.
    Log(Role),
    /// A line of a role's error capture.
    ErrorCapture(Role),
    /// One of the relay's start warnings, from its loader.
    ConfigWarning,
}

/// Text from a log, a peer or a file: data, never instructions. The report keeps it as read
/// (capped at 512 bytes when collected); the renderer removes control characters, escape
/// sequences and bidirectional marks, redacts addresses and long hex runs, and caps each excerpt
/// at 160 characters and 20 per report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Untrusted {
    /// Where it came from.
    pub source: UntrustedSource,
    /// When it was logged, in Unix milliseconds, when known.
    pub at_ms: Option<i64>,
    /// The text.
    pub text: String,
}

/// One finding of one check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// The check.
    pub id: CheckId,
    /// What it is about.
    pub subject: Subject,
    /// Its status.
    pub status: Status,
    /// What it rests on.
    pub basis: Basis,
    /// Typed facts, by name, in the check's order.
    pub facts: Vec<(&'static str, Fact)>,
    /// The advice.
    pub advice: Advice,
    /// Untrusted text the finding refers to; the renderer numbers it.
    pub untrusted: Vec<Untrusted>,
}

impl Finding {
    /// A finding with no facts and no untrusted text.
    pub fn new(
        id: CheckId,
        subject: Subject,
        status: Status,
        basis: Basis,
        advice: Advice,
    ) -> Self {
        Finding {
            id,
            subject,
            status,
            basis,
            facts: Vec::new(),
            advice,
            untrusted: Vec::new(),
        }
    }

    /// The `unknown` finding of `id` about `subject` for a source that gave `unknown`.
    pub fn unknown(id: CheckId, subject: Subject, unknown: &Unknown) -> Finding {
        let values = vec![
            ("source", Fact::Word(unknown.source.name())),
            ("reason", Fact::Word(unknown.reason.name())),
        ];
        let advice = Advice {
            key: keys::UNKNOWN,
            values,
        };
        Finding::new(
            id,
            subject,
            Status::Unknown,
            basis_of(unknown.source),
            advice,
        )
    }

    /// A `skipped` finding with the shared advice `key`.
    pub fn skipped(id: CheckId, subject: Subject, key: &'static str) -> Finding {
        Finding::new(
            id,
            subject,
            Status::Skipped,
            Basis::Config,
            Advice::key(key),
        )
    }
}

/// The basis of what `source` gives.
pub const fn basis_of(source: Source) -> Basis {
    match source {
        Source::RelayLog | Source::ForgerLog | Source::ErrorCapture => Basis::Log,
        Source::Store | Source::Api | Source::SigningRecord => Basis::Chain,
        Source::Process => Basis::Process,
        Source::Sockets => Basis::Socket,
        Source::Config | Source::Build => Basis::Config,
        Source::Files | Source::KeyFiles | Source::Proc | Source::Sys | Source::Probes => {
            Basis::System
        }
    }
}

/// The host, in the report's `host`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HostFacts {
    /// The operating system (`linux`).
    pub os: &'static str,
    /// The architecture (`x86_64`).
    pub arch: &'static str,
    /// The kernel's release, `<major>.<minor>`, when readable.
    pub kernel: Option<String>,
    /// The CPUs available to the doctor.
    pub cpus: Option<u32>,
    /// The host's memory, in bytes.
    pub memory_bytes: Option<u64>,
}

/// What the renderer redacts: never printed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HostIdentity {
    /// The home directory (printed as `~`).
    pub home: Option<PathBuf>,
    /// The user's name, from the environment (printed as `user`).
    pub user: Option<String>,
    /// The host's name (printed as `host`).
    pub host_name: Option<String>,
}

/// What the report is about, in its `scope`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportScope {
    /// The token.
    pub token: String,
    /// The network.
    pub network: String,
    /// The roles present on this host.
    pub roles: Vec<Role>,
    /// The profile.
    pub profile: DoctorProfile,
    /// The window, in minutes.
    pub window_minutes: u32,
}

/// One run's report: a pure function of what the sources gave and the clock at the run's start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    /// The clock at the run's start, in Unix milliseconds.
    pub created_at_ms: i64,
    /// This program's build.
    pub tool: BuildFacts,
    /// What was examined.
    pub scope: ReportScope,
    /// The redactions asked for (the JSON report and the MCP server always redact addresses).
    pub redaction: Redaction,
    /// The host.
    pub host: HostFacts,
    /// What the renderer redacts.
    pub identity: HostIdentity,
    /// The findings, in the registry's order.
    pub findings: Vec<Finding>,
}

/// The count of each status, and the worst.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Summary {
    /// Passes.
    pub pass: u32,
    /// Warns.
    pub warn: u32,
    /// Fails.
    pub fail: u32,
    /// Unknowns.
    pub unknown: u32,
    /// Skips.
    pub skipped: u32,
    /// The worst of fail, warn and pass (unknown and skipped do not count).
    pub worst: Status,
}

impl Report {
    /// The report's summary.
    pub fn summary(&self) -> Summary {
        let count = |status: Status| {
            let n = self.findings.iter().filter(|f| f.status == status).count();
            u32::try_from(n).unwrap_or(u32::MAX)
        };
        let (fail, warn) = (count(Status::Fail), count(Status::Warn));
        let worst = match (fail, warn) {
            (0, 0) => Status::Pass,
            (0, _) => Status::Warn,
            _ => Status::Fail,
        };
        Summary {
            pass: count(Status::Pass),
            warn,
            fail,
            unknown: count(Status::Unknown),
            skipped: count(Status::Skipped),
            worst,
        }
    }
}

/// Which statuses make the exit code non-zero.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum FailOn {
    /// A warn gives 1 and a fail 2 (the default).
    #[default]
    Warn,
    /// Only a fail counts: a warn gives 0.
    Fail,
}

/// The exit code of a report: 0 with no warn and no fail (unknown and skipped do not count), 1
/// with a warn and no fail (0 under [`FailOn::Fail`]), 2 with a fail. Code 3, "the doctor could
/// not run", is the command's.
pub fn exit_code(report: &Report, fail_on: FailOn) -> u8 {
    match (report.summary().worst, fail_on) {
        (Status::Fail, _) => 2,
        (Status::Warn, FailOn::Warn) => 1,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doctor::registry::CheckId;

    fn report(statuses: &[Status]) -> Report {
        let id = CheckId::parse("keys.file").unwrap();
        let finding = |status: &Status| {
            Finding::new(
                id,
                Subject::Forger,
                *status,
                Basis::System,
                Advice::key("x"),
            )
        };
        Report {
            created_at_ms: 0,
            tool: BuildFacts {
                version: "4.3.1-heartwood.0.1.0",
                features: vec![],
            },
            scope: ReportScope {
                token: "solar".into(),
                network: "testnet".into(),
                roles: vec![],
                profile: DoctorProfile::Default,
                window_minutes: 60,
            },
            redaction: Redaction::default(),
            host: HostFacts::default(),
            identity: HostIdentity::default(),
            findings: statuses.iter().map(finding).collect(),
        }
    }

    #[test]
    fn the_exit_code_counts_warns_and_fails_only() {
        use Status::*;
        let cases: [(&[Status], u8, u8); 6] = [
            (&[], 0, 0),
            (&[Pass, Unknown, Skipped], 0, 0),
            (&[Pass, Warn], 1, 0),
            (&[Warn, Unknown], 1, 0),
            (&[Fail], 2, 2),
            (&[Warn, Fail, Pass], 2, 2),
        ];
        for (statuses, on_warn, on_fail) in cases {
            let report = report(statuses);
            assert_eq!(exit_code(&report, FailOn::Warn), on_warn, "{statuses:?}");
            assert_eq!(exit_code(&report, FailOn::Fail), on_fail, "{statuses:?}");
        }
    }

    #[test]
    fn an_id_is_only_a_text_that_passes_its_grammar() {
        let hex = "ab".repeat(32);
        let key = format!("02{}", "cd".repeat(32));
        let valid = [
            (IdKind::Hex64, hex.as_str()),
            (IdKind::PublicKey, key.as_str()),
            (IdKind::Username, "genesis_1"),
            (IdKind::Username, "a.b!c@d$e&f"),
            (IdKind::Variable, "CORE_API_RATE_LIMIT_WHITELIST"),
            (IdKind::Version, "4.3.1-heartwood.0.3.0"),
            (IdKind::Name, "node1"),
        ];
        for (kind, text) in valid {
            assert_eq!(
                Id::new(kind, text).map(|id| id.as_str().to_owned()),
                Some(text.into())
            );
        }
        let upper = hex.to_uppercase();
        let long = "a".repeat(21);
        let invalid = [
            (IdKind::Hex64, upper.as_str()),
            (IdKind::Hex64, "ab"),
            (IdKind::PublicKey, hex.as_str()),
            (IdKind::Username, "_genesis"),
            (IdKind::Username, "12345"),
            (IdKind::Username, long.as_str()),
            (IdKind::Username, "Genesis"),
            (IdKind::Username, "a b"),
            (IdKind::Username, "a\nb"),
            (IdKind::Variable, "core_api"),
            (IdKind::Version, "4.3"),
            (IdKind::Version, "4.3.1-evil version"),
            (IdKind::Name, "a/b"),
            (IdKind::Name, ""),
        ];
        for (kind, text) in invalid {
            assert_eq!(Id::new(kind, text), None, "{kind:?} {text:?}");
        }
    }
}
