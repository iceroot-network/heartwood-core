//! `heartwood doctor`: read-only diagnostics of one instance, its check list, one check's
//! documentation, and the read-only MCP server.
//!
//! ```text
//! heartwood doctor [--token T] [--network N] [--role relay|forger|all] [--check ID]...
//!                  [--profile default|onboarding] [--window 60m] [--delegate NAME]... [--json]
//!                  [--show-addresses] [--hide-identity] [--fail-on warn|fail]
//! heartwood doctor checks [--json]
//! heartwood doctor explain ID
//! heartwood doctor mcp [--token T] [--network N]
//! ```
//!
//! The doctor parses its own command line, so that a bad flag exits with 3, "the doctor could not
//! run", as a missing instance does: 0, 1 and 2 are the report's (no warn or fail; a warn; a
//! fail). `--show-addresses` changes the text only: the JSON report always redacts addresses.

use std::ffi::OsString;
use std::io::{IsTerminal, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use clap::{Args, Parser, Subcommand, ValueEnum};
use heartwood_cli::doctor::redact::Redactor;
use heartwood_cli::doctor::registry::{CheckId, ENTRIES};
use heartwood_cli::doctor::report::{FailOn, HostIdentity, Status, exit_code};
use heartwood_cli::doctor::{
    DoctorOptions, DoctorProfile, DoctorScope, HostWorld, MAX_WINDOW, MIN_WINDOW, Redaction,
    RoleSelection, docs, mcp, render,
};
use heartwood_cli::instance::InstanceFlags;
use heartwood_cli::{InstanceError, Role};
use serde_json::json;

use crate::doctor_view::CoreNodeView;

/// The exit code when the doctor could not run.
pub const CANNOT_RUN: u8 = 3;

/// The words after `heartwood doctor`, which [`parse`] reads.
#[derive(Debug, Clone, Default, PartialEq, Eq, Args)]
#[command(disable_help_flag = true)]
pub struct DoctorLine {
    /// The doctor's flags and subcommand: see `heartwood doctor --help`.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, num_args = 0..)]
    pub args: Vec<OsString>,
}

/// The doctor's command line.
#[derive(Debug, Clone, PartialEq, Eq, Parser)]
#[command(
    name = "heartwood doctor",
    about = "Read-only diagnostics of this host's node",
    args_conflicts_with_subcommands = true,
    disable_version_flag = true
)]
pub struct Doctor {
    /// A subcommand; none runs the checks.
    #[command(subcommand)]
    pub command: Option<DoctorCommand>,
    /// The run's flags.
    #[command(flatten)]
    pub run: RunFlags,
}

/// A subcommand of `heartwood doctor`.
#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
pub enum DoctorCommand {
    /// List every check with its area and sources.
    Checks(ChecksFlags),
    /// Print one check's documentation.
    Explain(ExplainFlags),
    /// Serve the read-only MCP interface over standard input and output.
    Mcp(McpFlags),
}

/// The roles `--role` names.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, ValueEnum)]
pub enum RoleArg {
    /// The relay.
    Relay,
    /// The forger.
    Forger,
    /// Every role present on this host.
    #[default]
    All,
}

/// The profiles `--profile` names.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, ValueEnum)]
pub enum ProfileArg {
    /// Every check of a running node.
    #[default]
    Default,
    /// The default checks and the sync check of an application.
    Onboarding,
}

/// The statuses `--fail-on` names.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, ValueEnum)]
pub enum FailOnArg {
    /// A warn exits 1, a fail 2.
    #[default]
    Warn,
    /// Only a fail exits non-zero (2).
    Fail,
}

/// The flags of a run.
#[derive(Debug, Clone, Default, PartialEq, Eq, Args)]
pub struct RunFlags {
    /// The instance.
    #[command(flatten)]
    pub instance: InstanceFlags,
    /// The roles examined.
    #[arg(long, value_enum, default_value_t)]
    pub role: RoleArg,
    /// Run only this check (repeatable).
    #[arg(long = "check", value_parser = check_id)]
    pub checks: Vec<CheckId>,
    /// The profile.
    #[arg(long, value_enum, default_value_t)]
    pub profile: ProfileArg,
    /// How far back the log and store checks look, from 5m to 24h.
    #[arg(long, value_parser = window, default_value = "60m")]
    pub window: Duration,
    /// A delegate of ours (repeatable).
    #[arg(long = "delegate")]
    pub delegates: Vec<String>,
    /// Print the JSON report.
    #[arg(long)]
    pub json: bool,
    /// Show IP addresses and host names in the text (the JSON report always redacts them).
    #[arg(long)]
    pub show_addresses: bool,
    /// Replace delegate names and public keys by labels.
    #[arg(long)]
    pub hide_identity: bool,
    /// Which statuses make the exit code non-zero.
    #[arg(long, value_enum, default_value_t)]
    pub fail_on: FailOnArg,
}

/// The flags of `checks`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Args)]
pub struct ChecksFlags {
    /// Print JSON.
    #[arg(long)]
    pub json: bool,
}

/// The flags of `explain`.
#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct ExplainFlags {
    /// The check.
    #[arg(value_parser = check_id)]
    pub id: CheckId,
}

/// The flags of `mcp`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Args)]
pub struct McpFlags {
    /// The instance.
    #[command(flatten)]
    pub instance: InstanceFlags,
}

/// A check id of the registry.
fn check_id(text: &str) -> Result<CheckId, String> {
    CheckId::parse(text)
        .ok_or_else(|| format!("{text:?} is not a check: see heartwood doctor checks"))
}

/// A window such as `90m` or `2h`, from 5 minutes to 24 hours.
fn window(text: &str) -> Result<Duration, String> {
    let invalid = || format!("{text:?} is not a window from 5m to 24h, such as 60m or 2h");
    let (number, unit) = text.split_at(text.len().saturating_sub(1));
    let number: u64 = number.parse().map_err(|_| invalid())?;
    let seconds = match unit {
        "m" => number.checked_mul(60),
        "h" => number.checked_mul(3600),
        _ => None,
    };
    let window = Duration::from_secs(seconds.ok_or_else(invalid)?);
    let inside = (MIN_WINDOW..=MAX_WINDOW).contains(&window);
    inside.then_some(window).ok_or_else(invalid)
}

/// The doctor's command line from the words after `doctor`.
pub fn parse(args: &[OsString]) -> Result<Doctor, clap::Error> {
    let words = std::iter::once(OsString::from("heartwood doctor")).chain(args.iter().cloned());
    Doctor::try_parse_from(words)
}

/// Run `heartwood doctor` with the words of `line`.
pub fn run(line: DoctorLine) -> ExitCode {
    let doctor = match parse(&line.args) {
        Ok(doctor) => doctor,
        Err(error) => {
            let _ = error.print();
            let code = if error.use_stderr() { CANNOT_RUN } else { 0 };
            return ExitCode::from(code);
        }
    };
    match doctor.command {
        None => run_checks(&doctor.run),
        Some(DoctorCommand::Checks(flags)) => list(flags.json),
        Some(DoctorCommand::Explain(flags)) => explain(flags.id),
        Some(DoctorCommand::Mcp(flags)) => serve(&flags.instance),
    }
}

/// Write `text` to standard output; a closed output (a pipe whose reader left) is not an error of
/// the doctor's.
fn out(text: &str) {
    let _ = std::io::stdout().lock().write_all(text.as_bytes());
}

/// The message of `error` on standard error, and exit code 3.
fn cannot_run(error: impl std::fmt::Display) -> ExitCode {
    let _ = writeln!(std::io::stderr().lock(), "heartwood doctor: {error}");
    ExitCode::from(CANNOT_RUN)
}

/// The scope of the instance `flags` name, resolved as every role command resolves it: from the
/// `HOME` variable, which `relay run` and `forger run` require too (the home is never looked up
/// through the system's name services), and `USER` for the redaction.
fn scope(flags: &InstanceFlags) -> Result<DoctorScope, String> {
    let env = |name: &str| std::env::var(name).ok();
    let home = std::env::var_os("HOME").filter(|home| !home.is_empty());
    let home = PathBuf::from(home.ok_or("HOME is not set")?);
    let cwd = std::env::current_dir().unwrap_or_default();
    let instance = heartwood_cli::instance::resolve(Role::Relay, flags, &env, &home, &cwd);
    let identity = HostIdentity {
        home: Some(home),
        user: env("USER"),
        host_name: None,
    };
    let instance = instance.map_err(|error| instance_error(&error, &identity))?;
    Ok(DoctorScope::of(&instance, identity.home, identity.user))
}

/// `error` as the doctor prints it: cleaned and redacted as the report redacts untrusted text,
/// the home directory as `~` and the user's name as `user` (standard error reaches an SSH client
/// too).
fn instance_error(error: &InstanceError, identity: &HostIdentity) -> String {
    Redactor::new(identity, true).untrusted(&error.to_string())
}

fn run_checks(flags: &RunFlags) -> ExitCode {
    if let Err(error) = docs::docs() {
        return cannot_run(error);
    }
    let scope = match scope(&flags.instance) {
        Ok(scope) => scope,
        Err(error) => return cannot_run(error),
    };
    let node = CoreNodeView::from_process();
    let world = HostWorld::new(&scope, &node);
    let options = DoctorOptions {
        roles: match flags.role {
            RoleArg::Relay => RoleSelection::Relay,
            RoleArg::Forger => RoleSelection::Forger,
            RoleArg::All => RoleSelection::All,
        },
        checks: flags.checks.clone(),
        profile: match flags.profile {
            ProfileArg::Default => DoctorProfile::Default,
            ProfileArg::Onboarding => DoctorProfile::Onboarding,
        },
        window: flags.window,
        delegates: flags.delegates.clone(),
        redaction: Redaction {
            addresses: !flags.show_addresses,
            identity: flags.hide_identity,
        },
        budget: heartwood_cli::doctor::RUN_BUDGET,
    };
    let report = heartwood_cli::doctor::run(&world, &scope, &options);
    let text = match flags.json {
        true => render::render_json(&report),
        false => render::render_text(&report, std::io::stdout().is_terminal()),
    };
    out(&text);
    let fail_on = match flags.fail_on {
        FailOnArg::Warn => FailOn::Warn,
        FailOnArg::Fail => FailOn::Fail,
    };
    ExitCode::from(exit_code(&report, fail_on))
}

fn list(as_json: bool) -> ExitCode {
    let docs = match docs::docs() {
        Ok(docs) => docs,
        Err(error) => return cannot_run(error),
    };
    let entries = ENTRIES
        .iter()
        .filter_map(|entry| docs.check(entry.check_id()));
    if as_json {
        let checks: Vec<_> = entries
            .map(|doc| {
                json!({"id": doc.id, "title": doc.title, "area": doc.area,
                    "subjects": doc.subjects, "sources": doc.sources, "docs": doc.docs})
            })
            .collect();
        out(&format!("{}\n", json!({ "checks": checks })));
    } else {
        let lines = entries.map(|doc| format!("{:<26} {:<11} {}\n", doc.id, doc.area, doc.title));
        out(&lines.collect::<String>());
    }
    ExitCode::SUCCESS
}

fn explain(id: CheckId) -> ExitCode {
    let doc = docs::docs().map(|docs| docs.check(id).cloned());
    let doc = match doc {
        Ok(Some(doc)) => doc,
        Ok(None) => return cannot_run(format!("{} has no documentation", id.as_str())),
        Err(error) => return cannot_run(error),
    };
    let mut text = format!("{}: {}\n", doc.id, doc.title);
    let about = doc.subjects.join(", ");
    text.push_str(&format!("Area: {}. About: {about}.\n", doc.area));
    text.push_str(&format!("Reads: {}.\n", doc.sources.join(", ")));
    for status in Status::ALL.map(Status::name) {
        if let Some(rule) = doc.rules.get(status) {
            text.push_str(&format!("{status}: {rule}\n"));
        }
    }
    for (key, template) in &doc.advice {
        text.push_str(&format!("Advice ({key}): {template}\n"));
    }
    text.push_str(&format!("More: {}\n", doc.docs));
    out(&text);
    ExitCode::SUCCESS
}

fn serve(flags: &InstanceFlags) -> ExitCode {
    let scope = match scope(flags) {
        Ok(scope) => scope,
        Err(error) => return cannot_run(error),
    };
    let node = CoreNodeView::from_process();
    let world = HostWorld::new(&scope, &node);
    let (input, output) = (std::io::stdin().lock(), std::io::stdout().lock());
    match mcp::serve(input, output, &world, &scope) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => cannot_run(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(text: &str) -> Result<Doctor, clap::Error> {
        let words: Vec<OsString> = text.split_whitespace().map(OsString::from).collect();
        parse(&words)
    }

    #[test]
    fn the_flags_and_subcommands_parse_and_a_bad_one_is_an_error() {
        let run =
            line("--network testnet --role forger --check keys.file --window 2h --json").unwrap();
        assert_eq!(run.command, None);
        assert_eq!(run.run.role, RoleArg::Forger);
        assert_eq!(run.run.window, Duration::from_secs(7200));
        assert_eq!(run.run.checks, [CheckId::parse("keys.file").unwrap()]);
        assert!(run.run.json);
        assert_eq!(
            line("").unwrap().run.window,
            heartwood_cli::doctor::DEFAULT_WINDOW
        );
        let explain = line("explain slots.missed").unwrap().command;
        let id = CheckId::parse("slots.missed").unwrap();
        assert_eq!(explain, Some(DoctorCommand::Explain(ExplainFlags { id })));
        assert!(matches!(
            line("checks --json").unwrap().command,
            Some(DoctorCommand::Checks(ChecksFlags { json: true }))
        ));
        assert!(matches!(
            line("mcp --network testnet").unwrap().command,
            Some(DoctorCommand::Mcp(_))
        ));
        for bad in [
            "--bogus",
            "--check no.such-check",
            "--window 4m",
            "--window 25h",
            "--window 60",
            "--role core",
            "--fail-on unknown",
            "explain",
            "checks --network testnet",
        ] {
            let error = line(bad).unwrap_err();
            assert!(error.use_stderr(), "{bad}");
        }
        assert!(!line("--help").unwrap_err().use_stderr());
    }

    #[test]
    fn an_instance_error_prints_no_home_directory_and_no_user_name() {
        let identity = HostIdentity {
            home: Some(PathBuf::from("/srv/fakehome-secretname")),
            user: Some("secretname".to_owned()),
            host_name: None,
        };
        let config = PathBuf::from("/srv/fakehome-secretname/.config/solar-core/config.json");
        let errors = [
            InstanceError::MissingNetwork {
                config: config.clone(),
            },
            InstanceError::MissingToken {
                config: config.clone(),
            },
            InstanceError::Config {
                path: PathBuf::from("/srv/secretname/config.json"),
                reason: "expected value at /srv/fakehome-secretname\u{1b}[2J".to_owned(),
            },
        ];
        let printed: Vec<String> = errors
            .iter()
            .map(|e| instance_error(e, &identity))
            .collect();
        assert_eq!(
            printed,
            [
                "no network: pass --network or set it in ~/.config/solar-core/config.json",
                "no token: pass --token or set it in ~/.config/solar-core/config.json",
                "/srv/user/config.json is not valid: expected value at ~",
            ]
        );
    }

    #[test]
    fn the_command_tree_hands_every_word_after_doctor_to_the_doctors_parser() {
        let words = "heartwood doctor --network testnet --json".split_whitespace();
        let command = crate::commands::parse(words.map(OsString::from).collect()).unwrap();
        let args = ["--network", "testnet", "--json"]
            .map(OsString::from)
            .to_vec();
        assert_eq!(
            command,
            crate::commands::Command::Doctor(DoctorLine { args })
        );
        let colon = "heartwood doctor:explain keys.file".split_whitespace();
        let command = crate::commands::parse(colon.map(OsString::from).collect()).unwrap();
        let args = ["explain", "keys.file"].map(OsString::from).to_vec();
        assert_eq!(
            command,
            crate::commands::Command::Doctor(DoctorLine { args })
        );
    }
}
