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
//! - [`judge`]: the projections and the outcome of each request; [`missed`]: the reference's
//!   answer to `/blocks/missed` over a store's rows, for a node whose start wrote rows the
//!   control's store does not hold;
//! - [`outside`]: a live run's answers to the `outside` section, which its recipe sends itself;
//! - [`parse_query_case`] and [`paginate_case`]: the query grammar's and the pagination's records
//!   of class V28, answered with `heartwood_api`.

pub mod corpus;
pub mod http;
pub mod judge;
pub mod missed;
pub mod outside;
pub mod record;

use std::collections::BTreeSet;
use std::net::{IpAddr, Ipv4Addr};
use std::path::{Path, PathBuf};

use heartwood_api::ApiRequest;
use heartwood_api::pagination::{Pagination, envelope};
use heartwood_api::schemas::{PARSED, parse_query, validate_params, validate_query};
use serde_json::{Map, Value, json};

use crate::errors::{Error, Result};
use crate::live::run::{self as live_run, Manifest};
use crate::report::{CaseResult, Outcome, Report};
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
    /// The answers of one pair of nodes a live run recorded: the route corpus whose nodes the
    /// run's `run.json` names as `left` against `right` (the first corpus, in `api/`, or one of
    /// its `apiExtra`), judged as the whole run judges it.
    Pair {
        /// The run directory.
        run: PathBuf,
        /// The node under test, as the run names it (`node4`).
        left: String,
        /// The control node, as the run names it (`node2`).
        right: String,
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
/// [`outside::RATE_LIMIT`] when the run has one. Then the corpora the run's `run.json` lists as
/// `apiExtra`, each from its own directory, without the `outside` section (the recipe sends it
/// once, for the first corpus's nodes) and without the posts of a body the first corpus posted to
/// the same control node, which the control answered from its cooldown; the report's source
/// counts both. One pair's answers ([`ApiCompareArgs::Pair`]) are judged the same way: the first
/// corpus with its `outside` section, or one corpus of `apiExtra` alone. A live run's failed cases
/// are then judged again with its own records ([`crate::live::evidence`]), as its live check
/// judges them.
/// Live, the corpus is sent to both nodes first, and the report is also written to
/// `report.txt` in the output directory.
pub fn compare(args: &ApiCompareArgs) -> Result<Report> {
    match args {
        ApiCompareArgs::Recorded { run } => judge_run(run, None),
        ApiCompareArgs::Pair { run, left, right } => {
            judge_run(run, Some((left.as_str(), right.as_str())))
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

/// The pairs a run's manifest names, for a refusal: `node5 against node3 (api), ...`.
fn recorded_pairs(manifest: Option<&Manifest>) -> String {
    let Some(manifest) = manifest else {
        return "no run.json that loads".to_owned();
    };
    let first = (manifest.api.iter()).map(|(l, r)| format!("{l} against {r} ({RUN_ANSWERS_DIR})"));
    let extra =
        (manifest.api_extra.iter()).map(|e| format!("{} against {} ({})", e.left, e.right, e.dir));
    let pairs: Vec<String> = first.chain(extra).collect();
    if pairs.is_empty() {
        "none".to_owned()
    } else {
        pairs.join(", ")
    }
}

/// Judge the answers a live run recorded in `run`: every route corpus it recorded, or the one of
/// `pair` (its node under test and its control node, as `run.json` names them); then a live run's
/// failed cases again with its own records ([`crate::live::evidence`]).
fn judge_run(run: &Path, pair: Option<(&str, &str)>) -> Result<Report> {
    let (report, manifest) = judge_recorded(run, pair)?;
    Ok(match &manifest {
        Some(manifest) => crate::live::evidence::judged_with_evidence(run, manifest, report),
        None => report,
    })
}

/// [`judge_run`]'s answers judged as they were recorded, and the run's `run.json` when it loads.
pub(crate) fn judge_recorded(
    run: &Path,
    pair: Option<(&str, &str)>,
) -> Result<(Report, Option<Manifest>)> {
    let root = run.join(RUN_ANSWERS_DIR);
    let root = if root.is_dir() {
        root
    } else {
        run.to_path_buf()
    };
    let mut files = Vec::new();
    answer_files(&root, &mut files)?;
    if files.is_empty() {
        return Err(Error::Corpus {
            path: root,
            message: format!("no recorded {ANSWERS}"),
        });
    }
    let manifest = Manifest::load(run).ok();
    let first_pair = manifest.as_ref().and_then(|m| m.api.as_ref());
    let all_extra: Vec<&live_run::ApiCorpus> = manifest.iter().flat_map(|m| &m.api_extra).collect();
    // The corpora to judge: every one, or the first alone, or one of `apiExtra` alone.
    let (judge_first, extras) = match pair {
        None => (true, all_extra),
        Some((l, r)) if first_pair.is_some_and(|(a, b)| a == l && b == r) => (true, Vec::new()),
        Some((l, r)) => {
            let found: Vec<&live_run::ApiCorpus> = (all_extra.into_iter())
                .filter(|e| e.left == l && e.right == r)
                .collect();
            if found.is_empty() {
                return Err(Error::Corpus {
                    path: run.join("run.json"),
                    message: format!(
                        "the run recorded no route corpus of {l} against {r}; it recorded {}",
                        recorded_pairs(manifest.as_ref())
                    ),
                });
            }
            (false, found)
        }
    };
    let mut report = if judge_first {
        let outside = OutsideAnswers::load(&run.join(outside::RATE_LIMIT))?;
        let source = format!("answers recorded under {}", root.display());
        judge_files(&files, source, outside.as_ref())?
    } else {
        judge_files(&[], String::new(), None)?
    };
    for extra in extras {
        let path = run.join(&extra.dir).join(ANSWERS);
        let right = first_pair.map(|a| &a.1);
        let posted = match right {
            Some(right) if *right == extra.right => posted_bodies(&files)?,
            _ => BTreeSet::new(),
        };
        let mut more = judge_files(std::slice::from_ref(&path), String::new(), None)?;
        let answers = read_answers(&path)?;
        let (mut unsent, mut repeats) = (0, 0);
        let kept = more
            .results
            .drain(..)
            .zip(&answers)
            .filter(|(_, (_, recorded))| {
                let (outside, repeat) = (
                    OutsideAnswers::covers(recorded),
                    repeated_posts(recorded, &posted),
                );
                unsent += usize::from(outside);
                repeats += usize::from(repeat && !outside);
                !outside && !repeat
            });
        report
            .results
            .extend(kept.map(|(case, _)| case).collect::<Vec<_>>());
        let before = if report.source.is_empty() {
            String::new()
        } else {
            format!("{}, and ", report.source)
        };
        report.source = format!(
            "{before}{} against {} under {} ({unsent} requests of the outside section, which the \
             recipe sends for the first corpus only, and {repeats} posts the control node \
             answered from its cooldown, not judged)",
            extra.left,
            extra.right,
            run.join(&extra.dir).display()
        );
        report.files += 1;
    }
    Ok((report, manifest))
}

/// The case of the relay that answered the answers a live run recorded in `run`: whether it is
/// `binary`, the release binary of the tree being judged.
///
/// `None` for a directory that is not a live run's (one written by `api compare --left --right
/// --out`), and for a run without a Heartwood relay. A live run's directory (one with an `api/`
/// directory, or whose `run.json` names the live-run format) must have a `run.json` that loads:
/// otherwise which relay answered is unknown, and the case is a harness error.
pub fn relay_case(run: &Path, binary: &Path) -> Option<CaseResult> {
    let path = run.join("run.json");
    let named = std::fs::read_to_string(&path).is_ok_and(|text| text.contains(live_run::FORMAT));
    let live = named || run.join(RUN_ANSWERS_DIR).is_dir();
    let outcome = match Manifest::load(run) {
        Ok(manifest) => match live_run::relay_binding(&manifest, binary)? {
            Ok(_) => Outcome::Pass,
            Err(why) => Outcome::HarnessError(why),
        },
        Err(error) if live => Outcome::HarnessError(format!(
            "run.json does not load ({error}): the relay that answered is unknown"
        )),
        Err(_) => return None,
    };
    Some(CaseResult {
        file: path.display().to_string(),
        line: 0,
        op: "relay".into(),
        label: "the relay that answered is this tree's".into(),
        height: 0,
        outcome,
    })
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

/// The recorded answers of `path`, in order.
fn read_answers(path: &Path) -> Result<Vec<(usize, Recorded)>> {
    let text = std::fs::read_to_string(path).map_err(|e| Error::io(path, e))?;
    let lines = text.lines().enumerate().filter(|(_, l)| !l.is_empty());
    lines
        .map(|(index, line)| {
            let recorded = serde_json::from_str(line).map_err(|e| Error::Parse {
                path: path.to_owned(),
                line: index + 1,
                message: e.to_string(),
            })?;
            Ok((index + 1, recorded))
        })
        .collect()
}

/// The bodies of the POST requests the answers of `files` record. A later corpus that posts one
/// of them to the same control node is not comparable there: the reference's pool answers a
/// transaction it has already tried with its cooldown (`ERR_COOLDOWN`, the seconds left), which
/// the node under test, never sent it, does not.
fn posted_bodies(files: &[PathBuf]) -> Result<BTreeSet<String>> {
    let mut bodies = BTreeSet::new();
    for path in files {
        for (_, recorded) in read_answers(path)? {
            let request = recorded.request.filter(|r| r.method == "POST");
            bodies.extend(request.and_then(|r| r.body));
        }
    }
    Ok(bodies)
}

/// Whether `recorded` posts one of `posted` again ([`posted_bodies`]) and the control node
/// answered it from its cooldown, which only a repeat reaches.
fn repeated_posts(recorded: &Recorded, posted: &BTreeSet<String>) -> bool {
    let request = recorded.request.as_ref().filter(|r| r.method == "POST");
    let repeated = request
        .and_then(|r| r.body.as_ref())
        .is_some_and(|body| posted.contains(body));
    let cooldown = match &recorded.right {
        Some(http::Reply::Answer { body, .. }) => body.to_string().contains("ERR_COOLDOWN"),
        _ => false,
    };
    repeated && cooldown
}

fn judge_files(
    files: &[PathBuf],
    source: String,
    outside: Option<&OutsideAnswers>,
) -> Result<Report> {
    let mut results = Vec::new();
    for path in files {
        for (line, recorded) in read_answers(path)? {
            results.push(result(path, line, &recorded, outside));
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
pub(crate) fn params(pattern: &str, path: &str) -> Map<String, Value> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_live_run_without_a_manifest_that_loads_is_a_harness_error() {
        let dir = std::env::temp_dir().join(format!("heartwood-relay-case-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(RUN_ANSWERS_DIR)).unwrap();
        let binary = dir.join("heartwood");
        let error = |case: Option<CaseResult>| match case.map(|case| case.outcome) {
            Some(Outcome::HarnessError(why)) => why,
            other => panic!("{other:?}"),
        };
        // A run's layout: `api/`, with no `run.json`, or a truncated one.
        assert!(error(relay_case(&dir, &binary)).starts_with("run.json does not load"));
        std::fs::write(dir.join("run.json"), r#"{"format": "heartwood-live-r"#).unwrap();
        assert!(error(relay_case(&dir, &binary)).ends_with("the relay that answered is unknown"));
        // `run.json` names the format though the answers lie elsewhere.
        std::fs::remove_dir_all(dir.join(RUN_ANSWERS_DIR)).unwrap();
        std::fs::write(dir.join("run.json"), r#"{"format": "heartwood-live-run/1""#).unwrap();
        assert!(error(relay_case(&dir, &binary)).starts_with("run.json does not load"));
        // A directory that `api compare --left --right --out` wrote is no run: no case.
        std::fs::remove_file(dir.join("run.json")).unwrap();
        assert!(relay_case(&dir, &binary).is_none());
        // A run's manifest that loads and names no Heartwood relay: no case either.
        let manifest = r#"{"format": "heartwood-live-run/1", "run": "r", "network": "devnet",
            "seed": "s", "nodes": [{"name": "node1", "ip": "10.0.0.1",
            "implementation": "reference"}]}"#;
        std::fs::write(dir.join("run.json"), manifest).unwrap();
        assert!(relay_case(&dir, &binary).is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A recorded answer of `name` to `request`, `left` and `right` its answers' status and body.
    fn answer(name: &str, request: Value, left: (u16, Value), right: (u16, Value)) -> String {
        let reply = |(status, body): (u16, Value)| {
            json!({"status": status, "body": body,
                "headers": {"connection": "close", "x-block-height": "212"}})
        };
        let line = json!({"file": "f.json", "name": name, "request": request,
            "left": reply(left), "right": reply(right)});
        format!("{line}\n")
    }

    #[test]
    fn a_runs_second_corpus_is_judged_without_the_posts_the_control_answered_from_its_cooldown() {
        let dir = std::env::temp_dir().join(format!("heartwood-api-extra-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for sub in [RUN_ANSWERS_DIR, "api-node6"] {
            std::fs::create_dir_all(dir.join(sub)).unwrap();
        }
        let manifest = json!({"format": live_run::FORMAT, "run": "w", "network": "devnet",
            "seed": "s", "nodes": [], "api": {"left": "node4", "right": "node1"},
            "apiExtra": [{"left": "node6", "right": "node1", "dir": "api-node6"}]});
        std::fs::write(dir.join("run.json"), manifest.to_string()).unwrap();
        let get = json!({"method": "GET", "path": "/api"});
        let missing = json!({"statusCode": 404, "error": "Not Found", "message": "Not Found"});
        let post = json!({"method": "POST", "path": "/api/transactions", "body": "{\"t\":1}"});
        let tried = |kind: &str| json!({"errors": {"x": {"type": kind}}});
        let first = answer(
            "g",
            get.clone(),
            (404, missing.clone()),
            (404, missing.clone()),
        ) + &answer(
            "p",
            post.clone(),
            (200, tried("ERR_APPLY")),
            (200, tried("ERR_APPLY")),
        );
        std::fs::write(dir.join(RUN_ANSWERS_DIR).join(ANSWERS), first).unwrap();
        // Node 6's corpus: the post again, which node 1 answers from its cooldown, and a GET
        // whose answers differ.
        let other = json!({"statusCode": 404, "error": "Not Found", "message": "Other"});
        let second = answer(
            "p",
            post,
            (200, tried("ERR_APPLY")),
            (200, tried("ERR_COOLDOWN")),
        ) + &answer("g", get, (404, other), (404, missing));
        std::fs::write(dir.join("api-node6").join(ANSWERS), second).unwrap();
        let args = ApiCompareArgs::Recorded { run: dir.clone() };
        let report = compare(&args).unwrap();
        assert_eq!(report.files, 2);
        assert_eq!(report.results.len(), 3, "{report}");
        assert_eq!(report.failed(), 1, "{report}");
        let failed = report.results.iter().find(|r| r.outcome.is_failure());
        assert!(failed.unwrap().file.contains("api-node6"), "{report}");
        assert!(report.source.contains("and 1 posts"), "{}", report.source);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_pair_of_a_run_is_judged_alone_as_the_whole_run_judges_it() {
        let dir = std::env::temp_dir().join(format!("heartwood-api-pair-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for sub in [RUN_ANSWERS_DIR, "api-node6"] {
            std::fs::create_dir_all(dir.join(sub)).unwrap();
        }
        let manifest = json!({"format": live_run::FORMAT, "run": "w", "network": "devnet",
            "seed": "s", "nodes": [], "api": {"left": "node4", "right": "node1"},
            "apiExtra": [{"left": "node6", "right": "node1", "dir": "api-node6"}]});
        std::fs::write(dir.join("run.json"), manifest.to_string()).unwrap();
        let get = json!({"method": "GET", "path": "/api"});
        let missing = json!({"statusCode": 404, "error": "Not Found", "message": "Not Found"});
        let post = json!({"method": "POST", "path": "/api/transactions", "body": "{\"t\":1}"});
        let tried = |kind: &str| json!({"errors": {"x": {"type": kind}}});
        let first = answer(
            "g",
            get.clone(),
            (404, missing.clone()),
            (404, missing.clone()),
        ) + &answer(
            "p",
            post.clone(),
            (200, tried("ERR_APPLY")),
            (200, tried("ERR_APPLY")),
        );
        std::fs::write(dir.join(RUN_ANSWERS_DIR).join(ANSWERS), first).unwrap();
        let other = json!({"statusCode": 404, "error": "Not Found", "message": "Other"});
        let second = answer(
            "p",
            post,
            (200, tried("ERR_APPLY")),
            (200, tried("ERR_COOLDOWN")),
        ) + &answer("g", get, (404, other), (404, missing));
        std::fs::write(dir.join("api-node6").join(ANSWERS), second).unwrap();
        let pair = |left: &str, right: &str| ApiCompareArgs::Pair {
            run: dir.clone(),
            left: left.into(),
            right: right.into(),
        };
        // The first pair: its own two answers, both equal; the second corpus is not read.
        let report = compare(&pair("node4", "node1")).unwrap();
        assert_eq!((report.files, report.results.len()), (1, 2), "{report}");
        assert_eq!(report.failed(), 0, "{report}");
        // The second pair: its differing GET, and its repeated post left out as in the whole run.
        let report = compare(&pair("node6", "node1")).unwrap();
        assert_eq!((report.files, report.results.len()), (1, 1), "{report}");
        assert_eq!(report.failed(), 1, "{report}");
        assert!(report.results[0].file.contains("api-node6"), "{report}");
        assert!(
            report.source.starts_with("node6 against node1"),
            "{}",
            report.source
        );
        assert!(report.source.contains("and 1 posts"), "{}", report.source);
        // A pair the run did not record is refused, naming the pairs it did.
        let refused = compare(&pair("node5", "node3")).unwrap_err().to_string();
        assert!(
            refused.contains("no route corpus of node5 against node3"),
            "{refused}"
        );
        assert!(refused.contains("node4 against node1 (api)"), "{refused}");
        assert!(
            refused.contains("node6 against node1 (api-node6)"),
            "{refused}"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
