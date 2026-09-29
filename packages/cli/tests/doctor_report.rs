//! The text and the JSON report: the order of the findings, colour, key order and types, the
//! untrusted excerpts, `--show-addresses`, and the exit code of every mix of statuses.

use std::net::IpAddr;
use std::path::PathBuf;

use serde_json::{Value, json};

use heartwood_cli::Role;
use heartwood_cli::doctor::registry::{CheckId, ENTRIES};
use heartwood_cli::doctor::render::{render_json, render_text};
use heartwood_cli::doctor::report::{
    Advice, Basis, Fact, FailOn, Finding, HostFacts, HostIdentity, Id, IdKind, Report, ReportScope,
    Status, Untrusted, UntrustedSource, exit_code, keys,
};
use heartwood_cli::doctor::seams::{BuildFacts, Subject};
use heartwood_cli::doctor::view::{Reason, Source, Unknown};
use heartwood_cli::doctor::{DoctorProfile, Redaction};

fn id(index: usize) -> CheckId {
    ENTRIES[index].check_id()
}

fn finding(index: usize, status: Status) -> Finding {
    let unknown = Unknown::new(Source::Api, Reason::RateLimited);
    let mut finding = Finding::unknown(id(index), Subject::Relay, &unknown);
    finding.status = status;
    finding
}

fn report(findings: Vec<Finding>) -> Report {
    Report {
        created_at_ms: 1_790_949_791_000,
        tool: BuildFacts {
            version: "4.3.1-heartwood.0.3.0",
            features: vec!["devnet"],
        },
        scope: ReportScope {
            token: "solar".into(),
            network: "testnet".into(),
            roles: vec![Role::Relay, Role::Forger],
            profile: DoctorProfile::Default,
            window_minutes: 60,
        },
        redaction: Redaction::default(),
        host: HostFacts {
            os: "linux",
            arch: "x86_64",
            kernel: Some("6.8".into()),
            cpus: Some(4),
            memory_bytes: Some(8_214_528_000),
        },
        identity: HostIdentity {
            home: Some(PathBuf::from("/users/alice")),
            user: Some("alice".into()),
            host_name: Some("node7".into()),
        },
        findings,
    }
}

fn excerpt(text: &str) -> Untrusted {
    Untrusted {
        source: UntrustedSource::Log(Role::Relay),
        at_ms: Some(1_790_949_700_000),
        text: text.into(),
    }
}

fn ip(text: &str) -> IpAddr {
    text.parse().unwrap()
}

#[test]
fn the_text_lists_fails_then_warns_unknowns_passes_and_skips_in_registry_order() {
    use Status::*;
    let statuses = [Pass, Warn, Skipped, Fail, Unknown, Warn, Pass, Fail];
    let findings = statuses.iter().enumerate().map(|(i, s)| finding(i, *s));
    let text = render_text(&report(findings.collect()), false);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(
        lines[0],
        "heartwood doctor: solar-testnet (relay, forger), 2026-10-02 14:03:11 UTC, last 60 minutes"
    );
    let order: Vec<(&str, &str)> = lines[1..lines.len() - 1]
        .iter()
        .map(|line| (&line[..7], line[8..33].trim_end()))
        .collect();
    let expected = [
        ("FAIL   ", ENTRIES[3].id),
        ("FAIL   ", ENTRIES[7].id),
        ("WARN   ", ENTRIES[1].id),
        ("WARN   ", ENTRIES[5].id),
        ("UNKNOWN", ENTRIES[4].id),
        ("PASS   ", ENTRIES[0].id),
        ("PASS   ", ENTRIES[6].id),
        ("SKIPPED", ENTRIES[2].id),
    ];
    assert_eq!(order, expected);
    assert_eq!(
        lines[1][34..].to_owned(),
        "The doctor could not read the relay's API: rate limited (429). [relay, chain]"
    );
    assert_eq!(
        lines[lines.len() - 1],
        "Summary: 2 fail, 2 warn, 2 pass, 1 unknown, 1 skipped.  More: heartwood doctor explain <check>"
    );
}

#[test]
fn colour_appears_only_when_asked_and_untrusted_text_never_carries_an_escape() {
    let mut fail = finding(0, Status::Fail);
    fail.untrusted
        .push(excerpt("\u{1b}[2J\u{1b}]0;owned\u{7}cleared\nFAIL fake"));
    let report = report(vec![fail, finding(1, Status::Pass)]);
    let plain = render_text(&report, false);
    assert!(!plain.contains('\u{1b}'), "{plain}");
    let coloured = render_text(&report, true);
    assert!(coloured.contains("\u{1b}[1;31mFAIL   \u{1b}[0m process.roles"));
    assert!(coloured.contains("\u{1b}[32mPASS   \u{1b}[0m"));
    assert_eq!(coloured.matches('\u{1b}').count(), 4, "{coloured}");
    let indent = " ".repeat(34);
    let line =
        format!("{indent}untrusted 1 (relay log, 2026-10-02 14:01:40 UTC): cleared FAIL fake\n");
    assert!(plain.contains(&line), "{plain}");
    assert_eq!(plain.lines().count(), 5);
}

#[test]
fn multi_line_advice_is_indented_under_its_first_line() {
    let mut first = finding(0, Status::Warn);
    first.advice = Advice::key("Line one.\nRun: heartwood relay start");
    let text = render_text(&report(vec![first]), false);
    let lines: Vec<&str> = text.lines().collect();
    assert!(lines[1].ends_with(" Line one. [relay, chain]"), "{text}");
    assert_eq!(
        lines[2],
        format!("{}Run: heartwood relay start", " ".repeat(34))
    );
}

#[test]
fn the_json_report_has_its_keys_in_order_and_typed_values() {
    let mut first = finding(36, Status::Fail);
    first.subject = Subject::Forger;
    first.basis = Basis::System;
    let key = format!("02{}", "ab".repeat(32));
    first.facts = vec![
        ("count", Fact::Int(-3)),
        ("wanted", Fact::Bool(true)),
        ("type", Fact::Word("file")),
        (
            "delegate",
            Fact::Id(Id::new(IdKind::Username, "genesis_1").unwrap()),
        ),
        (
            "publicKey",
            Fact::Id(Id::new(IdKind::PublicKey, &key).unwrap()),
        ),
        ("address", Fact::Address(ip("203.0.113.7"))),
        ("port", Fact::Port(6002)),
        (
            "path",
            Fact::Path("/users/alice/.config/solar/delegates.json".into()),
        ),
        ("mode", Fact::Mode(0o640)),
        ("late", Fact::Millis(1_500)),
        ("size", Fact::Bytes(12_884_901_888)),
        ("at", Fact::Time(1_790_949_791_250)),
        ("never", Fact::Time(i64::MAX)),
    ];
    let value: Value = serde_json::from_str(&render_json(&report(vec![first]))).unwrap();
    let keys: Vec<&String> = value.as_object().unwrap().keys().collect();
    let expected = [
        "format",
        "createdAt",
        "tool",
        "scope",
        "redaction",
        "host",
        "summary",
        "findings",
        "untrusted",
    ];
    assert_eq!(keys, expected);
    assert_eq!(value["createdAt"], "2026-10-02T14:03:11.000Z");
    assert_eq!(
        value["scope"],
        json!({"instance": "solar-testnet", "network": "testnet", "roles": ["relay", "forger"],
               "profile": "default", "windowMinutes": 60})
    );
    let finding = &value["findings"][0];
    let keys: Vec<&String> = finding.as_object().unwrap().keys().collect();
    let expected = [
        "id",
        "subject",
        "status",
        "basis",
        "facts",
        "advice",
        "docs",
        "untrusted",
    ];
    assert_eq!(keys, expected);
    assert_eq!(
        finding["facts"],
        json!({"count": -3, "wanted": true, "type": "file", "delegate": "genesis_1",
               "publicKey": key, "address": "addr-1 (public)", "port": 6002,
               "path": "~/.config/solar/delegates.json", "mode": "0640", "late": 1500,
               "size": 12_884_901_888_u64, "at": "2026-10-02T14:03:11.250Z", "never": null})
    );
    assert_eq!(finding["id"], "keys.file");
    assert_eq!(
        finding["docs"],
        "https://docs.iceroot.com/validators/doctor/#keys-file"
    );
    assert_eq!(
        value["summary"],
        json!({"pass": 0, "warn": 0, "fail": 1, "unknown": 0, "skipped": 0, "worst": "fail"})
    );
}

#[test]
fn the_json_report_always_redacts_addresses_and_show_addresses_changes_the_text_only() {
    let mut first = finding(20, Status::Warn);
    first.advice = Advice {
        key: keys::UNKNOWN,
        values: vec![
            ("source", Fact::Address(ip("198.51.100.20"))),
            ("reason", Fact::Path("/srv/node7/data".into())),
        ],
    };
    first.facts = vec![("listen", Fact::Address(ip("198.51.100.20")))];
    first
        .untrusted
        .push(excerpt("peer 198.51.100.20 and 2001:db8::7 said no"));
    let mut redacted = report(vec![first]);
    let mut shown = redacted.clone();
    shown.redaction.addresses = false;
    assert_eq!(render_json(&shown), render_json(&redacted));
    let json = render_json(&shown);
    assert!(
        !json.contains("198.51.100") && !json.contains("2001:db8"),
        "{json}"
    );
    assert!(
        json.contains(r#""redaction":{"addresses":true,"identity":false,"untrustedOmitted":0}"#)
    );
    let text = render_text(&redacted, false);
    assert!(
        text.contains("The doctor could not read addr-1 (public): /srv/host/data. [relay, chain]")
    );
    assert!(text.contains("peer addr-1 (public) and addr-2 (public) said no"));
    assert!(!text.contains("198.51.100"));
    let text = render_text(&shown, false);
    assert!(text.contains("The doctor could not read 198.51.100.20: /srv/host/data."));
    assert!(text.contains("peer 198.51.100.20 and 2001:db8::7 said no"));
    redacted.redaction.identity = true;
    assert!(render_json(&redacted).contains(r#""identity":true"#));
}

#[test]
fn excerpts_are_listed_once_numbered_capped_at_20_and_named_by_their_findings() {
    let mut first = finding(0, Status::Warn);
    let mut second = finding(1, Status::Warn);
    for n in 0..15 {
        first.untrusted.push(excerpt(&format!("line {n}")));
    }
    second.untrusted.push(excerpt("line 3"));
    for n in 15..25 {
        second.untrusted.push(excerpt(&format!("line {n}")));
    }
    let value: Value = serde_json::from_str(&render_json(&report(vec![first, second]))).unwrap();
    let list = value["untrusted"].as_array().unwrap();
    assert_eq!(list.len(), 20);
    assert_eq!(
        list[0],
        json!({"id": 1, "source": "relay log", "at": "2026-10-02T14:01:40.000Z", "text": "line 0"})
    );
    assert_eq!(list[19]["text"], "line 19");
    assert_eq!(
        value["findings"][0]["untrusted"],
        json!((1..=15).collect::<Vec<_>>())
    );
    assert_eq!(
        value["findings"][1]["untrusted"],
        json!([4, 16, 17, 18, 19, 20])
    );
    assert_eq!(value["redaction"]["untrustedOmitted"], 5);
}

#[test]
fn every_mix_of_statuses_gives_its_exit_code_under_each_fail_on() {
    for mask in 0u32..32 {
        let statuses: Vec<Status> = Status::ALL
            .into_iter()
            .enumerate()
            .filter(|(bit, _)| mask & (1 << bit) != 0)
            .map(|(_, status)| status)
            .collect();
        let findings = statuses.iter().enumerate().map(|(i, s)| finding(i, *s));
        let report = report(findings.collect());
        let (fail, warn) = (
            statuses.contains(&Status::Fail),
            statuses.contains(&Status::Warn),
        );
        let on_warn = if fail { 2 } else { u8::from(warn) };
        let on_fail = if fail { 2 } else { 0 };
        assert_eq!(exit_code(&report, FailOn::Warn), on_warn, "{statuses:?}");
        assert_eq!(exit_code(&report, FailOn::Fail), on_fail, "{statuses:?}");
        let value: Value = serde_json::from_str(&render_json(&report)).unwrap();
        let worst = if fail {
            "fail"
        } else if warn {
            "warn"
        } else {
            "pass"
        };
        assert_eq!(value["summary"]["worst"], worst, "{statuses:?}");
    }
}

#[test]
fn a_skipped_finding_and_an_empty_host_render() {
    let mut report = report(vec![Finding::skipped(
        id(46),
        Subject::Relay,
        keys::SKIPPED_PROFILE,
    )]);
    report.host = HostFacts::default();
    report.scope.roles.clear();
    report.scope.profile = DoctorProfile::Onboarding;
    let text = render_text(&report, false);
    assert!(text.starts_with("heartwood doctor: solar-testnet (no role found), "));
    assert!(
        text.lines()
            .next()
            .unwrap()
            .ends_with("last 60 minutes, profile onboarding")
    );
    assert!(text.contains("This check is not part of the profile that was run. [relay, config]"));
    let value: Value = serde_json::from_str(&render_json(&report)).unwrap();
    assert_eq!(
        value["host"],
        json!({"os": "", "arch": "", "kernel": null, "cpus": null, "memoryBytes": null})
    );
    assert_eq!(value["summary"]["worst"], "pass");
}
