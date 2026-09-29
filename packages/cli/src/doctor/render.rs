//! The text and the JSON report.
//!
//! The JSON report (`heartwood-doctor-report/1`, described by [`REPORT_SCHEMA`]) has its keys in
//! a fixed order, its findings in the registry's order, integers as integers, times in UTC, and
//! addresses always redacted: it is the export an operator can share. Untrusted text is listed
//! once, cleaned, redacted and capped, in the report's `untrusted`, and each finding names its
//! excerpts by id. The text lists fails first, then warns, unknowns, passes and skips, each in
//! the registry's order, with colour only on a terminal; `--show-addresses` changes the text
//! only. Both are pure functions of the report.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use serde_json::{Map, Value, json};

use heartwood_kernel::timestamp::UtcDateTime;

use crate::doctor::docs::{self, DOCS_PAGE};
use crate::doctor::redact::{MAX_EXCERPTS, Redactor, clean};
use crate::doctor::report::{FORMAT, Fact, Finding, Report, Status, UntrustedSource};

/// The JSON schema of the report, as published on the documentation site.
pub const REPORT_SCHEMA: &str = include_str!("report.schema.json");

/// The terminal colour of each status, in [`Status::ALL`]'s order.
const COLOURS: [&str; 5] = ["1;31", "1;33", "35", "32", "2"];
/// The width of the text's status column, and of its check column.
const STATUS_WIDTH: usize = 7;
const ID_WIDTH: usize = 25;

/// The report's untrusted excerpts: each distinct one once, numbered from 1 in the report's
/// order, at most [`MAX_EXCERPTS`]; and for each finding the numbers of its own.
struct Excerpts {
    list: Vec<Value>,
    by_finding: Vec<Vec<usize>>,
    omitted: usize,
}

impl Excerpts {
    fn of(report: &Report, redactor: &mut Redactor<'_>) -> Excerpts {
        let mut seen = BTreeMap::new();
        let (mut list, mut by_finding, mut omitted) = (Vec::new(), Vec::new(), 0usize);
        for finding in &report.findings {
            let mut ids = Vec::new();
            for excerpt in &finding.untrusted {
                let key = (excerpt.source, excerpt.at_ms, excerpt.text.as_str());
                if let Some(id) = seen.get(&key) {
                    ids.push(*id);
                    continue;
                }
                if list.len() == MAX_EXCERPTS {
                    omitted = omitted.saturating_add(1);
                    continue;
                }
                let id = list.len().saturating_add(1);
                seen.insert(key, id);
                ids.push(id);
                list.push(json!({
                    "id": id,
                    "source": source_name(excerpt.source),
                    "at": excerpt.at_ms.and_then(time),
                    "text": redactor.untrusted(&excerpt.text),
                }));
            }
            by_finding.push(ids);
        }
        Excerpts {
            list,
            by_finding,
            omitted,
        }
    }
}

/// The report as text, with colour when `colour` is set (a terminal).
pub fn render_text(report: &Report, colour: bool) -> String {
    let mut redactor = Redactor::for_report(report, report.redaction.addresses);
    let excerpts = Excerpts::of(report, &mut redactor);
    let roles: Vec<&str> = report.scope.roles.iter().map(|role| role.name()).collect();
    let roles = Some(roles.join(", ")).filter(|r| !r.is_empty());
    let mut out = format!(
        "heartwood doctor: {}-{} ({}), {}, last {} minutes{}\n",
        clean(&report.scope.token),
        clean(&report.scope.network),
        roles.as_deref().unwrap_or("no role found"),
        text_time(&time(report.created_at_ms).unwrap_or_default()),
        report.scope.window_minutes,
        match report.scope.profile.name() {
            "default" => String::new(),
            other => format!(", profile {other}"),
        },
    );
    let indent = " ".repeat(STATUS_WIDTH + ID_WIDTH + 2);
    for status in Status::ALL {
        let found = report.findings.iter().zip(&excerpts.by_finding);
        for (finding, ids) in found.filter(|(f, _)| f.status == status) {
            let name = status.name().to_ascii_uppercase();
            let code = Status::ALL.iter().position(|s| *s == status);
            let code = code.and_then(|n| COLOURS.get(n)).filter(|_| colour);
            let (on, off) = code.map_or((String::new(), ""), |c| {
                (format!("\u{1b}[{c}m"), "\u{1b}[0m")
            });
            let id = finding.id.as_str();
            let _ = write!(out, "{on}{name:<STATUS_WIDTH$}{off} {id:<ID_WIDTH$} ");
            let text = advice(finding, &mut redactor);
            let (subject, basis) = (finding.subject.name(), finding.basis.name());
            for (n, line) in text.split('\n').enumerate() {
                match n {
                    0 => writeln!(out, "{line} [{subject}, {basis}]"),
                    _ => writeln!(out, "{indent}{line}"),
                }
                .unwrap_or_default();
            }
            for excerpt in ids
                .iter()
                .filter_map(|id| excerpts.list.get(id.wrapping_sub(1)))
            {
                let at = excerpt["at"]
                    .as_str()
                    .map(|at| format!(", {}", text_time(at)));
                let _ = writeln!(
                    out,
                    "{indent}untrusted {} ({}{}): {}",
                    excerpt["id"],
                    excerpt["source"].as_str().unwrap_or_default(),
                    at.unwrap_or_default(),
                    excerpt["text"].as_str().unwrap_or_default(),
                );
            }
        }
    }
    let s = report.summary();
    let _ = writeln!(
        out,
        "Summary: {} fail, {} warn, {} pass, {} unknown, {} skipped.  More: heartwood doctor explain <check>",
        s.fail, s.warn, s.pass, s.unknown, s.skipped
    );
    out
}

/// The JSON report, addresses always redacted, on one line with a newline.
pub fn render_json(report: &Report) -> String {
    format!("{}\n", report_value(report))
}

/// The JSON report as a value, addresses always redacted.
pub fn report_value(report: &Report) -> Value {
    let mut redactor = Redactor::for_report(report, true);
    let excerpts = Excerpts::of(report, &mut redactor);
    let findings: Vec<Value> = report
        .findings
        .iter()
        .zip(&excerpts.by_finding)
        .map(|(finding, ids)| {
            let mut facts = Map::new();
            for (name, fact) in &finding.facts {
                facts.insert((*name).to_owned(), fact_value(fact, &mut redactor));
            }
            json!({
                "id": finding.id.as_str(),
                "subject": finding.subject.name(),
                "status": finding.status.name(),
                "basis": finding.basis.name(),
                "facts": facts,
                "advice": advice(finding, &mut redactor),
                "docs": format!("{DOCS_PAGE}#{}", finding.id.anchor()),
                "untrusted": ids,
            })
        })
        .collect();
    let s = report.summary();
    json!({
        "format": FORMAT,
        "createdAt": time(report.created_at_ms).unwrap_or_default(),
        "tool": {"version": report.tool.version, "features": report.tool.features},
        "scope": {
            "instance": clean(&format!("{}-{}", report.scope.token, report.scope.network)),
            "network": clean(&report.scope.network),
            "roles": report.scope.roles.iter().map(|role| role.name()).collect::<Vec<_>>(),
            "profile": report.scope.profile.name(),
            "windowMinutes": report.scope.window_minutes,
        },
        "redaction": {
            "addresses": true,
            "identity": report.redaction.identity,
            "untrustedOmitted": excerpts.omitted,
        },
        "host": {
            "os": report.host.os,
            "arch": report.host.arch,
            "kernel": report.host.kernel.as_deref().map(clean),
            "cpus": report.host.cpus,
            "memoryBytes": report.host.memory_bytes,
        },
        "summary": {
            "pass": s.pass, "warn": s.warn, "fail": s.fail, "unknown": s.unknown,
            "skipped": s.skipped, "worst": s.worst.name(),
        },
        "findings": findings,
        "untrusted": excerpts.list,
    })
}

/// An ISO time as `YYYY-MM-DD HH:MM:SS UTC`.
fn text_time(iso: &str) -> String {
    format!("{} UTC", iso.get(..19).unwrap_or(iso).replace('T', " "))
}

/// A time as `YYYY-MM-DDTHH:MM:SS.mmmZ`, when it has one.
fn time(unix_ms: i64) -> Option<String> {
    UtcDateTime::from_unix_ms(unix_ms).map(|t| t.to_iso_string())
}

/// Where untrusted text came from, as the report names it.
fn source_name(source: UntrustedSource) -> String {
    match source {
        UntrustedSource::PeerVersion => "peer version".into(),
        UntrustedSource::Log(role) => format!("{} log", role.name()),
        UntrustedSource::ErrorCapture(role) => format!("{} error capture", role.name()),
        UntrustedSource::ConfigWarning => "configuration warning".into(),
    }
}

fn fact_value(fact: &Fact, redactor: &mut Redactor<'_>) -> Value {
    match fact {
        Fact::Int(n) | Fact::Millis(n) => json!(n),
        Fact::Bool(b) => json!(b),
        Fact::Word(word) => json!(word),
        Fact::Id(id) => json!(redactor.id(id)),
        Fact::Address(ip) => json!(redactor.address(*ip)),
        Fact::Port(port) => json!(port),
        Fact::Path(path) => json!(redactor.path(path)),
        Fact::Mode(mode) => json!(format!("{mode:04o}")),
        Fact::Bytes(n) => json!(n),
        Fact::Time(ms) => json!(time(*ms)),
    }
}

/// A finding's advice: its template with its values filled in, redacted as the facts are.
fn advice(finding: &Finding, redactor: &mut Redactor<'_>) -> String {
    let template = docs::docs()
        .ok()
        .and_then(|docs| docs.template(finding.id, finding.advice.key));
    let mut text = template.unwrap_or(finding.advice.key).to_owned();
    for (name, value) in &finding.advice.values {
        let value = match fact_value(value, redactor) {
            Value::String(text) => text,
            Value::Null => "an unknown time".to_owned(),
            other => other.to_string(),
        };
        text = text.replace(&format!("{{{name}}}"), &value);
    }
    text
}
