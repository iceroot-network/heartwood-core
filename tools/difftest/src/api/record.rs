//! Sending the route corpus to two nodes and recording their answers.
//!
//! Each case goes to the node under test (left), then to the control node (right), query by
//! query, and one line per request is written to `answers.jsonl` in the output directory (see
//! [`Recorded`]); `run.json` says what ran. The variables a case binds come from the control
//! node's answer.

use std::io::Write;
use std::net::IpAddr;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::corpus::{self, Case, Vars};
use super::http::{Node, Reply, Request};
use crate::errors::{Error, Result};

/// The recorded answers in a run's output directory.
pub const ANSWERS: &str = "answers.jsonl";

/// The environment variable naming the local address of the `outside` section.
pub const OUTSIDE: &str = "DIFFTEST_API_OUTSIDE";

/// One request of the corpus and both nodes' answers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Recorded {
    /// The corpus file.
    pub file: String,
    /// The case.
    pub name: String,
    /// The repetition, from 0, for a case sent several times.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repeat: Option<u32>,
    /// The request as sent (variables replaced).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request: Option<Request>,
    /// Sent from the `outside` address.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub outside: bool,
    /// The documented difference the case may show.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub documented: Option<String>,
    /// Why the case was not sent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skipped: Option<String>,
    /// The node under test's answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub left: Option<Reply>,
    /// The control node's answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub right: Option<Reply>,
}

/// Send the corpus in `corpus` to `left` and `right` and record the answers in `out`.
pub fn record(left: &str, right: &str, corpus: &Path, out: &Path) -> Result<()> {
    let files = corpus::load(corpus)?;
    let (left_node, right_node) = (
        Node::parse(left).map_err(Error::Usage)?,
        Node::parse(right).map_err(Error::Usage)?,
    );
    let outside: Option<IpAddr> = match std::env::var(OUTSIDE) {
        Ok(text) if !text.is_empty() => Some(
            text.parse()
                .map_err(|_| Error::Usage(format!("{OUTSIDE}={text} is not an address")))?,
        ),
        _ => None,
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| Error::io(out, e))?;
    std::fs::create_dir_all(out).map_err(|e| Error::io(out, e))?;
    let path = out.join(ANSWERS);
    let mut answers =
        std::io::BufWriter::new(std::fs::File::create(&path).map_err(|e| Error::io(&path, e))?);
    let started = now();
    let mut vars = Vars::new();
    let mut count = 0usize;
    for file in &files {
        for case in &file.cases {
            let times = case.repeat.unwrap_or(1).max(1);
            for index in 0..times {
                let mut line = Recorded {
                    file: file.name.clone(),
                    name: case.name.clone(),
                    repeat: case.repeat.map(|_| index),
                    request: None,
                    outside: case.source.as_deref() == Some("outside"),
                    documented: case.documented.clone(),
                    skipped: None,
                    left: None,
                    right: None,
                };
                match request(case, &vars) {
                    Err(unbound) => line.skipped = Some(format!("${{{unbound}}} is not bound")),
                    Ok(_) if line.outside && outside.is_none() => {
                        line.skipped = Some(format!("{OUTSIDE} is not set"));
                    }
                    Ok(sent) => {
                        let source = if line.outside { outside } else { None };
                        let l = runtime.block_on(left_node.send(&sent, source));
                        let r = runtime.block_on(right_node.send(&sent, source));
                        if index == 0 {
                            bind(case, &l, &r, &mut vars);
                        }
                        line.request = Some(sent);
                        line.left = Some(l);
                        line.right = Some(r);
                    }
                }
                let text = serde_json::to_string(&line).map_err(|e| Error::Corpus {
                    path: path.clone(),
                    message: e.to_string(),
                })?;
                writeln!(answers, "{text}").map_err(|e| Error::io(&path, e))?;
                count += 1;
            }
        }
    }
    answers.flush().map_err(|e| Error::io(&path, e))?;
    let run = json!({
        "left": left,
        "right": right,
        "corpus": corpus.display().to_string(),
        "files": files.iter().map(|f| f.name.clone()).collect::<Vec<_>>(),
        "requests": count,
        "outside": outside.map(|a| a.to_string()),
        "started": started,
        "finished": now(),
        "bindings": vars,
    });
    let run_path = out.join("run.json");
    std::fs::write(&run_path, format!("{run:#}\n")).map_err(|e| Error::io(&run_path, e))
}

/// The request of `case` with the variables replaced, or the first unbound variable.
fn request(case: &Case, vars: &Vars) -> std::result::Result<Request, String> {
    let mut headers = Vec::new();
    for (name, value) in &case.headers {
        headers.push((name.clone(), corpus::substitute(value, vars)?));
    }
    let mut body = match (&case.body, &case.text) {
        (Some(json), _) => {
            if !case
                .headers
                .keys()
                .any(|h| h.eq_ignore_ascii_case("content-type"))
            {
                headers.push(("content-type".into(), "application/json".into()));
            }
            Some(json.to_string())
        }
        (None, Some(text)) => Some(corpus::substitute(text, vars)?),
        (None, None) => None,
    };
    if let (Some(pad), Some(text)) = (case.pad_to, body.as_mut()) {
        let missing = pad.saturating_sub(text.len());
        text.push_str(&" ".repeat(missing));
    }
    Ok(Request {
        method: case.method.clone(),
        path: corpus::substitute(&case.path, vars)?,
        query: corpus::substitute(case.query.as_deref().unwrap_or(""), vars)?,
        headers,
        body,
    })
}

/// Bind the case's variables from the control node's answer (when it is a 200).
fn bind(case: &Case, left: &Reply, right: &Reply, vars: &mut Vars) {
    let Reply::Answer {
        status: 200,
        body: right_body,
        ..
    } = right
    else {
        return;
    };
    let left_body = match left {
        Reply::Answer { body, .. } => body.clone(),
        Reply::Failed { .. } => Value::Null,
    };
    for (name, path) in &case.bind {
        if let Some(value) = corpus::bind(path, right_body, &left_body) {
            vars.insert(name.clone(), value);
        }
    }
}

/// The time now, in UTC, to the second.
fn now() -> String {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default();
    let days = seconds / 86_400;
    let (hour, minute, second) = (seconds % 86_400 / 3600, seconds % 3600 / 60, seconds % 60);
    let (year, month, day) = civil(days);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// The calendar date `days` days after 1970-01-01.
fn civil(days: u64) -> (u64, u64, u64) {
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + u64::from(month <= 2);
    (year, month, day)
}
