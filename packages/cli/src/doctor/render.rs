//! The text and the JSON report.
//!
//! The JSON report (`heartwood-doctor-report/1`) has its keys in a fixed order, its findings in
//! the registry's order, times in UTC, and addresses always redacted: it is the export an operator
//! can share. The text lists fails first, then warns, unknowns, passes and skips, each in the
//! registry's order; `--show-addresses` changes the text only.

use serde_json::{Map, Value, json};

use heartwood_kernel::timestamp::UtcDateTime;

use crate::doctor::docs::{self, DOCS_PAGE};
use crate::doctor::redact::Redactor;
use crate::doctor::report::{FORMAT, Fact, Finding, Report, Status};

/// The report as text, with colour when `colour` is set (a terminal).
pub fn render_text(report: &Report, colour: bool) -> String {
    let _ = colour;
    let mut redactor = Redactor::new(&report.identity, report.redaction.addresses);
    let roles: Vec<&str> = report.scope.roles.iter().map(|role| role.name()).collect();
    let mut out = format!(
        "heartwood doctor: {}-{} ({}), {}, last {} minutes\n",
        report.scope.token,
        report.scope.network,
        roles.join(", "),
        text_time(report.created_at_ms),
        report.scope.window_minutes,
    );
    for status in Status::ALL {
        for finding in report.findings.iter().filter(|f| f.status == status) {
            let advice = advice(finding, &mut redactor);
            let (name, id) = (status.name().to_ascii_uppercase(), finding.id.as_str());
            out.push_str(&format!("{name:<7} {id:<26} {advice}\n"));
        }
    }
    let s = report.summary();
    out.push_str(&format!(
        "Summary: {} fail, {} warn, {} pass, {} unknown, {} skipped.  More: heartwood doctor explain <check>\n",
        s.fail, s.warn, s.pass, s.unknown, s.skipped
    ));
    out
}

/// The JSON report, addresses always redacted, one line and a newline.
pub fn render_json(report: &Report) -> String {
    let mut redactor = Redactor::new(&report.identity, true);
    let mut untrusted = Vec::new();
    let findings: Vec<Value> = report
        .findings
        .iter()
        .map(|finding| {
            let mut facts = Map::new();
            for (name, fact) in &finding.facts {
                facts.insert((*name).to_owned(), fact_value(fact, &mut redactor));
            }
            let mut ids = Vec::new();
            for excerpt in &finding.untrusted {
                let id = untrusted.len().saturating_add(1);
                ids.push(json!(id));
                untrusted.push(json!({
                    "id": id,
                    "source": source_name(excerpt.source),
                    "at": excerpt.at_ms.map(time),
                    "text": redactor.untrusted(&excerpt.text),
                }));
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
    let value = json!({
        "format": FORMAT,
        "createdAt": time(report.created_at_ms),
        "tool": {"version": report.tool.version, "features": report.tool.features},
        "scope": {
            "instance": format!("{}-{}", report.scope.token, report.scope.network),
            "network": report.scope.network,
            "roles": report.scope.roles.iter().map(|role| role.name()).collect::<Vec<_>>(),
            "profile": report.scope.profile.name(),
            "windowMinutes": report.scope.window_minutes,
        },
        "redaction": {"addresses": true, "identity": report.redaction.identity},
        "host": {
            "os": report.host.os,
            "arch": report.host.arch,
            "kernel": report.host.kernel,
            "cpus": report.host.cpus,
            "memoryBytes": report.host.memory_bytes,
        },
        "summary": {
            "pass": s.pass, "warn": s.warn, "fail": s.fail, "unknown": s.unknown,
            "skipped": s.skipped, "worst": s.worst.name(),
        },
        "findings": findings,
        "untrusted": untrusted,
    });
    format!("{value}\n")
}

/// A time as `YYYY-MM-DD HH:MM:SS UTC`.
fn text_time(unix_ms: i64) -> String {
    let iso = time(unix_ms);
    format!("{} UTC", iso.get(..19).unwrap_or(&iso).replace('T', " "))
}

/// A time as `YYYY-MM-DDTHH:MM:SS.mmmZ`.
fn time(unix_ms: i64) -> String {
    UtcDateTime::from_unix_ms(unix_ms).map_or_else(String::new, |t| t.to_iso_string())
}

fn source_name(source: crate::doctor::report::UntrustedSource) -> String {
    use crate::doctor::report::UntrustedSource as U;
    match source {
        U::PeerVersion => "peer version".into(),
        U::Log(role) => format!("{role} log"),
        U::ErrorCapture(role) => format!("{role} error capture"),
        U::ConfigWarning => "configuration warning".into(),
    }
}

fn fact_value(fact: &Fact, redactor: &mut Redactor<'_>) -> Value {
    match fact {
        Fact::Int(n) | Fact::Millis(n) => json!(n),
        Fact::Bool(b) => json!(b),
        Fact::Word(word) => json!(word),
        Fact::Id(id) => json!(id.as_str()),
        Fact::Address(ip) => json!(redactor.address(*ip)),
        Fact::Port(port) => json!(port),
        Fact::Path(path) => json!(redactor.path(path)),
        Fact::Mode(mode) => json!(format!("{mode:04o}")),
        Fact::Bytes(n) => json!(n),
        Fact::Time(ms) => json!(time(*ms)),
    }
}

fn fact_text(fact: &Fact, redactor: &mut Redactor<'_>) -> String {
    match fact_value(fact, redactor) {
        Value::String(text) => text,
        other => other.to_string(),
    }
}

/// A finding's advice: its template with its values filled in.
fn advice(finding: &Finding, redactor: &mut Redactor<'_>) -> String {
    let template = docs::docs()
        .ok()
        .and_then(|docs| docs.template(finding.id, finding.advice.key));
    let mut text = template.unwrap_or(finding.advice.key).to_owned();
    for (name, value) in &finding.advice.values {
        text = text.replace(&format!("{{{name}}}"), &fact_text(value, redactor));
    }
    text
}
