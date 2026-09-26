//! L0: golden vectors.
//!
//! [`run`] passes every record of a vector set to the subject registered for its operation and
//! compares the result with the oracle's. [`diff_runs`] compares two oracle runs with each other:
//! a regeneration must be bit-identical unless the oracle changed, and the native and js bcrypto
//! backends, and the image and local runs, must agree.

use serde_json::Value;

use crate::compare::{self, Difference};
use crate::report::{CaseResult, Outcome, Report, describe_meta};
use crate::subject::{Case, Registry, SubjectError, Verdict};
use crate::vectors::{Expected, Meta, Record, VectorSet};

/// Compare every record with Heartwood's subjects.
pub fn run(set: &VectorSet, registry: &Registry) -> Report {
    let mut results = Vec::with_capacity(set.record_count());
    for file in &set.files {
        let file_name = file.file_name();
        for record in &file.records {
            results.push(CaseResult {
                file: file_name.clone(),
                line: record.line,
                op: record.op.clone(),
                label: record.label(),
                height: record.height,
                outcome: judge(record, registry),
            });
        }
    }
    Report {
        title: super::Level::L0.title().to_string(),
        source: set
            .meta()
            .map(describe_meta)
            .unwrap_or_else(|| "no vector files".into()),
        files: set.files.len(),
        results,
    }
}

fn judge(record: &Record, registry: &Registry) -> Outcome {
    let case = Case {
        op: &record.op,
        network: &record.network,
        height: record.height,
        input: &record.input,
    };
    judge_case(&case, &record.expected, registry)
}

/// Run `case` with the subject registered for its operation and judge the result against the
/// oracle's `expected` answer. Error messages are never compared. When both sides reject, the
/// error classes are compared where Heartwood's subject names the reference's class; a refusal
/// by design must come with an input of the documented form.
pub fn judge_case(case: &Case<'_>, expected: &Expected, registry: &Registry) -> Outcome {
    let Some(subject) = registry.get(case.op) else {
        return Outcome::NoSubject;
    };
    match (subject.run(case), expected) {
        (Ok(Verdict::Output(actual)), Expected::Output(expected)) => {
            let differences = compare::diff(
                &without_error_messages(expected),
                &without_error_messages(&actual),
            );
            if differences.is_empty() {
                Outcome::Pass
            } else {
                Outcome::Mismatch(differences)
            }
        }
        (Ok(Verdict::Rejected { class, .. }), Expected::Error(error)) => match class {
            Some(class) if class != error.class => Outcome::ClassMismatch {
                oracle: error.class.clone(),
                heartwood: class.to_owned(),
            },
            _ => Outcome::Pass,
        },
        (Ok(Verdict::RefusedByDesign { difference, holds }), expected) => match (holds, expected) {
            (false, _) => Outcome::Mislabelled(difference.reason().to_owned()),
            (true, Expected::Error(_)) => Outcome::Pass,
            (true, Expected::Output(_)) => Outcome::Documented(difference.reason().to_owned()),
        },
        (Ok(Verdict::Rejected { class, detail }), Expected::Output(_)) => {
            Outcome::VerdictMismatch {
                oracle: "accepted".into(),
                heartwood: format!("rejected ({})", class.map_or(detail, str::to_owned)),
            }
        }
        (Ok(Verdict::Output(_)), Expected::Error(error)) => Outcome::VerdictMismatch {
            oracle: format!("rejected ({})", error.class),
            heartwood: "accepted".into(),
        },
        (Err(SubjectError::Unsupported(why)), _) => Outcome::Skipped(why),
        (Err(SubjectError::Failed(why)), _) => Outcome::HarnessError(why),
    }
}

/// `value` without the `message` of the errors nested in an output (an object with `class` and
/// `message`): only an error's class is observable, and messages are never compared.
fn without_error_messages(value: &Value) -> Value {
    match value {
        Value::Object(object) => Value::Object(
            object
                .iter()
                .filter(|(key, value)| {
                    !(key.as_str() == "message"
                        && value.is_string()
                        && object.contains_key("class"))
                })
                .map(|(key, value)| (key.clone(), without_error_messages(value)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(without_error_messages).collect()),
        other => other.clone(),
    }
}

/// Differences between two oracle runs.
#[derive(Debug, Clone, Default)]
pub struct RunDiff {
    /// Meta fields that differ (expected between backends or images; informational).
    pub meta: Vec<String>,
    /// Vector files present in one run only.
    pub missing_files: Vec<String>,
    /// Records that differ, as `file:line label` and the differences.
    pub records: Vec<(String, Vec<Difference>)>,
    /// Records compared.
    pub compared: usize,
}

impl RunDiff {
    pub fn is_identical(&self) -> bool {
        self.missing_files.is_empty() && self.records.is_empty()
    }
}

fn meta_fields(meta: &Meta) -> Vec<(&'static str, Value)> {
    vec![
        ("solar.commit", meta.solar.commit.clone().into()),
        ("solar.tree", meta.solar.tree.clone().into()),
        ("solar.dirty", meta.solar.dirty.into()),
        (
            "solar.seriesSha256",
            meta.solar.series_sha256.clone().into(),
        ),
        ("oracle.mode", meta.oracle.mode.clone().into()),
        ("oracle.image", meta.oracle.image.clone().into()),
        ("node", meta.node.clone().into()),
        ("icu", meta.icu.clone().into()),
        ("bcrypto", meta.bcrypto.clone().into()),
        ("network", meta.network.clone()),
    ]
}

/// Compare two runs record by record. Meta records are compared field by field and reported, but
/// only record differences and missing files make the runs differ.
pub fn diff_runs(a: &VectorSet, b: &VectorSet) -> RunDiff {
    let mut out = RunDiff::default();
    if let (Some(ma), Some(mb)) = (a.meta(), b.meta()) {
        for ((name, va), (_, vb)) in meta_fields(ma).into_iter().zip(meta_fields(mb)) {
            if va != vb {
                out.meta.push(format!("{name}: {va} vs {vb}"));
            }
        }
    }
    for fa in &a.files {
        let name = fa.file_name();
        let Some(fb) = b.files.iter().find(|f| f.file_name() == name) else {
            out.missing_files
                .push(format!("{name} (only in {})", a.dir.display()));
            continue;
        };
        let n = fa.records.len().max(fb.records.len());
        for i in 0..n {
            let (ra, rb) = (fa.records.get(i), fb.records.get(i));
            let (va, vb) = (
                ra.map(|r| &r.raw).unwrap_or(&Value::Null),
                rb.map(|r| &r.raw).unwrap_or(&Value::Null),
            );
            out.compared += 1;
            let differences = compare::diff(va, vb);
            if let (false, Some(r)) = (differences.is_empty(), ra.or(rb)) {
                out.records
                    .push((format!("{name}:{} {}", r.line, r.label()), differences));
            }
        }
    }
    for fb in &b.files {
        let name = fb.file_name();
        if !a.files.iter().any(|f| f.file_name() == name) {
            out.missing_files
                .push(format!("{name} (only in {})", b.dir.display()));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::subject::{Documented, Subject};
    use crate::vectors::testutil::{TestDir, keys_record, write_file};

    /// Answers keys.fromPassphrase with a fixed public key, or rejects "reject me".
    struct FakeKeys(&'static str);

    impl Subject for FakeKeys {
        fn op(&self) -> &'static str {
            "keys.fromPassphrase"
        }
        fn run(&self, case: &Case<'_>) -> Result<Verdict, SubjectError> {
            match case.input["passphrase"].as_str() {
                Some("reject me") => Ok(Verdict::Rejected {
                    class: Some("KeyError"),
                    detail: "KeyError".into(),
                }),
                Some("wrong class") => Ok(Verdict::Rejected {
                    class: Some("OtherError"),
                    detail: "OtherError".into(),
                }),
                Some("any class") => Ok(Verdict::Rejected {
                    class: None,
                    detail: "Internal".into(),
                }),
                Some("by design") => Ok(Verdict::RefusedByDesign {
                    difference: Documented::ExtendedHeader,
                    holds: true,
                }),
                Some("mislabelled") => Ok(Verdict::RefusedByDesign {
                    difference: Documented::ExtendedHeader,
                    holds: false,
                }),
                Some("later") => Err(SubjectError::Unsupported("later".into())),
                _ => Ok(Verdict::Output(
                    json!({"privateKey": "00", "publicKey": self.0}),
                )),
            }
        }
    }

    fn set(name: &str, records: &[Value], backend: &str) -> (TestDir, VectorSet) {
        let dir = TestDir::new(name);
        write_file(dir.path(), "V01-keys-addresses", records, backend);
        let set = VectorSet::load(dir.path()).unwrap();
        (dir, set)
    }

    #[test]
    fn empty_registry_compares_nothing() {
        let (_dir, set) = set(
            "empty",
            &[keys_record("a", "02aa"), keys_record("b", "02bb")],
            "native",
        );
        let report = run(&set, &Registry::new());
        assert_eq!(report.compared(), 0);
        assert!(report.is_success());
        assert_eq!(
            report.without_subject().get("keys.fromPassphrase"),
            Some(&2)
        );
        assert!(
            report
                .to_string()
                .contains("0 compared: 0 passed, 0 failed")
        );
    }

    #[test]
    fn subjects_pass_fail_and_skip() {
        let rejected = json!({"op": "keys.fromPassphrase", "network": "devnet", "height": 2,
                              "input": {"passphrase": "reject me"},
                              "error": {"class": "KeyError", "message": "no"}});
        let accepted_by_ts = keys_record("reject me", "02cc");
        let later = keys_record("later", "02dd");
        let (_dir, set) = set(
            "subjects",
            &[
                keys_record("a", "02aa"),
                keys_record("b", "02bb"),
                rejected,
                accepted_by_ts,
                later,
            ],
            "native",
        );
        let mut registry = Registry::new();
        registry.register(Box::new(FakeKeys("02aa"))).unwrap();
        let report = run(&set, &registry);
        let outcomes: Vec<&Outcome> = report.results.iter().map(|r| &r.outcome).collect();
        assert_eq!(outcomes[0], &Outcome::Pass);
        assert!(matches!(outcomes[1], Outcome::Mismatch(d) if d[0].path() == "$.publicKey"));
        assert_eq!(outcomes[2], &Outcome::Pass);
        assert!(matches!(outcomes[3], Outcome::VerdictMismatch { .. }));
        assert!(matches!(outcomes[4], Outcome::Skipped(_)));
        assert_eq!(
            (report.compared(), report.passed(), report.failed()),
            (4, 2, 2)
        );
        let text = report.to_string();
        assert!(
            text.contains("FAIL V01-keys-addresses.jsonl:3 keys.fromPassphrase (height 2)"),
            "{text}"
        );
        assert!(
            text.contains("oracle accepted, Heartwood rejected (KeyError)"),
            "{text}"
        );
    }

    #[test]
    fn rejections_compare_their_classes_and_documented_forms() {
        let error = |passphrase: &str, class: &str| {
            json!({"op": "keys.fromPassphrase", "network": "devnet", "height": 2,
                   "input": {"passphrase": passphrase},
                   "error": {"class": class, "message": "no"}})
        };
        let (_dir, set) = set(
            "classes",
            &[
                error("reject me", "KeyError"),
                error("wrong class", "KeyError"),
                error("any class", "KeyError"),
                error("by design", "KeyError"),
                keys_record("by design", "02aa"),
                keys_record("mislabelled", "02aa"),
                error("mislabelled", "KeyError"),
            ],
            "native",
        );
        let mut registry = Registry::new();
        registry.register(Box::new(FakeKeys("02aa"))).unwrap();
        let report = run(&set, &registry);
        let outcomes: Vec<&Outcome> = report.results.iter().map(|r| &r.outcome).collect();
        assert_eq!(outcomes[0], &Outcome::Pass);
        assert_eq!(
            outcomes[1],
            &Outcome::ClassMismatch {
                oracle: "KeyError".into(),
                heartwood: "OtherError".into()
            }
        );
        assert_eq!(outcomes[2], &Outcome::Pass);
        assert_eq!(outcomes[3], &Outcome::Pass);
        assert!(matches!(outcomes[4], Outcome::Documented(_)));
        assert!(matches!(outcomes[5], Outcome::Mislabelled(_)));
        assert!(matches!(outcomes[6], Outcome::Mislabelled(_)));
        assert_eq!(report.failed(), 3);
        let text = report.to_string();
        assert!(
            text.contains("oracle rejected (KeyError), Heartwood rejected (OtherError)"),
            "{text}"
        );
        assert!(
            text.contains(
                "refused as a documented difference, but the input does not have its form"
            ),
            "{text}"
        );
    }

    #[test]
    fn runs_that_differ_only_in_meta_are_identical() {
        let records = [keys_record("a", "02aa"), keys_record("b", "02bb")];
        let (_da, a) = set("native", &records, "native");
        let (_db, b) = set("js", &records, "js");
        let d = diff_runs(&a, &b);
        assert!(d.is_identical());
        assert_eq!(d.compared, 2);
        assert_eq!(d.meta, vec!["bcrypto: \"native\" vs \"js\"".to_string()]);
    }

    #[test]
    fn runs_with_different_records_differ() {
        let (_da, a) = set(
            "a",
            &[keys_record("a", "02aa"), keys_record("b", "02bb")],
            "native",
        );
        let (_db, b) = set("b", &[keys_record("a", "02aa")], "native");
        let d = diff_runs(&a, &b);
        assert!(!d.is_identical());
        assert_eq!(d.records.len(), 1);
        assert_eq!(
            d.records[0].0,
            "V01-keys-addresses.jsonl:3 keys.fromPassphrase"
        );
    }
}
