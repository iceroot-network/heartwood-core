//! Results of a comparison run and their text report.

use std::collections::BTreeMap;
use std::fmt;

use crate::compare::Difference;
use crate::vectors::Meta;

/// Differences shown per failed case.
const SHOWN_DIFFERENCES: usize = 10;

/// What happened to one case.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// Heartwood produced exactly the oracle's result.
    Pass,
    /// Both produced an output, and the outputs differ.
    Mismatch(Vec<Difference>),
    /// One side accepted and the other rejected.
    VerdictMismatch {
        /// The oracle's verdict.
        oracle: String,
        /// Heartwood's verdict.
        heartwood: String,
    },
    /// Both rejected, with different error classes where the class is observable.
    ClassMismatch {
        /// The oracle's error class.
        oracle: String,
        /// The class Heartwood's subject names.
        heartwood: String,
    },
    /// Heartwood refused the case as a documented difference, but the input does not have the
    /// documented form: an undocumented refusal. The text is the difference's reason.
    Mislabelled(String),
    /// Heartwood rejects the case on purpose where the reference accepts it: a documented
    /// difference, with its reason. Not a failure, and not counted as compared.
    Documented(String),
    /// The subject does not model this input yet.
    Skipped(String),
    /// The subject failed (a harness problem, counted as a failure).
    HarnessError(String),
    /// No Rust implementation of the operation is registered yet.
    NoSubject,
}

impl Outcome {
    /// Whether the case failed.
    pub fn is_failure(&self) -> bool {
        matches!(
            self,
            Outcome::Mismatch(_)
                | Outcome::VerdictMismatch { .. }
                | Outcome::ClassMismatch { .. }
                | Outcome::Mislabelled(_)
                | Outcome::HarnessError(_)
        )
    }

    /// A subject ran and its result was compared with the oracle's.
    pub fn is_compared(&self) -> bool {
        matches!(
            self,
            Outcome::Pass
                | Outcome::Mismatch(_)
                | Outcome::VerdictMismatch { .. }
                | Outcome::ClassMismatch { .. }
                | Outcome::Mislabelled(_)
        )
    }
}

/// One case and its outcome.
#[derive(Debug, Clone)]
pub struct CaseResult {
    /// The vector file, or the corpus, the case came from.
    pub file: String,
    /// The line in the file, from 1.
    pub line: usize,
    /// The oracle operation.
    pub op: String,
    /// The case's label: the operation and its name.
    pub label: String,
    /// The rule height.
    pub height: u64,
    /// What happened.
    pub outcome: Outcome,
}

/// The results of one level's run over one source of cases.
#[derive(Debug, Clone)]
pub struct Report {
    /// The level's title.
    pub title: String,
    /// One line describing the oracle run the cases came from.
    pub source: String,
    /// The number of files the cases came from.
    pub files: usize,
    /// Every case, in order.
    pub results: Vec<CaseResult>,
}

impl Report {
    fn count(&self, f: impl Fn(&Outcome) -> bool) -> usize {
        self.results.iter().filter(|r| f(&r.outcome)).count()
    }

    /// Cases whose result was compared with the oracle's.
    pub fn compared(&self) -> usize {
        self.count(Outcome::is_compared)
    }

    /// Cases that passed.
    pub fn passed(&self) -> usize {
        self.count(|o| *o == Outcome::Pass)
    }

    /// Cases that failed.
    pub fn failed(&self) -> usize {
        self.count(Outcome::is_failure)
    }

    /// Cases a subject skipped.
    pub fn skipped(&self) -> usize {
        self.count(|o| matches!(o, Outcome::Skipped(_)))
    }

    /// Cases that are documented differences.
    pub fn documented(&self) -> usize {
        self.count(|o| matches!(o, Outcome::Documented(_)))
    }

    /// Cases per reason that Heartwood rejects on purpose where the reference accepts them.
    pub fn documented_by_reason(&self) -> BTreeMap<&str, usize> {
        let mut out = BTreeMap::new();
        for r in &self.results {
            if let Outcome::Documented(reason) = &r.outcome {
                *out.entry(reason.as_str()).or_insert(0) += 1;
            }
        }
        out
    }

    /// Cases per reason that a subject skipped.
    pub fn skipped_by_reason(&self) -> BTreeMap<(&str, &str), usize> {
        let mut out = BTreeMap::new();
        for r in &self.results {
            if let Outcome::Skipped(reason) = &r.outcome {
                *out.entry((r.op.as_str(), reason.as_str())).or_insert(0) += 1;
            }
        }
        out
    }

    /// Cases per operation that have no Rust implementation yet.
    pub fn without_subject(&self) -> BTreeMap<&str, usize> {
        let mut out = BTreeMap::new();
        for r in self
            .results
            .iter()
            .filter(|r| r.outcome == Outcome::NoSubject)
        {
            *out.entry(r.op.as_str()).or_insert(0) += 1;
        }
        out
    }

    /// Whether no case failed.
    pub fn is_success(&self) -> bool {
        self.failed() == 0
    }
}

/// One line about where vectors came from: reference commit, oracle mode, Node, backend.
pub fn describe_meta(meta: &Meta) -> String {
    let short = |s: &Option<String>| {
        s.as_deref()
            .map(|c| c.chars().take(9).collect::<String>())
            .unwrap_or_else(|| "unknown".into())
    };
    let dirty = if meta.solar.dirty == Some(true) {
        " (dirty)"
    } else {
        ""
    };
    let oracle = match &meta.oracle.image {
        Some(image) => format!("container {image}"),
        None => meta.oracle.mode.clone(),
    };
    format!(
        "reference {} {}{dirty} (tree {}), {oracle}, Node {}, bcrypto {}",
        meta.solar.version,
        short(&meta.solar.commit),
        short(&meta.solar.tree),
        meta.node,
        meta.bcrypto
    )
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "{}: {} records in {} files",
            self.title,
            self.results.len(),
            self.files
        )?;
        writeln!(f, "  oracle: {}", self.source)?;
        writeln!(
            f,
            "  {} compared: {} passed, {} failed",
            self.compared(),
            self.passed(),
            self.failed()
        )?;
        if self.documented() > 0 {
            writeln!(
                f,
                "  {} documented differences (Heartwood rejects them on purpose):",
                self.documented()
            )?;
            for (reason, n) in self.documented_by_reason() {
                writeln!(f, "    {n:>5}  {reason}")?;
            }
        }
        if self.skipped() > 0 {
            writeln!(
                f,
                "  {} skipped (not modelled by the subject):",
                self.skipped()
            )?;
            for ((op, reason), n) in self.skipped_by_reason() {
                writeln!(f, "    {n:>5}  {op}: {reason}")?;
            }
        }
        let missing = self.without_subject();
        if !missing.is_empty() {
            let total: usize = missing.values().sum();
            writeln!(f, "  {total} records have no Rust implementation yet:")?;
            for (op, n) in &missing {
                writeln!(f, "    {n:>5}  {op}")?;
            }
        }
        for r in self.results.iter().filter(|r| r.outcome.is_failure()) {
            writeln!(
                f,
                "  FAIL {}:{} {} (height {})",
                r.file, r.line, r.label, r.height
            )?;
            match &r.outcome {
                Outcome::Mismatch(differences) => {
                    for d in differences.iter().take(SHOWN_DIFFERENCES) {
                        writeln!(f, "      {}", d.to_string().replace('\n', "\n      "))?;
                    }
                    if differences.len() > SHOWN_DIFFERENCES {
                        writeln!(
                            f,
                            "      ... and {} more differences",
                            differences.len() - SHOWN_DIFFERENCES
                        )?;
                    }
                }
                Outcome::VerdictMismatch { oracle, heartwood } => {
                    writeln!(f, "      oracle {oracle}, Heartwood {heartwood}")?;
                }
                Outcome::ClassMismatch { oracle, heartwood } => {
                    writeln!(
                        f,
                        "      oracle rejected ({oracle}), Heartwood rejected ({heartwood})"
                    )?;
                }
                Outcome::Mislabelled(reason) => writeln!(
                    f,
                    "      refused as a documented difference, but the input does not have its \
                     form: {reason}"
                )?,
                Outcome::HarnessError(message) => writeln!(f, "      harness error: {message}")?,
                _ => {}
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn case(op: &str, outcome: Outcome) -> CaseResult {
        CaseResult {
            file: "V01.jsonl".into(),
            line: 2,
            op: op.into(),
            label: op.into(),
            height: 2,
            outcome,
        }
    }

    #[test]
    fn counts_and_text() {
        let report = Report {
            title: "L0 golden vectors".into(),
            source: "test".into(),
            files: 1,
            results: vec![
                case("a", Outcome::Pass),
                case(
                    "b",
                    Outcome::Mismatch(vec![Difference::Value {
                        path: "$.x".into(),
                        expected: json!(1),
                        actual: json!(2),
                    }]),
                ),
                case("c", Outcome::NoSubject),
                case("c", Outcome::NoSubject),
                case("d", Outcome::Skipped("later".into())),
                case("e", Outcome::Documented("on purpose".into())),
            ],
        };
        assert_eq!(
            (report.compared(), report.passed(), report.failed()),
            (2, 1, 1)
        );
        assert!(!report.is_success());
        assert_eq!((report.skipped(), report.documented()), (1, 1));
        let text = report.to_string();
        assert!(
            text.contains("1 documented differences (Heartwood rejects them on purpose):\n        1  on purpose"),
            "{text}"
        );
        assert!(
            text.contains("1 skipped (not modelled by the subject):\n        1  d: later"),
            "{text}"
        );
        assert!(text.contains("2 compared: 1 passed, 1 failed"), "{text}");
        assert!(
            text.contains("2 records have no Rust implementation yet:\n        2  c"),
            "{text}"
        );
        assert!(
            text.contains("FAIL V01.jsonl:2 b (height 2)\n      $.x: expected 1, actual 2"),
            "{text}"
        );
    }
}
