//! The route corpus and its comparison: the queries of every served route (each criterion alone
//! and in pairs, orders, pages, unknown keys, invalid values, the 404 texts, the wallets, the peers'
//! filters, the pool after a submission, and a section of 101 requests for the rate limit), sent to
//! two nodes alternately at a paused height, and their answers judged: equal status codes,
//! `connection: close` and `X-Block-Height` present, bodies equal after the projections (the
//! volatile fields left out, the peer routes compared as sets by address), and each documented
//! difference behaving as documented.
//!
//! - [`corpus`]: the corpus files and their variables; [`http`]: the client;
//! - [`record`]: sending the corpus and recording the answers (`answers.jsonl`, `run.json`);
//! - [`judge`]: the projections and the outcome of each request;
//! - [`outside`]: a live run's answers to the `outside` section, which its recipe sends itself;
//! - [`parse_query_case`] and [`paginate_case`]: the query grammar's and the pagination's records
//!   of class V28, answered with `heartwood_api`.

pub mod corpus;
pub mod http;
pub mod judge;
pub mod outside;
pub mod record;

use std::net::{IpAddr, Ipv4Addr};
use std::path::{Path, PathBuf};

use heartwood_api::ApiRequest;
use heartwood_api::pagination::{Pagination, envelope};
use heartwood_api::schemas::{PARSED, parse_query, validate_params, validate_query};
use serde_json::{Map, Value, json};

use crate::errors::{Error, Result};
use crate::report::{CaseResult, Report};
use crate::subject::{Case, SubjectError, Verdict};

pub use judge::route_of;
use outside::OutsideAnswers;
pub use record::{ANSWERS, OUTSIDE, Recorded};

/// The committed route corpus, relative to the harness's directory.
pub const CORPUS_DIR: &str = "api-corpus";

/// Where a live run keeps the answers the recipe recorded, in the run directory.
pub const RUN_ANSWERS_DIR: &str = "api";

/// What `difftest api compare` compares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiCompareArgs {
    /// The answers a live run recorded at its pause, in its run directory.
    Recorded {
        /// The run directory.
        run: PathBuf,
    },
    /// Two live nodes, queried now.
    Live {
        /// The node under test's API.
        left: String,
        /// The control node's API.
        right: String,
        /// The route corpus.
        corpus: PathBuf,
        /// Where the answers and the report go.
        out: PathBuf,
    },
}

/// Compare the answers of `args`.
///
/// Recorded answers are every `answers.jsonl` under the run directory's `api/` (or under the
/// directory itself when it has no `api/`), as `api compare --left --right --out` writes them. A
/// request of the `outside` section left unsent there is judged from the run's
/// [`outside::RATE_LIMIT`] when the run has one.
/// Live, the corpus is sent to both nodes first, and the report is also written to
/// `report.txt` in the output directory.
pub fn compare(args: &ApiCompareArgs) -> Result<Report> {
    match args {
        ApiCompareArgs::Recorded { run } => {
            let root = run.join(RUN_ANSWERS_DIR);
            let root = if root.is_dir() { root } else { run.clone() };
            let mut files = Vec::new();
            answer_files(&root, &mut files)?;
            if files.is_empty() {
                return Err(Error::Corpus {
                    path: root,
                    message: format!("no recorded {ANSWERS}"),
                });
            }
            let outside = OutsideAnswers::load(&run.join(outside::RATE_LIMIT))?;
            judge_files(
                &files,
                format!("answers recorded under {}", root.display()),
                outside.as_ref(),
            )
        }
        ApiCompareArgs::Live {
            left,
            right,
            corpus,
            out,
        } => {
            record::record(left, right, corpus, out)?;
            let report = judge_files(
                &[out.join(ANSWERS)],
                format!("{left} (under test) against {right} (control)"),
                None,
            )?;
            let path = out.join("report.txt");
            std::fs::write(&path, report.to_string()).map_err(|e| Error::io(&path, e))?;
            Ok(report)
        }
    }
}

fn answer_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| Error::io(dir, e))?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            answer_files(&path, out)?;
        } else if path.file_name().is_some_and(|name| name == ANSWERS) {
            out.push(path);
        }
    }
    Ok(())
}

fn judge_files(
    files: &[PathBuf],
    source: String,
    outside: Option<&OutsideAnswers>,
) -> Result<Report> {
    let mut results = Vec::new();
    for path in files {
        let text = std::fs::read_to_string(path).map_err(|e| Error::io(path, e))?;
        for (index, line) in text.lines().enumerate().filter(|(_, l)| !l.is_empty()) {
            let recorded: Recorded = serde_json::from_str(line).map_err(|e| Error::Parse {
                path: path.clone(),
                line: index + 1,
                message: e.to_string(),
            })?;
            results.push(result(path, index + 1, &recorded, outside));
        }
    }
    Ok(Report {
        title: "API route corpus".into(),
        source,
        files: files.len(),
        results,
    })
}

fn result(
    path: &Path,
    line: usize,
    recorded: &Recorded,
    outside: Option<&OutsideAnswers>,
) -> CaseResult {
    if let Some(answers) = outside.filter(|_| OutsideAnswers::covers(recorded)) {
        return CaseResult {
            file: path.display().to_string(),
            line,
            op: format!("outside ({})", outside::RATE_LIMIT),
            label: match recorded.repeat {
                Some(repeat) => format!("{} #{}", recorded.name, repeat + 1),
                None => recorded.name.clone(),
            },
            height: 0,
            outcome: answers.judge(recorded),
        };
    }
    let op = match &recorded.request {
        Some(request) => match route_of(&request.method, &request.path) {
            Some(route) => format!("{} {}", request.method, route.path()),
            None => format!("{} (no route)", request.method),
        },
        None => "not sent".to_owned(),
    };
    let label = match recorded.repeat {
        Some(repeat) => format!("{} #{}", recorded.name, repeat + 1),
        None => recorded.name.clone(),
    };
    let height = match &recorded.right {
        Some(http::Reply::Answer { headers, .. }) => headers
            .get("x-block-height")
            .and_then(Value::as_str)
            .and_then(|h| h.parse().ok())
            .unwrap_or(0),
        _ => 0,
    };
    CaseResult {
        file: path.display().to_string(),
        line,
        op,
        label,
        height,
        outcome: judge::outcome(recorded),
    }
}

fn text<'a>(case: &'a Case<'_>, key: &str) -> std::result::Result<&'a str, SubjectError> {
    case.input[key]
        .as_str()
        .ok_or_else(|| SubjectError::Failed(format!("input.{key} is not text")))
}

/// `api.parseQuery`: a query string to the parsed object, with `heartwood_api::schemas`:
/// `{query}`, or `{status: 500}` where the reference's plugins throw.
pub fn parse_query_case(case: &Case<'_>) -> std::result::Result<Verdict, SubjectError> {
    let parsed = parse_query(text(case, "query")?);
    Ok(Verdict::Output(if parsed.is_null() {
        json!({ "status": 500 })
    } else {
        json!({ "query": parsed })
    }))
}

/// `api.paginate`: a list's path, query and count to its `meta` and the validated query that
/// its handler reads, with `heartwood_api::pagination`: `{meta, query}`, or `{status}` when the
/// path's parameters or the query are refused. The page holds as many matches as the page and
/// the count leave.
pub fn paginate_case(case: &Case<'_>) -> std::result::Result<Verdict, SubjectError> {
    let path = text(case, "path")?;
    let raw = text(case, "query")?;
    let total = case.input["total"]
        .as_u64()
        .ok_or_else(|| SubjectError::Failed("input.total is not a count".into()))?;
    let route = route_of("GET", path)
        .ok_or_else(|| SubjectError::Unsupported(format!("{path} is not a served route")))?;
    let params = params(route.path(), path);
    let status = |status: u16| Ok(Verdict::Output(json!({ "status": status })));
    let parsed = parse_query(raw);
    if parsed.is_null() {
        return status(500);
    }
    if let Err(error) = validate_params(route, &params) {
        return status(error.status);
    }
    let validated = match validate_query(route, &parsed) {
        Ok(validated) => validated,
        Err(error) => return status(error.status),
    };
    let pagination = Pagination::of(&validated);
    let count = pagination
        .limit
        .min(total.saturating_sub(pagination.offset));
    let request = ApiRequest {
        route,
        params,
        path: path.to_owned(),
        query: validated.clone(),
        payload: None,
        client: IpAddr::V4(Ipv4Addr::LOCALHOST),
    };
    let data = vec![Value::Null; usize::try_from(count).unwrap_or(0)];
    let body = envelope(&request, pagination, total, false, data);
    let mut query = validated;
    if let Some(map) = query.as_object_mut() {
        map.shift_remove(PARSED);
    }
    Ok(Verdict::Output(
        json!({ "meta": body["meta"], "query": query }),
    ))
}

/// The path parameters of `path` under the route pattern `pattern`.
fn params(pattern: &str, path: &str) -> Map<String, Value> {
    let path = path.strip_prefix("/api").unwrap_or(path);
    pattern
        .split('/')
        .zip(path.split('/'))
        .filter_map(|(p, s)| {
            let name = p.strip_prefix('{')?.strip_suffix('}')?;
            Some((name.to_owned(), Value::from(s)))
        })
        .collect()
}
