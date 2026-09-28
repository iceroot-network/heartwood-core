//! The route corpus: its cases, and the variables a case binds from an answer and later cases use.
//!
//! A corpus is a directory of JSON files, read in the order of their names, each an array of
//! cases:
//!
//! - `name` (unique in the corpus), `method` (`GET` when absent), `path` (the whole path, with
//!   `/api`), `query` (the raw query string, without `?`);
//! - `headers` (request headers), `body` (a JSON body, sent as `application/json`) or `text` (a
//!   body as it is), and `padTo` (spaces added to the body up to this many bytes);
//! - `repeat` (send it this many times), `source: "outside"` (send it from the local address in
//!   `DIFFTEST_API_OUTSIDE`, which the nodes' rate-limit whitelist does not name);
//! - `bind` (variables from the control node's answer, by path, such as `data.0.id`; a `*`
//!   segment takes the first element whose value at the rest of the path the other node's answer
//!   also has there);
//! - `documented` (the documented difference the case may show: `heightAboveTip`,
//!   `storeFailure`, `peersIpFilter`, `tieOrder` or `decodeReason`).
//!
//! `path`, `query`, the header values and `text` may use `${name}`, `${name+1}`, `${name-1}`,
//! `${name|upper}`, `${name|uri}` (escaped as `encodeURIComponent` does) and `${name|head:N}`
//! (the first N characters). A case whose variable is not bound is recorded as skipped.

use std::collections::BTreeMap;
use std::path::Path;

use heartwood_crypto::utils::js;
use serde::Deserialize;
use serde_json::Value;

use crate::errors::{Error, Result};

/// One query of the corpus.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Case {
    /// The case's name, unique in the corpus.
    pub name: String,
    /// The HTTP method.
    #[serde(default = "get")]
    pub method: String,
    /// The path, with the base path.
    pub path: String,
    /// The raw query string.
    #[serde(default)]
    pub query: Option<String>,
    /// Request headers.
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    /// A JSON body.
    #[serde(default)]
    pub body: Option<Value>,
    /// A body as it is.
    #[serde(default)]
    pub text: Option<String>,
    /// The body's length after padding with spaces.
    #[serde(default)]
    pub pad_to: Option<usize>,
    /// How many times the case is sent.
    #[serde(default)]
    pub repeat: Option<u32>,
    /// `outside`: sent from the address in `DIFFTEST_API_OUTSIDE`.
    #[serde(default)]
    pub source: Option<String>,
    /// Variables bound from the control node's answer, by path.
    #[serde(default)]
    pub bind: BTreeMap<String, String>,
    /// The documented difference the case may show.
    #[serde(default)]
    pub documented: Option<String>,
}

fn get() -> String {
    "GET".to_owned()
}

/// The cases of one corpus file.
#[derive(Debug, Clone)]
pub struct CorpusFile {
    /// The file's name.
    pub name: String,
    /// Its cases, in order.
    pub cases: Vec<Case>,
}

/// The corpus in `dir`: every `*.json` file, in the order of the names.
pub fn load(dir: &Path) -> Result<Vec<CorpusFile>> {
    let entries = std::fs::read_dir(dir).map_err(|e| Error::io(dir, e))?;
    let mut names: Vec<String> = entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".json"))
        .collect();
    names.sort();
    let mut files = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for name in names {
        let path = dir.join(&name);
        let text = std::fs::read_to_string(&path).map_err(|e| Error::io(&path, e))?;
        let cases: Vec<Case> = serde_json::from_str(&text).map_err(|e| Error::Corpus {
            path: path.clone(),
            message: e.to_string(),
        })?;
        if let Some(case) = cases.iter().find(|case| !seen.insert(case.name.clone())) {
            return Err(Error::Corpus {
                path,
                message: format!("the case name {:?} is used twice", case.name),
            });
        }
        files.push(CorpusFile { name, cases });
    }
    if files.is_empty() {
        return Err(Error::Corpus {
            path: dir.to_owned(),
            message: "no corpus file (*.json)".into(),
        });
    }
    Ok(files)
}

/// The variables bound so far.
pub type Vars = BTreeMap<String, String>;

/// `text` with its variables replaced, or the name of the first one that is not bound.
pub fn substitute(text: &str, vars: &Vars) -> std::result::Result<String, String> {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("${") {
        out.push_str(rest.get(..start).unwrap_or(""));
        let after = rest.get(start + 2..).unwrap_or("");
        let Some(end) = after.find('}') else {
            return Err(format!("an unclosed ${{ in {text:?}"));
        };
        out.push_str(&expand(after.get(..end).unwrap_or(""), vars)?);
        rest = after.get(end + 1..).unwrap_or("");
    }
    out.push_str(rest);
    Ok(out)
}

/// One `${...}`: a name, an optional `+N` or `-N`, and an optional `|filter`.
fn expand(expression: &str, vars: &Vars) -> std::result::Result<String, String> {
    let (reference, filter) = match expression.split_once('|') {
        Some((reference, filter)) => (reference, Some(filter)),
        None => (expression, None),
    };
    let (name, offset) = match reference.find(['+', '-']) {
        Some(at) => (
            reference.get(..at).unwrap_or(""),
            reference.get(at..).and_then(|n| n.parse::<i64>().ok()),
        ),
        None => (reference, None),
    };
    let mut value = vars.get(name).cloned().ok_or_else(|| name.to_owned())?;
    if let Some(offset) = offset {
        let number: i64 = value
            .parse()
            .map_err(|_| format!("{name} is not a number"))?;
        value = (number + offset).to_string();
    }
    Ok(match filter {
        None => value,
        Some("upper") => value.to_uppercase(),
        Some("uri") => encode_uri_component(&value),
        Some(other) => match other.strip_prefix("head:").and_then(|n| n.parse().ok()) {
            Some(count) => value.chars().take(count).collect(),
            None => return Err(format!("unknown filter {other}")),
        },
    })
}

/// `encodeURIComponent`.
pub fn encode_uri_component(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&byte) {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// The value at `path` of the control node's answer `right`, as text: a `*` segment takes the
/// first element whose value at the rest of the path is also found there in `left`.
pub fn bind(path: &str, right: &Value, left: &Value) -> Option<String> {
    let segments: Vec<&str> = path.split('.').collect();
    let value = at(right, &segments, Some(left))?;
    Some(match value {
        Value::String(text) => text,
        Value::Number(number) => js::number_to_string(js::number_value(&number)),
        other => other.to_string(),
    })
}

fn at(value: &Value, segments: &[&str], other: Option<&Value>) -> Option<Value> {
    let Some((first, rest)) = segments.split_first() else {
        return Some(value.clone());
    };
    if *first == "*" {
        let others: Vec<Value> = other
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| at(item, rest, None))
                    .collect()
            })
            .unwrap_or_default();
        return value
            .as_array()?
            .iter()
            .filter_map(|item| at(item, rest, None))
            .find(|found| others.contains(found));
    }
    let next = match value {
        Value::Array(items) => items.get(first.parse::<usize>().ok()?),
        Value::Object(map) => map.get(*first),
        _ => None,
    }?;
    let other_next = other.and_then(|o| match o {
        Value::Array(items) => first.parse::<usize>().ok().and_then(|i| items.get(i)),
        Value::Object(map) => map.get(*first),
        _ => None,
    });
    at(next, rest, other_next)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn variables_expand_with_offsets_and_filters() {
        let vars: Vars = [
            ("tip".to_owned(), "265".to_owned()),
            ("id".to_owned(), "ab c".to_owned()),
        ]
        .into_iter()
        .collect();
        assert_eq!(
            substitute("/blocks/${tip+1}", &vars).unwrap(),
            "/blocks/266"
        );
        assert_eq!(
            substitute("${tip-5}&${id|upper}", &vars).unwrap(),
            "260&AB C"
        );
        assert_eq!(
            substitute("m=${id|uri}&h=${id|head:2}", &vars).unwrap(),
            "m=ab%20c&h=ab"
        );
        assert_eq!(substitute("${nope}", &vars), Err("nope".to_owned()));
    }

    #[test]
    fn a_star_binds_what_both_answers_hold() {
        let right = json!({"data": [{"ip": "192.0.2.2"}, {"ip": "192.0.2.3"}]});
        let left = json!({"data": [{"ip": "192.0.2.1"}, {"ip": "192.0.2.3"}]});
        assert_eq!(
            bind("data.*.ip", &right, &left).as_deref(),
            Some("192.0.2.3")
        );
        assert_eq!(
            bind("data.0.ip", &right, &left).as_deref(),
            Some("192.0.2.2")
        );
        assert_eq!(bind("data.5.ip", &right, &left), None);
    }

    #[test]
    fn the_committed_corpus_marks_what_its_answers_can_show() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join(crate::api::CORPUS_DIR);
        let files = load(&dir).unwrap();
        let cases: Vec<&Case> = files.iter().flat_map(|f| &f.cases).collect();
        let marked = |kind: &str| -> Vec<&Case> {
            cases
                .iter()
                .copied()
                .filter(|c| c.documented.as_deref() == Some(kind))
                .collect()
        };
        for case in &cases {
            if let Some(kind) = &case.documented {
                assert!(
                    [
                        "heightAboveTip",
                        "storeFailure",
                        "peersIpFilter",
                        "tieOrder",
                        "decodeReason"
                    ]
                    .contains(&kind.as_str()),
                    "{}: {kind}",
                    case.name
                );
            }
        }
        // A store failure a client can cause: a memo pattern that ends with its escape character.
        let failures = marked("storeFailure");
        assert!(!failures.is_empty());
        for case in failures {
            let query = case.query.as_deref().unwrap_or("");
            assert!(
                query.contains("memo=") && query.ends_with("%5C"),
                "{}",
                case.name
            );
        }
        // A tie can only be seen in the sorted column: the resource has no `sequence`, the raw
        // data (`transform=false`) no `timestamp`.
        for case in marked("tieOrder") {
            let query = case.query.as_deref().unwrap_or("");
            let raw = query.contains("transform=false");
            let property = query
                .split('&')
                .find_map(|p| p.strip_prefix("orderBy="))
                .and_then(|o| o.split(':').next())
                .unwrap_or_else(|| panic!("{}: a tie of no order", case.name));
            match property {
                "sequence" => assert!(raw, "{}", case.name),
                "timestamp" => assert!(!raw, "{}", case.name),
                _ => {}
            }
        }
    }
}
