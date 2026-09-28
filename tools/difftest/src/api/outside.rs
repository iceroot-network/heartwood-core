//! The corpus's `outside` section in a live run: the live recipe sends it from a container whose
//! address no node whitelists and records the answers in `rate-limit.jsonl` beside `api/`, while
//! the corpus's runner, whose requests the nodes whitelist, leaves the section's requests unsent.
//! A recorded request of the section that was not sent is judged from those answers: the same
//! status, the same header names and the same body where the recipe records one (a 429's) on
//! both nodes. An answer that failed or has no status is a harness error, and so is a section in
//! which a node never answered 429: without one, nothing shows that the requests came from
//! outside the whitelist and reached the rate limit.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::Value;

use super::record::{OUTSIDE, Recorded};
use crate::compare::Difference;
use crate::errors::{Error, Result};
use crate::report::Outcome;

/// The file of the outside section's answers, in a live run's directory.
pub const RATE_LIMIT: &str = "rate-limit.jsonl";

/// Why the corpus's runner did not send a request of the outside section.
pub fn not_sent() -> String {
    format!("{OUTSIDE} is not set")
}

/// The outside section's answers, by node (`left`, `right`), case and request number (from 1).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct OutsideAnswers(BTreeMap<(String, String, u64), Value>);

impl OutsideAnswers {
    /// The answers in `path`; `None` when there is no such file.
    pub fn load(path: &Path) -> Result<Option<OutsideAnswers>> {
        if !path.exists() {
            return Ok(None);
        }
        let text = std::fs::read_to_string(path).map_err(|e| Error::io(path, e))?;
        let mut answers = BTreeMap::new();
        for (index, line) in text.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let answer: Value = serde_json::from_str(line).map_err(|e| Error::Parse {
                path: path.to_path_buf(),
                line: index + 1,
                message: e.to_string(),
            })?;
            let node = answer.get("node").and_then(Value::as_str);
            let case = answer.get("case").and_then(Value::as_str);
            if let (Some(node), Some(case), Some(i)) =
                (node, case, answer.get("i").and_then(Value::as_u64))
            {
                answers.insert((node.to_owned(), case.to_owned(), i), answer);
            }
        }
        Ok(Some(OutsideAnswers(answers)))
    }

    /// Whether `line` is a request of the outside section that the corpus's runner did not send.
    pub fn covers(line: &Recorded) -> bool {
        line.outside && line.request.is_none() && line.skipped.as_deref() == Some(&not_sent())
    }

    /// Whether `node` answered 429 to a request of the section.
    fn limited(&self, node: &str) -> bool {
        let limited = |answer: &Value| answer.get("status").and_then(Value::as_u64) == Some(429);
        (self.0.iter()).any(|((by, _, _), answer)| by == node && limited(answer))
    }

    /// The outcome of the outside request `line`, from the answers of both nodes to it.
    pub fn judge(&self, line: &Recorded) -> Outcome {
        let i = line.repeat.map_or(1, |r| u64::from(r) + 1);
        let answer = |node: &str| self.0.get(&(node.to_owned(), line.name.clone(), i));
        let (Some(left), Some(right)) = (answer("left"), answer("right")) else {
            return Outcome::Skipped(format!(
                "{RATE_LIMIT} holds no answer of both nodes to request {i}"
            ));
        };
        for (node, answer) in [("left", left), ("right", right)] {
            let status = answer.get("status").and_then(Value::as_u64);
            let failed = match (answer.get("error"), status) {
                (Some(error), _) => Some(error.to_string()),
                (None, None) => Some("no status".to_owned()),
                (None, Some(_)) => None,
            };
            if let Some(why) = failed {
                return Outcome::HarnessError(format!(
                    "request {i} has no answer of the {node} node: {why}"
                ));
            }
            if !self.limited(node) {
                return Outcome::HarnessError(format!(
                    "no answer of the {node} node in {RATE_LIMIT} is a 429"
                ));
            }
        }
        let names = |a: &Value| -> Value {
            let mut names: Vec<&String> = (a.get("headers").and_then(Value::as_object))
                .map(|headers| headers.keys().collect())
                .unwrap_or_default();
            names.sort();
            Value::from(names.into_iter().cloned().collect::<Vec<_>>())
        };
        let field = |a: &Value, key: &str| a.get(key).cloned().unwrap_or(Value::Null);
        let differences: Vec<Difference> = [
            ("status", field(right, "status"), field(left, "status")),
            ("body", field(right, "body"), field(left, "body")),
            ("header names", names(right), names(left)),
        ]
        .into_iter()
        .filter(|(_, expected, actual)| expected != actual)
        .map(|(path, expected, actual)| Difference::Value {
            path: path.to_owned(),
            expected,
            actual,
        })
        .collect();
        if differences.is_empty() {
            Outcome::Pass
        } else {
            Outcome::Mismatch(differences)
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn line(name: &str, repeat: Option<u32>) -> Recorded {
        Recorded {
            file: "rate-limit.json".into(),
            name: name.into(),
            repeat,
            request: None,
            outside: true,
            documented: None,
            skipped: Some(not_sent()),
            left: None,
            right: None,
        }
    }

    #[test]
    fn an_unsent_outside_request_is_judged_from_the_recorded_answers() {
        let dir = std::env::temp_dir().join(format!("heartwood-outside-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(RATE_LIMIT);
        assert_eq!(OutsideAnswers::load(&path).unwrap(), None);
        let answer = |node: &str, i: u64, status: u16| {
            json!({"node": node, "case": "rate-limit/a", "i": i, "status": status,
                   "headers": {"x-block-height": "9", "connection": "close"}})
        };
        let lines = [
            answer("left", 1, 422),
            answer("right", 1, 422),
            answer("left", 2, 429),
            answer("right", 2, 422),
            answer("left", 3, 429),
            answer("left", 4, 429),
            answer("right", 4, 429),
        ];
        let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
        std::fs::write(&path, text).unwrap();
        let answers = OutsideAnswers::load(&path).unwrap().unwrap();
        std::fs::remove_dir_all(&dir).unwrap();

        assert!(OutsideAnswers::covers(&line("rate-limit/a", Some(0))));
        let mut sent = line("rate-limit/a", Some(0));
        sent.skipped = None;
        assert!(!OutsideAnswers::covers(&sent));

        assert_eq!(answers.judge(&line("rate-limit/a", Some(0))), Outcome::Pass);
        assert!(answers.judge(&line("rate-limit/a", Some(1))).is_failure());
        assert!(matches!(
            answers.judge(&line("rate-limit/a", Some(2))),
            Outcome::Skipped(_)
        ));
        assert!(matches!(
            answers.judge(&line("rate-limit/b", None)),
            Outcome::Skipped(_)
        ));
    }

    #[test]
    fn a_failed_request_or_a_section_without_a_429_is_a_harness_error() {
        let judge = |lines: &[Value], i: u32| {
            let answers = lines.iter().map(|answer| {
                let key = (answer["node"].as_str().unwrap().to_owned(), "c".to_owned());
                (
                    (key.0, key.1, answer["i"].as_u64().unwrap()),
                    answer.clone(),
                )
            });
            OutsideAnswers(answers.collect()).judge(&line("c", Some(i)))
        };
        let failed = |node: &str, i: u64| json!({"node": node, "case": "c", "i": i, "error": "connect ECONNREFUSED"});
        let answered = |node: &str, i: u64, status: u16| json!({"node": node, "case": "c", "i": i, "status": status, "headers": {}});
        let both_failed = [failed("left", 1), failed("right", 1)];
        assert!(matches!(judge(&both_failed, 0), Outcome::HarnessError(_)));
        let limited = [answered("left", 2, 429), answered("right", 2, 429)];
        let mut one_failed = vec![answered("left", 1, 422), failed("right", 1)];
        one_failed.extend(limited.clone());
        assert!(matches!(judge(&one_failed, 0), Outcome::HarnessError(_)));
        let no_status = json!({"node": "right", "case": "c", "i": 1, "headers": {}});
        let mut no_status = vec![answered("left", 1, 422), no_status];
        no_status.extend(limited.clone());
        assert!(matches!(judge(&no_status, 0), Outcome::HarnessError(_)));
        // Equal answers of a section that never reached the rate limit prove nothing.
        let whitelisted = [answered("left", 1, 422), answered("right", 1, 422)];
        assert!(matches!(judge(&whitelisted, 0), Outcome::HarnessError(_)));
        let mut reached = whitelisted.to_vec();
        reached.extend(limited);
        assert_eq!(judge(&reached, 0), Outcome::Pass);
    }
}
