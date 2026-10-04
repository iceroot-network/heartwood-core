//! `gate register`: every entry of the register of documented differences, active or deferred,
//! has a trigger and a named test that passes at the head, and the runs' hits are the ones each
//! run may have.
//!
//! ```text
//! difftest gate register --criteria FILE --records DIR... [--ledger DIR...] [--run] [--out DIR]
//!     [--strict] [--repo DIR] [--head REV] [--allow-dirty]
//! ```
//!
//! What is checked:
//!
//! - an entry that is not retired has a non-empty trigger and at least one test;
//! - a cargo test exists in the file it is named in (a `fn` of that name), and with `--run` it
//!   passes (the tests run grouped by package, test file and features, and a test marked ignored
//!   runs too); a record test is a valid passing record of its make target, and a record test
//!   that names what it waits on leaves its entry pending, not failed, while no such record exists
//!   yet (none at all, or only stale ones);
//! - each code name an entry claims is a real name;
//! - an honest run (the product run as meant to run) hit no entry beyond those its rule allows;
//!   the intended hits of a configuration or attack suite and of the vectors and properties are
//!   counted, not failed; a record with hits and no rule fails. The records of the commands that
//!   judge other records (`register`, `coverage`) carry no hits of their own and are not read.
//!
//! Without `--strict` the command exits 0 when no entry fails and no hit is a problem; with it,
//! only when every entry is also met (none pending) and the tests ran (`--run`). With `--out` the
//! command writes its own record (target `register`), a pass only when every entry is met and the
//! tests ran; while entries wait on later runs and none fails, it writes none.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::Path;
use std::process::Command;

use super::criteria::{Criteria, Entry, EntryStatus, RecordRule, RunClass, TestRef};
use super::flags::Flags;
use super::git::Validity;
use super::make_record::{OwnRecord, write_own};
use super::names;
use super::needs::mentions;
use super::record::{Counts, GateRecord, GateResult};
use super::s1::setup;
use super::store::{Evidence, Item, Sources};
use super::time;
use crate::errors::{Error, Result};

/// One entry's findings: what fails it, and what it waits on.
#[derive(Debug, Default)]
struct Finding {
    problems: Vec<String>,
    waits: Vec<String>,
}

/// The targets of the commands that judge other records: their records are not runs.
const JUDGING: [&str; 2] = ["register", "coverage"];

/// Whether `text` defines a function called `name`.
fn defines(text: &str, name: &str) -> bool {
    text.match_indices(&format!("fn {name}")).any(|(at, m)| {
        text[at + m.len()..]
            .chars()
            .next()
            .is_some_and(|c| c == '(' || c == '<' || c.is_whitespace())
    })
}

/// Whether the record is the one a record rule or record test names.
fn matches(record: &GateRecord, target: &str, contains: &[String]) -> bool {
    record.target == target && contains.iter().all(|c| mentions(record, c))
}

/// The rule for `record`, the most specific one (the most `contains`) first.
fn rule_for<'a>(rules: &'a [RecordRule], record: &GateRecord) -> Option<&'a RecordRule> {
    rules
        .iter()
        .filter(|r| matches(record, &r.target, &r.contains))
        .max_by_key(|r| r.contains.len())
}

/// The passing tests of a `cargo test` run, by name (the last path segment).
fn passed_tests(output: &str) -> BTreeSet<String> {
    output
        .lines()
        .filter_map(|line| {
            let rest = line.strip_prefix("test ")?;
            let (path, verdict) = rest.rsplit_once(" ... ")?;
            (verdict.trim() == "ok").then(|| path.rsplit("::").next().unwrap_or(path).to_owned())
        })
        .collect()
}

/// Run the cargo tests of `entries`, grouped by package and test file; the names that passed.
fn run_cargo(repo: &Path, entries: &[&Entry]) -> Result<BTreeSet<(String, String)>> {
    let mut groups: BTreeMap<(String, Option<String>, Vec<String>), BTreeSet<String>> =
        BTreeMap::new();
    for entry in entries {
        for test in &entry.tests {
            if let TestRef::Cargo {
                package,
                test,
                name,
                features,
                ..
            } = test
            {
                let mut features = features.clone();
                features.sort();
                groups
                    .entry((package.clone(), test.clone(), features))
                    .or_default()
                    .insert(name.clone());
            }
        }
    }
    let mut passed = BTreeSet::new();
    for ((package, test, features), names) in groups {
        let mut command = Command::new("cargo");
        command
            .current_dir(repo)
            .args(["test", "--locked", "-p", &package]);
        if !features.is_empty() {
            command.args(["--features", &features.join(",")]);
        }
        match &test {
            Some(test) => command.args(["--test", test]),
            None => command.arg("--lib"),
        };
        // The names are the filter, so an integration test that is marked ignored runs too.
        command.arg("--").arg("--include-ignored");
        for name in &names {
            command.arg(name);
        }
        let output = command.output().map_err(|e| Error::io(repo, e))?;
        let text = String::from_utf8_lossy(&output.stdout);
        let ok = passed_tests(&text);
        for name in names {
            if ok.contains(&name) {
                passed.insert((package.clone(), name));
            }
        }
    }
    Ok(passed)
}

/// Judge one entry: its trigger, its tests and its code names.
fn judge_entry(
    entry: &Entry,
    sources: &Sources,
    evidence: &Evidence,
    passed: Option<&BTreeSet<(String, String)>>,
) -> Finding {
    let mut finding = Finding::default();
    if entry.trigger.trim().is_empty() {
        finding.problems.push("has no trigger".to_owned());
    }
    if entry.tests.is_empty() {
        finding.problems.push("names no test".to_owned());
    }
    for name in &entry.code_names {
        if !names::all_names().contains(&name.as_str()) {
            finding
                .problems
                .push(format!("claims {name}, which is not a code name"));
        }
    }
    for test in &entry.tests {
        match test {
            TestRef::Cargo {
                package,
                file,
                name,
                ..
            } => match std::fs::read_to_string(sources.repo.join(file)) {
                Err(_) => finding
                    .problems
                    .push(format!("test file {file} is not there")),
                Ok(text) if !defines(&text, name) => {
                    finding.problems.push(format!("{file} has no test {name}"));
                }
                Ok(_) => {
                    if let Some(passed) = passed
                        && !passed.contains(&(package.clone(), name.clone()))
                    {
                        finding
                            .problems
                            .push(format!("{package} test {name} did not pass"));
                    }
                }
            },
            TestRef::Record {
                target,
                contains,
                waits,
            } => {
                let valid: Vec<&GateRecord> = evidence
                    .records
                    .iter()
                    .filter(|item| item.validity == Validity::Valid)
                    .map(|item| &item.value)
                    .filter(|record| matches(record, target, contains))
                    .collect();
                if valid.iter().any(|r| r.result == GateResult::Pass) {
                    continue;
                }
                let what = format!(
                    "no valid passing record of {target}{}",
                    if contains.is_empty() {
                        String::new()
                    } else {
                        format!(" with {}", contains.join(" "))
                    }
                );
                match waits {
                    // Nothing valid ran yet: the record is still to come.
                    Some(waits) if valid.is_empty() => {
                        finding.waits.push(format!("waits on {waits}: {what}"));
                    }
                    _ => finding.problems.push(what),
                }
            }
        }
    }
    finding
}

/// The records that count and how each meets the rules: problems, and the hits by code name.
fn judge_hits(
    criteria: &Criteria,
    records: &[Item<GateRecord>],
) -> (Vec<String>, BTreeMap<String, u64>) {
    let mut problems = Vec::new();
    let mut hits: BTreeMap<String, u64> = BTreeMap::new();
    let claimed: BTreeSet<&str> = criteria
        .register
        .entries
        .iter()
        .flat_map(|e| e.code_names.iter().map(String::as_str))
        .collect();
    for item in records
        .iter()
        .filter(|i| i.validity.counts() && !JUDGING.contains(&i.value.target.as_str()))
    {
        let record = &item.value;
        if record.hits.is_empty() {
            continue;
        }
        let file = item.path.display();
        let rule = rule_for(&criteria.register.records, record);
        for (name, count) in &record.hits {
            *hits.entry(name.clone()).or_insert(0) += count;
            if !claimed.contains(name.as_str()) {
                problems.push(format!("{file}: hit {name}, which no entry claims"));
            }
            match rule {
                None => problems.push(format!(
                    "{file}: {} hit {name} {count} time(s) and the register has no rule for it",
                    record.target
                )),
                Some(rule) if rule.class == RunClass::Honest && !rule.allow.contains(name) => {
                    problems.push(format!(
                        "{file}: a run of {} hit {name} {count} time(s); an honest run hits none",
                        record.target
                    ));
                }
                Some(_) => {}
            }
        }
    }
    (problems, hits)
}

/// Run `gate register`.
pub fn run(args: &[String]) -> Result<bool> {
    let started = time::now_utc();
    let mut flags = Flags::parse(args, &["strict", "allow-dirty", "run"])?;
    let strict = flags.switch("strict");
    let run_tests = flags.switch("run");
    let out = flags.one("out")?;
    let setup = setup(&mut flags)?;
    flags.finish()?;
    let criteria = &setup.criteria;
    let evidence = Evidence::load(criteria, &setup.sources)?;

    let live: Vec<&Entry> = criteria.register.entries.iter().collect();
    let passed = if run_tests {
        Some(run_cargo(&setup.sources.repo, &live)?)
    } else {
        None
    };

    let mut text = String::new();
    let (mut ok, mut pending, mut bad) = (0u64, 0u64, 0u64);
    let mut seen = BTreeSet::new();
    let id_width = criteria
        .register
        .entries
        .iter()
        .map(|e| e.id.chars().count())
        .max()
        .unwrap_or(0)
        + 2;
    let (problems, hits) = judge_hits(criteria, &evidence.records);
    for entry in &criteria.register.entries {
        let mut finding = judge_entry(entry, &setup.sources, &evidence, passed.as_ref());
        if !seen.insert(entry.id.as_str()) {
            finding.problems.push("is listed twice".to_owned());
        }
        if criteria.register.retired.contains(&entry.id) {
            finding
                .problems
                .push("is retired and also listed".to_owned());
        }
        let word = match entry.status {
            EntryStatus::Active => "active",
            EntryStatus::Deferred => "deferred",
        };
        // The runs' hits of the entry's code names, counted per entry.
        let met: u64 = entry
            .code_names
            .iter()
            .filter_map(|name| hits.get(name))
            .sum();
        let met = if met > 0 {
            format!("  {met} hit{}", if met == 1 { "" } else { "s" })
        } else {
            String::new()
        };
        if !finding.problems.is_empty() {
            bad += 1;
            let _ = writeln!(text, "{:<id_width$}{word:<9}FAIL{met}", entry.id);
            for problem in &finding.problems {
                let _ = writeln!(text, "      {problem}");
            }
        } else if !finding.waits.is_empty() {
            pending += 1;
            let _ = writeln!(text, "{:<id_width$}{word:<9}pending{met}", entry.id);
            for waits in &finding.waits {
                let _ = writeln!(text, "      {waits}");
            }
        } else {
            ok += 1;
            let _ = writeln!(text, "{:<id_width$}{word:<9}ok{met}", entry.id);
        }
    }
    if !problems.is_empty() {
        let _ = writeln!(text, "hits");
        for problem in &problems {
            let _ = writeln!(text, "      FAIL    {problem}");
        }
    }
    if !hits.is_empty() {
        let _ = writeln!(text, "counted hits");
        for (name, count) in &hits {
            let _ = writeln!(text, "      {count:>8}  {name}");
        }
    }
    print!("{text}");
    let failed = bad + problems.len() as u64;
    println!(
        "{} entries ({} retired): {ok} ok, {pending} pending, {bad} fail; {} hit problem(s){}",
        criteria.register.entries.len(),
        criteria.register.retired.len(),
        problems.len(),
        if run_tests {
            ""
        } else {
            "; tests were not run (--run)"
        }
    );
    let met = failed == 0 && pending == 0 && run_tests;
    let all = failed == 0 && (!strict || met);
    // A record is a pass or a failure. Entries that only wait on later runs are neither: no record
    // is written for them, since a failed one would block every row that reads the register.
    if out.is_some() && failed == 0 && pending > 0 {
        println!("gate register: no record written: {pending} entries wait on later runs");
    } else if let Some(out) = out {
        let path = write_own(
            Path::new(&out),
            &setup.sources.repo,
            criteria,
            OwnRecord {
                target: "register",
                command: format!("difftest gate register {}", args.join(" ")),
                pass: met,
                started,
                counts: Counts {
                    compared: criteria.register.entries.len() as u64,
                    passed: ok,
                    failed,
                    documented: hits.values().sum(),
                    skipped: pending,
                },
                hits,
            },
        )?;
        println!("gate register: {}", path.display());
    }
    Ok(all)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_function_is_found_by_its_whole_name() {
        let text = "#[test]\nfn a_test() {}\nasync fn another_test () {}\nfn a_test_longer() {}";
        assert!(defines(text, "a_test"));
        assert!(defines(text, "another_test"));
        assert!(!defines(text, "a_tes"));
        assert!(!defines(text, "longer"));
    }

    #[test]
    fn cargo_output_names_the_tests_that_passed() {
        let out = "running 2 tests\ntest a::b::first ... ok\ntest second ... FAILED\ntest third ... ignored\n";
        let ok = passed_tests(out);
        assert!(ok.contains("first"));
        assert!(!ok.contains("second"));
        assert!(!ok.contains("third"));
    }
}
