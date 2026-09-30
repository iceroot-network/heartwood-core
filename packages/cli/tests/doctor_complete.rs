//! The doctor is complete: every check of the registry is built, the two checks that are not in
//! this release report that they are skipped, every status a check declares has a case, and every
//! advice a case gives has its text and a value for each name in it.

mod doctor_cases;
mod doctor_support;

use heartwood_cli::doctor::DoctorOptions;
use heartwood_cli::doctor::docs::docs;
use heartwood_cli::doctor::registry::{CheckId, CheckInput, ENTRIES, is_not_built};
use heartwood_cli::doctor::report::{Basis, Status, keys};

use doctor_support::FakeView;

#[test]
fn no_registry_entry_is_not_built() {
    let (view, options) = (FakeView::new(), DoctorOptions::default());
    let not_built: Vec<&str> = ENTRIES
        .iter()
        .filter(|entry| {
            let input = CheckInput {
                entry,
                view: &view,
                options: &options,
            };
            let findings = (entry.run)(&input);
            is_not_built(&findings) || findings.iter().any(|f| f.advice.key == keys::NOT_BUILT)
        })
        .map(|entry| entry.id)
        .collect();
    assert!(not_built.is_empty(), "checks not built: {not_built:?}");
}

#[test]
fn the_checks_not_in_this_release_are_built_to_report_skipped() {
    let (view, options) = (FakeView::new(), DoctorOptions::default());
    for id in ["process.core-role", "quorum.reach"] {
        let entry = ENTRIES.iter().find(|entry| entry.id == id).unwrap();
        let input = CheckInput {
            entry,
            view: &view,
            options: &options,
        };
        let findings = (entry.run)(&input);
        assert!(!findings.is_empty(), "{id}: no finding");
        for finding in &findings {
            assert_eq!(finding.status, Status::Skipped, "{id}");
            assert_eq!(finding.advice.key, keys::SKIPPED_RELEASE, "{id}");
        }
    }
}

#[test]
fn the_registry_walk_finds_a_case_for_every_declared_status() {
    let cases = doctor_cases::all();
    let mut missing = Vec::new();
    for entry in &ENTRIES {
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
    assert!(missing.is_empty(), "statuses without a case: {missing:#?}");
}

#[test]
fn every_advice_a_case_gives_has_its_text() {
    let docs = docs().unwrap();
    let mut missing = Vec::new();
    for case in doctor_cases::all() {
        let entry = ENTRIES.iter().find(|entry| entry.id == case.check).unwrap();
        let id = CheckId::parse(entry.id).unwrap();
        let input = CheckInput {
            entry,
            view: &case.view,
            options: &case.options,
        };
        for finding in (entry.run)(&input) {
            if docs.template(id, finding.advice.key).is_none() {
                missing.push(format!("{} {}", entry.id, finding.advice.key));
            }
        }
    }
    missing.sort();
    missing.dedup();
    assert!(missing.is_empty(), "advice without a text: {missing:#?}");
}

/// The names an advice text puts between braces.
fn placeholders(text: &str) -> Vec<&str> {
    let mut names = Vec::new();
    let mut rest = text;
    while let Some((_, after)) = rest.split_once('{') {
        let Some((name, tail)) = after.split_once('}') else {
            break;
        };
        names.push(name);
        rest = tail;
    }
    names
}

#[test]
fn every_value_an_advice_text_names_is_given() {
    let docs = docs().unwrap();
    let mut missing = Vec::new();
    for case in doctor_cases::all() {
        let entry = ENTRIES.iter().find(|entry| entry.id == case.check).unwrap();
        let id = CheckId::parse(entry.id).unwrap();
        let input = CheckInput {
            entry,
            view: &case.view,
            options: &case.options,
        };
        for finding in (entry.run)(&input) {
            let Some(text) = docs.template(id, finding.advice.key) else {
                continue;
            };
            let given: Vec<&str> = finding
                .advice
                .values
                .iter()
                .map(|(name, _)| *name)
                .collect();
            for name in placeholders(text) {
                if !given.contains(&name) {
                    missing.push(format!("{} {} {{{name}}}", entry.id, finding.advice.key));
                }
            }
        }
    }
    missing.sort();
    missing.dedup();
    assert!(missing.is_empty(), "advice values not given: {missing:#?}");
}

#[test]
fn a_fail_on_the_log_alone_asks_to_stop_remove_roll_back_or_move_nothing() {
    let docs = docs().unwrap();
    let disruptive = ["stop", "remove", "delete", "roll back", "rollback", "move"];
    // The start errors fail only while the process table shows the role down.
    let confirmed = ["relay.start-errors", "forger.start-errors"];
    let mut found = Vec::new();
    for case in doctor_cases::all() {
        let entry = ENTRIES.iter().find(|entry| entry.id == case.check).unwrap();
        let id = CheckId::parse(entry.id).unwrap();
        let input = CheckInput {
            entry,
            view: &case.view,
            options: &case.options,
        };
        for finding in (entry.run)(&input) {
            let log_only = finding.status == Status::Fail && finding.basis == Basis::Log;
            if !log_only || confirmed.contains(&entry.id) {
                continue;
            }
            let text = docs.template(id, finding.advice.key).unwrap_or_default();
            let words = text
                .to_lowercase()
                .replace(|c: char| !c.is_ascii_alphabetic(), " ");
            let words = format!(" {words} ");
            if disruptive
                .iter()
                .any(|verb| words.contains(&format!(" {verb} ")))
            {
                found.push(format!("{} {}", entry.id, finding.advice.key));
            }
        }
    }
    found.sort();
    found.dedup();
    assert!(
        found.is_empty(),
        "log-only fails that ask to act: {found:#?}"
    );
}
