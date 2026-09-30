//! The doctor's checks against their case tables, the registry walk, and a whole run over a world
//! of fakes.

mod doctor_cases;
mod doctor_support;

use std::collections::BTreeSet;

use heartwood_cli::doctor::registry::{CheckInput, ENTRIES, is_not_built};
use heartwood_cli::doctor::render::render_json;
use heartwood_cli::doctor::report::Status;
use heartwood_cli::doctor::{DoctorOptions, run};

use doctor_support::{FakeView, FakeWorld, scope};

#[test]
fn every_case_gives_its_findings_and_only_statuses_its_check_declares() {
    for case in doctor_cases::all() {
        let entry = ENTRIES.iter().find(|entry| entry.id == case.check);
        let entry = entry.unwrap_or_else(|| panic!("{}: no check {}", case.name, case.check));
        let input = CheckInput {
            entry,
            view: &case.view,
            options: &case.options,
        };
        let findings = (entry.run)(&input);
        assert!(!findings.is_empty(), "{}: no finding", case.name);
        for finding in &findings {
            assert_eq!(finding.id.as_str(), entry.id, "{}", case.name);
            assert!(
                entry.statuses.contains(&finding.status),
                "{}: {:?} is not declared by {}",
                case.name,
                finding.status,
                entry.id
            );
        }
        for expect in &case.expect {
            let found = findings.iter().any(|f| {
                f.subject == expect.subject
                    && f.status == expect.status
                    && f.advice.key == expect.advice
            });
            assert!(found, "{}: no {expect:?} in {findings:#?}", case.name);
        }
    }
}

#[test]
fn every_size_is_a_count_of_bytes_under_a_name_that_says_so_and_every_word_is_one() {
    use heartwood_cli::doctor::report::Fact;
    let units = ["_kb", "_mb", "_gb", "_tb", "_kib", "_mib", "_gib", "_tib"];
    for case in doctor_cases::all() {
        let entry = ENTRIES.iter().find(|entry| entry.id == case.check).unwrap();
        let input = CheckInput {
            entry,
            view: &case.view,
            options: &case.options,
        };
        for finding in (entry.run)(&input) {
            let facts = finding.facts.len();
            let all = finding.facts.iter().chain(&finding.advice.values);
            for (n, (name, fact)) in all.enumerate() {
                let what = format!("{}: {} {name} = {fact:?}", case.name, finding.id.as_str());
                assert!(!units.iter().any(|unit| name.contains(unit)), "{what}");
                let bytes = name.ends_with("_bytes") || name.contains("_bytes_");
                assert_eq!(matches!(fact, Fact::Bytes(_)), bytes, "{what}");
                // A fixed word of the report's facts is one word; the advice shows its phrase.
                let word = |w: &str| w.bytes().all(|b| b.is_ascii_lowercase() || b == b'-');
                let phrase = matches!(fact, Fact::Word(w) if !word(w));
                assert!(n >= facts || !phrase, "{what}");
            }
        }
    }
}

#[test]
fn every_duration_is_milliseconds_under_millis_or_ms_or_a_count_of_the_unit_its_name_gives() {
    use heartwood_cli::doctor::report::Fact;
    // The documented rule: milliseconds under `millis` or a `_ms` name, and nothing else under
    // them; a duration under another unit's name (`minutes`, `hours`, `days`) is a plain count.
    let counted = ["seconds", "minutes", "hours", "days"];
    for case in doctor_cases::all() {
        let entry = ENTRIES.iter().find(|entry| entry.id == case.check).unwrap();
        let input = CheckInput {
            entry,
            view: &case.view,
            options: &case.options,
        };
        for finding in (entry.run)(&input) {
            let all = finding.facts.iter().chain(&finding.advice.values);
            for (name, fact) in all {
                let what = format!("{}: {} {name} = {fact:?}", case.name, finding.id.as_str());
                let millis = *name == "millis" || name.ends_with("_ms");
                assert_eq!(matches!(fact, Fact::Millis(_)), millis, "{what}");
                let count = counted.contains(name);
                assert!(!count || matches!(fact, Fact::Int(_)), "{what}");
            }
        }
    }
}

#[test]
fn the_registry_walk_finds_a_case_for_every_status_of_every_built_check() {
    let cases = doctor_cases::all();
    let (mut not_built, mut missing) = (Vec::new(), Vec::new());
    let (view, options) = (FakeView::new(), DoctorOptions::default());
    for entry in &ENTRIES {
        let input = CheckInput {
            entry,
            view: &view,
            options: &options,
        };
        if is_not_built(&(entry.run)(&input)) {
            not_built.push(entry.id);
            continue;
        }
        for status in entry.statuses {
            let covered = cases
                .iter()
                .filter(|case| case.check == entry.id)
                .any(|case| case.expect.iter().any(|expect| expect.status == *status));
            if !covered {
                missing.push(format!("{} {}", entry.id, status.name()));
            }
        }
    }
    eprintln!(
        "checks not built yet ({}): {}",
        not_built.len(),
        not_built.join(" ")
    );
    assert!(missing.is_empty(), "statuses without a case: {missing:#?}");
}

#[test]
fn the_registry_holds_47_checks_with_unique_ids_and_declared_statuses() {
    assert_eq!(ENTRIES.len(), 47);
    let ids: BTreeSet<&str> = ENTRIES.iter().map(|entry| entry.id).collect();
    assert_eq!(ids.len(), ENTRIES.len());
    for entry in &ENTRIES {
        assert!(!entry.subjects.is_empty(), "{}", entry.id);
        assert!(!entry.statuses.is_empty(), "{}", entry.id);
        assert!(!entry.profiles.is_empty(), "{}", entry.id);
    }
    for id in ["process.core-role", "quorum.reach"] {
        let entry = ENTRIES.iter().find(|entry| entry.id == id).unwrap();
        assert_eq!(entry.statuses, [Status::Skipped], "{id}");
    }
}

#[test]
fn a_relay_only_host_with_a_stray_key_file_runs_no_forger_checks() {
    use heartwood_cli::doctor::seams::{FileFacts, FileKind, Meta, Subject, SupervisorFacts};
    use heartwood_cli::{Role, StatusRow, SupervisedStatus};
    let mut world = FakeWorld::default();
    let scope = scope("/nodes/node1");
    // The relay runs under its supervisor; no forger ever ran here (no lock, pid or state file),
    // and a key file was left in the configuration directory.
    let relay = StatusRow {
        pid: Some(4242),
        name: "solar-testnet-relay".into(),
        version: "4.3.1-heartwood.0.1.0".into(),
        status: SupervisedStatus::Online,
        uptime: None,
        restarts: 0,
        cpu_percent: None,
        rss_bytes: None,
    };
    let supervisor = SupervisorFacts {
        started_at_ms: Some(doctor_support::NOW_MS - 3_600_000),
        restarts: 0,
        unstable: 0,
        version: "4.3.1-heartwood.0.1.0".into(),
    };
    world
        .processes
        .rows
        .insert(Role::Relay, (relay, Some(supervisor)));
    let key = scope.paths.config.join("delegates.json");
    let own = Meta {
        kind: FileKind::File,
        mode: 0o600,
        uid: 1000,
        gid: 1000,
        size: 12,
        dev: 1,
        ino: 7,
        nlink: 1,
        modified_ms: 0,
    };
    let (target, link_target) = (None, None);
    let facts = FileFacts {
        path: key.clone(),
        own,
        target,
        link_target,
    };
    world.fs.facts.insert(key, Ok(facts));
    let report = run(&world, &scope, &DoctorOptions::default());
    assert_eq!(report.scope.roles, [Role::Relay]);
    let found = |id: &str, subject: Subject| {
        let mut found = report.findings.iter();
        let finding = found.find(|f| f.id.as_str() == id && f.subject == subject);
        finding.map(|f| (f.status, f.advice.key))
    };
    // No "start the forger": the key file alone is not a forger.
    assert_eq!(found("process.roles", Subject::Forger), None);
    assert_eq!(
        found("keys.stray", Subject::Relay),
        Some((Status::Warn, "stray"))
    );
    let absent = Some((Status::Skipped, "skipped-absent"));
    for id in [
        "keys.file",
        "slots.missed",
        "forger.link",
        "forger.signing-record",
    ] {
        assert_eq!(found(id, Subject::Forger), absent, "{id}");
    }
}

#[test]
fn a_run_over_fakes_gives_one_finding_per_check_and_the_same_json_each_time() {
    let world = FakeWorld::default();
    let scope = scope("/nodes/node1");
    let options = DoctorOptions::default();
    let report = run(&world, &scope, &options);
    let ids: BTreeSet<&str> = report.findings.iter().map(|f| f.id.as_str()).collect();
    assert_eq!(ids.len(), ENTRIES.len());
    let first = render_json(&report);
    let value: serde_json::Value = serde_json::from_str(&first).unwrap();
    assert_eq!(value["format"], "heartwood-doctor-report/1");
    assert_eq!(value["redaction"]["addresses"], true);
    for _ in 0..3 {
        assert_eq!(render_json(&run(&world, &scope, &options)), first);
    }
}
