//! The resource report of a forging run: each node's whole process tree sampled every 10 s (resident
//! and proportional memory, CPU time, written bytes) into the run's series, against the budgets (a
//! relay at 60 MB typical and 120 MB with a full pool, a forger at 15 MB, five nodes at 0.5 GB,
//! five nodes at 5 % of one core with empty blocks, five nodes at 100 MB a day of disk); a budget
//! missed is reported with its cause, never re-baselined. The budgets are the mixed forging runs'
//! and the churn run's; any other run's figures (the core run's) are recorded, not judged. CPU time
//! and disk writes are taken over the quiet intervals, which no fault window and no full pool
//! touch; each node's writes over the whole run are recorded beside them.
//!
//! The series are `resources/<node>-<role>.jsonl` (role `relay`, `forger`, or `core` for the
//! combined role's one process), one line per sample: `{t, pid, rss, pss, cpuMs, written}`. A
//! sampler pass stamps every series with one time. The counters are the process tree's and start
//! again at zero when its process does, so a change of `pid` between two samples starts a new
//! count. A megabyte is 1,000,000 bytes, which is the stricter reading of the budgets.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde_json::Value;

use super::run::{self, Implementation, Manifest, Step, Window};
use super::{Cases, ForgingRun};
use crate::errors::{Error, Result};
use crate::report::Report;

const SECTION: &str = "resources";

/// A megabyte, as the budgets count it.
pub const MB: u64 = 1_000_000;
/// A relay's resident memory, typically (the 95th percentile of its samples outside a full pool).
pub const RELAY_TYPICAL: u64 = 60 * MB;
/// A relay's resident memory with a full pool (its highest sample while the pool is full).
pub const RELAY_FULL_POOL: u64 = 120 * MB;
/// A forger's resident memory (the 95th percentile of its samples).
pub const FORGER: u64 = 15 * MB;
/// The one process of the combined role holds both roles' budgets.
pub const CORE: u64 = RELAY_TYPICAL + FORGER;
/// The five nodes' resident memory together, at its highest outside a full pool.
pub const FIVE_NODES: u64 = 500 * MB;
/// The five nodes' CPU time in quiet windows, in percent of one core.
pub const QUIET_CPU_PERCENT: u64 = 5;
/// A node's disk writes (its data and its logs) in a day, extrapolated from the run.
pub const DISK_PER_DAY: u64 = 100 * MB;
/// The shortest span of quiet samples a CPU figure rests on, in milliseconds.
const MIN_QUIET_MS: i64 = 60_000;
/// The shortest span a disk figure is extrapolated from, in milliseconds.
const MIN_SPAN_MS: i64 = 60_000;
const DAY_MS: u128 = 86_400_000;

/// A sampled process tree's role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Role {
    /// The relay.
    Relay,
    /// The forger.
    Forger,
    /// The combined role: a relay and a forger in one process.
    Core,
}

impl Role {
    fn parse(text: &str) -> Option<Role> {
        match text {
            "relay" => Some(Role::Relay),
            "forger" => Some(Role::Forger),
            "core" => Some(Role::Core),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Role::Relay => "relay",
            Role::Forger => "forger",
            Role::Core => "core",
        }
    }
}

/// One sample of a process tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sample {
    /// When, in milliseconds since the Unix epoch.
    pub at: i64,
    /// The tree's root process.
    pub pid: u64,
    /// Its resident memory in bytes.
    pub rss: u64,
    /// Its proportional memory in bytes, where `/proc` gave it.
    pub pss: Option<u64>,
    /// Its CPU time in milliseconds since the process started.
    pub cpu_ms: u64,
    /// The bytes it wrote since the process started, where `/proc` gave them.
    pub written: Option<u64>,
}

/// A node's role's samples, in time order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Series {
    /// `nodeK`.
    pub node: String,
    /// The role.
    pub role: Role,
    /// The samples.
    pub samples: Vec<Sample>,
}

impl Series {
    fn label(&self) -> String {
        format!("{} {}", self.role.name(), self.node)
    }
}

/// The series of the run in `dir` (`resources/*.jsonl`), ordered by node and role; a torn last
/// line (a sampler cut off in a write) is dropped, any other line that is no sample is an error.
pub fn read_series(dir: &Path) -> Result<Vec<Series>> {
    let folder = dir.join("resources");
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(&folder) else {
        return Ok(out);
    };
    let mut paths: Vec<_> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
    paths.sort();
    for path in paths {
        let Some(stem) = path
            .file_name()
            .and_then(|s| s.to_str())
            .and_then(|s| s.strip_suffix(".jsonl"))
        else {
            continue;
        };
        let Some((node, role)) = stem
            .rsplit_once('-')
            .and_then(|(n, r)| Some((n, Role::parse(r)?)))
        else {
            continue;
        };
        let bytes = std::fs::read(&path).map_err(|e| Error::io(&path, e))?;
        let mut samples = Vec::new();
        for (index, part) in bytes.split_inclusive(|b| *b == b'\n').enumerate() {
            if !part.ends_with(b"\n") || part.iter().all(u8::is_ascii_whitespace) {
                continue;
            }
            let sample = serde_json::from_slice::<Value>(part)
                .ok()
                .and_then(|v| sample_of(&v))
                .ok_or_else(|| Error::Corpus {
                    path: path.clone(),
                    message: format!("line {} is not a resource sample", index + 1),
                })?;
            samples.push(sample);
        }
        samples.sort_by_key(|s| s.at);
        out.push(Series {
            node: node.to_owned(),
            role,
            samples,
        });
    }
    out.sort_by(|a, b| (&a.node, a.role).cmp(&(&b.node, b.role)));
    Ok(out)
}

fn sample_of(v: &Value) -> Option<Sample> {
    Some(Sample {
        at: run::parse_time(v.get("t")?.as_str()?)?,
        pid: v.get("pid")?.as_u64()?,
        rss: v.get("rss")?.as_u64()?,
        pss: v.get("pss").and_then(Value::as_u64),
        cpu_ms: v.get("cpuMs")?.as_u64()?,
        written: v.get("written").and_then(Value::as_u64),
    })
}

/// The nearest-rank percentile of `sorted`.
fn percentile(sorted: &[u64], p: usize) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let rank = (p * sorted.len()).div_ceil(100).clamp(1, sorted.len());
    sorted[rank - 1]
}

fn mb(bytes: u64) -> String {
    format!("{}.{} MB", bytes / MB, bytes % MB / 100_000)
}

/// One budget's verdict for one subject, or a figure recorded beside them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// What was measured (the subject and the budget).
    pub label: String,
    /// The figures: what was measured, where, and what the budget is.
    pub detail: String,
    /// The verdict.
    pub verdict: Verdict,
}

/// A finding's verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// The budget held.
    Held,
    /// The figure is over its budget.
    Over,
    /// The series hold too little to measure the figure.
    NoEvidence,
    /// A figure recorded beside the budgets, never a failure.
    Recorded,
}

impl Finding {
    /// Whether the finding is no failure.
    pub fn ok(&self) -> bool {
        matches!(self.verdict, Verdict::Held | Verdict::Recorded)
    }

    /// Whether the finding is judged against a budget (a recorded figure is not).
    pub fn gated(&self) -> bool {
        self.verdict != Verdict::Recorded
    }
}

/// What the assessment reads besides the series.
pub struct Inputs<'a> {
    /// The series of every sampled process tree of the run.
    pub series: &'a [Series],
    /// The nodes the budgets apply to (the Heartwood nodes).
    pub counted: &'a BTreeSet<String>,
    /// The nodes that make up "five nodes" (the validators' nodes).
    pub five: &'a BTreeSet<String>,
    /// The span the pool was full in (its step's start to the end of its drain), when the run has
    /// one.
    pub pool: Option<(i64, i64)>,
    /// The fault windows: no quiet window overlaps one.
    pub windows: &'a [Window],
    /// The bytes of each node's logs, for the disk rate.
    pub logs: &'a BTreeMap<String, u64>,
    /// What the run was doing at a time, for the cause of a miss.
    pub what: &'a dyn Fn(i64) -> String,
}

fn finding(label: String, ok: bool, detail: String) -> Finding {
    let verdict = if ok { Verdict::Held } else { Verdict::Over };
    Finding {
        label,
        detail,
        verdict,
    }
}

fn missing(label: String, detail: String) -> Finding {
    Finding {
        label,
        detail,
        verdict: Verdict::NoEvidence,
    }
}

fn recorded(label: String, detail: String) -> Finding {
    Finding {
        label,
        detail,
        verdict: Verdict::Recorded,
    }
}

/// Turn every finding into a recorded figure: for a run the budgets are not set for (they are
/// measured on the mixed forging runs and the churn run), whose figures are still reported.
pub fn record_only(found: &mut [Finding]) {
    for f in found {
        match f.verdict {
            Verdict::Over => f.detail.push_str(" (over the budget)"),
            Verdict::NoEvidence => f.detail.push_str(" (not measured)"),
            Verdict::Held | Verdict::Recorded => {}
        }
        f.verdict = Verdict::Recorded;
    }
}

/// Whether the budgets are this run's to meet: the mixed forging runs and the churn run.
fn budgets_apply(run: &str) -> bool {
    matches!(run, "mixed-short" | "mixed" | "mixed-split" | "churn")
}

/// Every budget against the series.
pub fn assess(i: &Inputs<'_>) -> Vec<Finding> {
    let mut out = Vec::new();
    let mine: Vec<&Series> = i
        .series
        .iter()
        .filter(|s| i.counted.contains(&s.node))
        .collect();
    for s in &mine {
        out.extend(memory(i, s));
    }
    out.extend(total(i, &mine));
    out.extend(cpu(i, &mine));
    out.extend(disk(i, &mine));
    out
}

fn in_pool(i: &Inputs<'_>, at: i64) -> bool {
    i.pool.is_some_and(|(from, to)| (from..=to).contains(&at))
}

fn memory(i: &Inputs<'_>, s: &Series) -> Vec<Finding> {
    let mut out = Vec::new();
    let label = |what: &str| format!("{} resident memory {what}", s.label());
    let outside: Vec<&Sample> = s.samples.iter().filter(|x| !in_pool(i, x.at)).collect();
    let mut rss: Vec<u64> = outside.iter().map(|x| x.rss).collect();
    rss.sort_unstable();
    let limit = match s.role {
        Role::Relay => RELAY_TYPICAL,
        Role::Forger => FORGER,
        Role::Core => CORE,
    };
    let what = if s.role == Role::Forger {
        "(95th percentile)"
    } else {
        "outside a full pool (95th percentile)"
    };
    match (outside.iter().max_by_key(|x| x.rss), rss.is_empty()) {
        (Some(peak), false) => {
            let p95 = percentile(&rss, 95);
            out.push(finding(
                label(what),
                p95 <= limit,
                format!(
                    "{} of {} samples, peak {} at {} ({}), budget {}",
                    mb(p95),
                    rss.len(),
                    mb(peak.rss),
                    run::format_time(peak.at),
                    (i.what)(peak.at),
                    mb(limit)
                ),
            ));
        }
        _ => out.push(missing(
            label(what),
            "no sample outside the full-pool window".into(),
        )),
    }
    if s.role == Role::Relay && i.pool.is_some() {
        let full = s.samples.iter().filter(|x| in_pool(i, x.at));
        let l = label("with a full pool (highest)");
        match full.max_by_key(|x| x.rss) {
            Some(peak) => out.push(finding(
                l,
                peak.rss <= RELAY_FULL_POOL,
                format!(
                    "{} at {} ({}), budget {}",
                    mb(peak.rss),
                    run::format_time(peak.at),
                    (i.what)(peak.at),
                    mb(RELAY_FULL_POOL)
                ),
            )),
            None => out.push(missing(l, "no sample while the pool was full".into())),
        }
    }
    if let Some(pss) = s.samples.iter().filter_map(|x| x.pss).max() {
        // The proportional figure is recorded for the report; the budgets are the resident size's.
        out.push(recorded(
            format!("{} proportional memory (highest)", s.label()),
            mb(pss),
        ));
    }
    out
}

/// The five nodes' resident memory summed over each sampler pass, at its highest outside a full
/// pool.
fn total(i: &Inputs<'_>, mine: &[&Series]) -> Vec<Finding> {
    let mut passes: BTreeMap<i64, u64> = BTreeMap::new();
    for s in mine.iter().filter(|s| i.five.contains(&s.node)) {
        for x in s.samples.iter().filter(|x| !in_pool(i, x.at)) {
            *passes.entry(x.at).or_default() += x.rss;
        }
    }
    let label = format!(
        "{} resident memory together (highest pass outside a full pool)",
        the_nodes(i)
    );
    match passes.iter().max_by_key(|(_, sum)| **sum) {
        Some((at, sum)) => vec![finding(
            label,
            *sum <= FIVE_NODES,
            format!(
                "{} at {} ({}), budget {}",
                mb(*sum),
                run::format_time(*at),
                (i.what)(*at),
                mb(FIVE_NODES)
            ),
        )],
        None => vec![missing(label, "no sample".into())],
    }
}

/// The nodes the five-node budgets sum, named by their count for a finding's label: "the five
/// nodes'" in a run of five validators' nodes, "the three nodes'" in a run of three.
fn the_nodes(i: &Inputs<'_>) -> String {
    const WORDS: [&str; 10] = [
        "no", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine",
    ];
    let n = i.five.len();
    let count = WORDS
        .get(n)
        .map_or_else(|| n.to_string(), |w| (*w).to_owned());
    if n == 1 {
        format!("the {count} node's")
    } else {
        format!("the {count} nodes'")
    }
}

/// Whether the interval from `from` to `to` is quiet: no fault window and no full pool touches it.
fn quiet(i: &Inputs<'_>, from: i64, to: i64) -> bool {
    !i.windows.iter().any(|w| w.start <= to && from <= w.end)
        && !i.pool.is_some_and(|(a, b)| a <= to && from <= b)
}

/// The CPU time the five nodes' trees used in the intervals between samples that no fault window
/// and no full pool touched, against the time those intervals span.
fn cpu(i: &Inputs<'_>, mine: &[&Series]) -> Vec<Finding> {
    let (mut cpu_ms, mut quiet_ms) = (0_u64, 0_i64);
    for s in mine.iter().filter(|s| i.five.contains(&s.node)) {
        let mut span = 0_i64;
        for pair in s.samples.windows(2) {
            let (a, b) = (&pair[0], &pair[1]);
            if a.pid == b.pid && b.cpu_ms >= a.cpu_ms && quiet(i, a.at, b.at) {
                cpu_ms += b.cpu_ms - a.cpu_ms;
                span += b.at - a.at;
            }
        }
        quiet_ms = quiet_ms.max(span);
    }
    let label = format!("{} CPU time in quiet windows (of one core)", the_nodes(i));
    if quiet_ms < MIN_QUIET_MS {
        return vec![missing(
            label,
            format!(
                "only {} s of quiet samples, at least 60 s are needed",
                quiet_ms / 1_000
            ),
        )];
    }
    // Tenths of a percent: cpu_ms / quiet_ms * 100.
    let tenths = u128::from(cpu_ms) * 1_000 / u128::try_from(quiet_ms).unwrap_or(1);
    vec![finding(
        label,
        tenths <= u128::from(QUIET_CPU_PERCENT) * 10,
        format!(
            "{}.{} % over {} s of quiet windows, budget {QUIET_CPU_PERCENT} %",
            tenths / 10,
            tenths % 10,
            quiet_ms / 1_000
        ),
    )]
}

/// The bytes a series wrote: each sample's counter less the one before it, a process that started
/// after the last sample counting from zero.
fn written(s: &Series) -> u64 {
    let mut sum = 0_u64;
    let mut before: Option<(u64, u64)> = None;
    for x in &s.samples {
        let Some(now) = x.written else {
            continue;
        };
        sum += match before {
            Some((pid, was)) if pid == x.pid => now.saturating_sub(was),
            _ => now,
        };
        before = Some((x.pid, now));
    }
    sum
}

/// The bytes a series wrote in its quiet intervals (between two samples of one process, no fault
/// window and no full pool touching them) and the milliseconds those intervals span.
fn quiet_written(i: &Inputs<'_>, s: &Series) -> (u64, i64) {
    let (mut bytes, mut span) = (0_u64, 0_i64);
    for pair in s.samples.windows(2) {
        let (a, b) = (&pair[0], &pair[1]);
        let (Some(was), Some(now)) = (a.written, b.written) else {
            continue;
        };
        if a.pid == b.pid && now >= was && quiet(i, a.at, b.at) {
            bytes += now - was;
            span += b.at - a.at;
        }
    }
    (bytes, span)
}

/// Bytes over `span_ms` milliseconds, extrapolated to a day.
fn per_day(bytes: u64, span_ms: i64) -> u64 {
    let day = u128::from(bytes) * DAY_MS / u128::try_from(span_ms.max(1)).unwrap_or(1);
    u64::try_from(day).unwrap_or(u64::MAX)
}

/// The five nodes' disk writes together (their data and their logs, which are one copy: the
/// sampled trees write each line once), extrapolated to a day from the quiet intervals, where the
/// chain runs on its own; and each node's writes over the whole run, the restarts and the full pool
/// included, recorded beside it.
fn disk(i: &Inputs<'_>, mine: &[&Series]) -> Vec<Finding> {
    let label = format!(
        "{} disk writes in a day (extrapolated from the quiet windows)",
        the_nodes(i)
    );
    let mut out = Vec::new();
    let (mut day, mut short, mut top) = (0_u64, Vec::new(), None::<(u64, String)>);
    let (mut relays, mut forgers) = (0_u64, 0_u64);
    for s in mine.iter().filter(|s| i.five.contains(&s.node)) {
        let (bytes, span) = quiet_written(i, s);
        if span < MIN_SPAN_MS {
            short.push(format!("{} ({} s)", s.label(), span / 1_000));
            continue;
        }
        let rate = per_day(bytes, span);
        day = day.saturating_add(rate);
        match s.role {
            Role::Forger => forgers = forgers.saturating_add(rate),
            Role::Relay | Role::Core => relays = relays.saturating_add(rate),
        }
        if top.as_ref().is_none_or(|(r, _)| rate > *r) {
            top = Some((rate, s.label()));
        }
    }
    if !short.is_empty() || top.is_none() {
        out.push(missing(
            label,
            format!(
                "too few quiet samples, at least 60 s a series are needed: {}",
                if short.is_empty() {
                    "no series".to_owned()
                } else {
                    short.join(", ")
                }
            ),
        ));
    } else if let Some((rate, most)) = top {
        let logs: u64 = i
            .logs
            .iter()
            .filter(|(n, _)| i.five.contains(n.as_str()))
            .map(|(_, b)| *b)
            .sum();
        out.push(finding(
            label,
            day <= DISK_PER_DAY,
            format!(
                "{} a day (the relays' processes {}, the forgers' {}; the most {} at {}), budget {}; the log files hold {} in all",
                mb(day),
                mb(relays),
                mb(forgers),
                most,
                mb(rate),
                mb(DISK_PER_DAY),
                mb(logs)
            ),
        ));
    }
    let mut nodes: BTreeMap<&str, (u64, i64, i64)> = BTreeMap::new();
    for s in mine {
        let (Some(first), Some(last)) = (s.samples.first(), s.samples.last()) else {
            continue;
        };
        let e = nodes
            .entry(s.node.as_str())
            .or_insert((0, first.at, last.at));
        e.0 += written(s);
        e.1 = e.1.min(first.at);
        e.2 = e.2.max(last.at);
    }
    for (node, (bytes, from, to)) in nodes {
        let logs = i.logs.get(node).copied().unwrap_or(0);
        out.push(recorded(
            format!("{node} disk writes over the whole run"),
            format!(
                "{} written and {} of logs over {} s ({} a day), counting every restart and full pool of the run",
                mb(bytes),
                mb(logs),
                (to - from) / 1_000,
                mb(per_day(bytes, to - from))
            ),
        ));
    }
    out
}

/// What `steps` say ran at `at`: the last step that began by then and had not ended, or else the
/// last that began by then.
fn what_ran(steps: &[Step]) -> impl Fn(i64) -> String + '_ {
    move |at| {
        let began = steps.iter().filter(|s| s.start.is_some_and(|t| t <= at));
        let open = began
            .clone()
            .filter(|s| s.time("end").is_none_or(|e| e >= at))
            .max_by_key(|s| s.start);
        match open.or_else(|| began.max_by_key(|s| s.start)) {
            Some(s) => format!("step {}", s.name),
            None => "before the first step".to_owned(),
        }
    }
}

/// The span the pool was full: the start of the `full-pool` step to the end of its drain.
fn pool_span(steps: &[Step]) -> Option<(i64, i64)> {
    let s = steps.iter().find(|s| s.name == "full-pool")?;
    let from = s.start?;
    let to = s.time("drainedAt").or_else(|| s.time("end"))?;
    Some((from, to.max(from)))
}

/// The bytes of the files under each Heartwood node's `nodes/<node>/log`.
fn log_bytes(dir: &Path, manifest: &Manifest) -> BTreeMap<String, u64> {
    let size = |path: &Path| -> u64 {
        std::fs::read_dir(path)
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok()?.metadata().ok())
            .filter(|m| m.is_file())
            .map(|m| m.len())
            .sum()
    };
    manifest
        .nodes
        .iter()
        .filter(|n| n.implementation == Implementation::Heartwood)
        .map(|n| {
            (
                n.name.clone(),
                size(&dir.join("nodes").join(&n.name).join("log")),
            )
        })
        .collect()
}

/// The findings of the run in `dir`: the budgets against its series.
fn findings(dir: &Path, manifest: &Manifest, windows: &[Window]) -> Result<Vec<Finding>> {
    let series = read_series(dir)?;
    let steps = run::steps(dir)?;
    let counted: BTreeSet<String> = manifest
        .nodes
        .iter()
        .filter(|n| n.implementation == Implementation::Heartwood)
        .map(|n| n.name.clone())
        .collect();
    let five: BTreeSet<String> = manifest
        .nodes
        .iter()
        .filter(|n| n.implementation == Implementation::Heartwood && n.forger)
        .map(|n| n.name.clone())
        .collect();
    let what = what_ran(&steps);
    let mut out = Vec::new();
    // A series for every role a node runs.
    for n in manifest.nodes.iter().filter(|n| counted.contains(&n.name)) {
        let has = |role: Role| {
            series
                .iter()
                .any(|s| s.node == n.name && s.role == role && s.samples.len() >= 2)
        };
        let combined = manifest.run == "core" || has(Role::Core);
        let roles: &[Role] = if combined {
            &[Role::Core]
        } else if n.forger {
            &[Role::Relay, Role::Forger]
        } else {
            &[Role::Relay]
        };
        for role in roles {
            let label = format!("a resource series for {} {}", role.name(), n.name);
            out.push(if has(*role) {
                finding(label, true, "at least two samples".into())
            } else {
                missing(label, "fewer than two samples".into())
            });
        }
    }
    let mut measured = assess(&Inputs {
        series: &series,
        counted: &counted,
        five: &five,
        pool: pool_span(&steps),
        windows,
        logs: &log_bytes(dir, manifest),
        what: &what,
    });
    if !budgets_apply(&manifest.run) {
        record_only(&mut measured);
    }
    out.extend(measured);
    Ok(out)
}

fn push(cases: &mut Cases, found: Vec<Finding>) {
    for f in found {
        match f.verdict {
            Verdict::Recorded => cases.record(SECTION, format!("{}: {}", f.label, f.detail)),
            Verdict::Held => cases.pass(SECTION, format!("{}: {}", f.label, f.detail)),
            Verdict::NoEvidence => cases.fail(SECTION, f.label, f.detail),
            Verdict::Over => cases.fail(
                SECTION,
                format!("{}: over its budget", f.label),
                format!(
                    "{}; reported with the series as its evidence, not re-baselined: its cause and the work that removes it are to be named",
                    f.detail
                ),
            ),
        }
    }
}

/// The resource rules within a run's check: every budget against the run's series.
pub(super) fn check(run: &ForgingRun<'_>, cases: &mut Cases) -> Result<()> {
    match findings(run.run_dir, run.manifest, run.windows) {
        Ok(found) => push(cases, found),
        Err(e) => cases.fail(SECTION, "the resource series", e.to_string()),
    }
    Ok(())
}

/// `difftest live resources RUN_DIR`: the run's resource report.
pub fn report(run_dir: &Path) -> Result<Report> {
    let manifest = Manifest::load(run_dir)?;
    let windows = run::windows(run_dir, manifest.window_seconds)?;
    let mut cases = Cases {
        run: manifest.run.clone(),
        results: Vec::new(),
    };
    push(&mut cases, findings(run_dir, &manifest, &windows)?);
    Ok(Report {
        title: format!(
            "Resource report of run {} ({})",
            manifest.run, manifest.network
        ),
        source: run_dir.display().to_string(),
        files: read_series(run_dir)?.len(),
        results: cases.results,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const T0: i64 = 1_790_000_000_000;

    fn sample(n: i64, pid: u64, rss: u64, cpu_ms: u64, written: u64) -> Sample {
        Sample {
            at: T0 + n * 10_000,
            pid,
            rss,
            pss: None,
            cpu_ms,
            written: Some(written),
        }
    }

    fn series(node: &str, role: Role, samples: Vec<Sample>) -> Series {
        Series {
            node: node.into(),
            role,
            samples,
        }
    }

    fn names(list: &[&str]) -> BTreeSet<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    fn run_findings(all: &[Series], pool: Option<(i64, i64)>, windows: &[Window]) -> Vec<Finding> {
        let counted = names(&["node1", "node2"]);
        let logs = BTreeMap::new();
        let what = |_: i64| "step x".to_owned();
        assess(&Inputs {
            series: all,
            counted: &counted,
            five: &counted,
            pool,
            windows,
            logs: &logs,
            what: &what,
        })
    }

    fn find<'a>(found: &'a [Finding], part: &str) -> &'a Finding {
        found
            .iter()
            .find(|f| f.label.contains(part))
            .unwrap_or_else(|| panic!("no finding with {part}"))
    }

    #[test]
    fn a_relay_over_its_typical_size_is_reported_with_where_it_peaked() {
        let samples = (0..40)
            .map(|n| sample(n, 7, 70 * MB, 100 * n as u64, 0))
            .collect();
        let found = run_findings(&[series("node1", Role::Relay, samples)], None, &[]);
        let f = find(&found, "relay node1 resident memory outside");
        assert!(!f.ok());
        assert!(f.detail.contains("70.0 MB"), "{}", f.detail);
        assert!(f.detail.contains("step x"), "{}", f.detail);
    }

    #[test]
    fn a_full_pool_has_its_own_budget_and_the_typical_one_leaves_it_out() {
        let samples = (0..40)
            .map(|n| {
                let rss = if (10..20).contains(&n) { 110 } else { 40 };
                sample(n, 7, rss * MB, 0, 0)
            })
            .collect();
        let pool = Some((T0 + 10 * 10_000, T0 + 19 * 10_000));
        let all = [series("node1", Role::Relay, samples)];
        let found = run_findings(&all, pool, &[]);
        assert!(find(&found, "outside a full pool").ok());
        assert!(find(&found, "with a full pool").ok());
        // 130 MB while the pool is full misses the full-pool budget only.
        let mut over = all.clone();
        over[0].samples[12].rss = 130 * MB;
        let found = run_findings(&over, pool, &[]);
        assert!(find(&found, "outside a full pool").ok());
        assert!(!find(&found, "with a full pool").ok());
    }

    #[test]
    fn a_pool_the_run_never_filled_has_no_full_pool_line() {
        let samples = (0..5).map(|n| sample(n, 7, 40 * MB, 0, 0)).collect();
        let found = run_findings(&[series("node1", Role::Relay, samples)], None, &[]);
        assert!(found.iter().all(|f| !f.label.contains("with a full pool")));
    }

    #[test]
    fn a_forger_has_fifteen_megabytes() {
        let ok = (0..20).map(|n| sample(n, 9, 14 * MB, 0, 0)).collect();
        let over = (0..20).map(|n| sample(n, 9, 16 * MB, 0, 0)).collect();
        assert!(
            find(
                &run_findings(&[series("node1", Role::Forger, ok)], None, &[]),
                "forger node1"
            )
            .ok()
        );
        assert!(
            !find(
                &run_findings(&[series("node1", Role::Forger, over)], None, &[]),
                "forger node1"
            )
            .ok()
        );
    }

    #[test]
    fn the_five_nodes_are_summed_per_sampler_pass() {
        let two = |rss: u64| {
            [
                series(
                    "node1",
                    Role::Relay,
                    (0..5).map(|n| sample(n, 1, rss, 0, 0)).collect(),
                ),
                series(
                    "node2",
                    Role::Relay,
                    (0..5).map(|n| sample(n, 2, rss, 0, 0)).collect(),
                ),
            ]
        };
        let ok = run_findings(&two(240 * MB), None, &[]);
        assert!(find(&ok, "the two nodes' resident memory").ok());
        let over = run_findings(&two(260 * MB), None, &[]);
        let f = find(&over, "the two nodes' resident memory");
        assert!(!f.ok());
        assert!(f.detail.contains("520.0 MB"), "{}", f.detail);
    }

    #[test]
    fn cpu_counts_only_quiet_intervals_and_a_new_process_starts_a_new_count() {
        // 13 s of CPU over 120 s is 10.8 %; the restart in the middle must not count its reset as time.
        let mut samples: Vec<Sample> = (0..12)
            .map(|n| sample(n, 1, MB, 1_000 * n as u64, 0))
            .collect();
        samples.extend((12..24).map(|n| sample(n, 2, MB, 1_000 * (n as u64 - 12), 0)));
        let all = [series("node1", Role::Relay, samples)];
        let found = run_findings(&all, None, &[]);
        let f = find(&found, "CPU time");
        assert!(!f.ok());
        assert!(f.detail.contains("10.0 %"), "{}", f.detail);
        // The same CPU time inside a fault window is not quiet time: nothing is left to judge.
        let window = Window {
            fault: "restart".into(),
            nodes: vec!["node1".into()],
            start: T0 - 1,
            end: T0 + 24 * 10_000,
        };
        let found = run_findings(&all, None, &[window]);
        assert!(find(&found, "CPU time").detail.contains("quiet samples"));
        // A quiet node at 2 % passes.
        let quiet = (0..24)
            .map(|n| sample(n, 1, MB, 200 * n as u64, 0))
            .collect();
        let found = run_findings(&[series("node1", Role::Relay, quiet)], None, &[]);
        assert!(find(&found, "CPU time").ok());
    }

    #[test]
    fn the_disk_budget_is_the_five_nodes_together() {
        // Each node writes 60 MB a day (6,944 bytes every 10 s): within 100 MB alone, 120 MB
        // together.
        let steady = |pid: u64| {
            (0..100)
                .map(|n| sample(n, pid, MB, 0, 6_944 * n as u64))
                .collect()
        };
        let all = [
            series("node1", Role::Relay, steady(1)),
            series("node2", Role::Relay, steady(2)),
        ];
        let found = run_findings(&all, None, &[]);
        let f = find(&found, "the two nodes' disk writes in a day");
        assert!(!f.ok(), "{f:?}");
        assert!(f.detail.contains("119.9 MB a day"), "{}", f.detail);
    }

    #[test]
    fn the_full_pools_writes_are_not_a_days_writes() {
        // 1 kB every 10 s, and 50 MB in the ten samples of the full pool.
        let samples: Vec<Sample> = (0..100)
            .map(|n| {
                let burst = if n >= 20 {
                    50 * MB
                } else {
                    (n.max(10) - 10) as u64 * 5 * MB
                };
                sample(n, 1, MB, 0, 1_000 * n as u64 + burst)
            })
            .collect();
        let pool = Some((T0 + 10 * 10_000, T0 + 20 * 10_000));
        let all = [series("node1", Role::Relay, samples)];
        let found = run_findings(&all, pool, &[]);
        let f = find(&found, "the two nodes' disk writes in a day");
        assert!(f.ok(), "{f:?}");
        assert!(f.detail.contains("8.6 MB a day"), "{}", f.detail);
        // The whole run's figure, full pool included, is recorded, not judged.
        let whole = find(&found, "node1 disk writes over the whole run");
        assert!(whole.ok() && !whole.gated(), "{whole:?}");
    }

    #[test]
    fn a_run_whose_budgets_are_not_its_own_records_them() {
        let samples = (0..40)
            .map(|n| sample(n, 7, 500 * MB, 100 * n as u64, MB * n as u64))
            .collect();
        let mut found = run_findings(&[series("node1", Role::Relay, samples)], None, &[]);
        assert!(found.iter().any(|f| !f.ok()));
        record_only(&mut found);
        assert!(found.iter().all(|f| !f.gated()));
    }

    #[test]
    fn disk_is_extrapolated_from_what_was_written_and_a_restart_counts_from_zero() {
        // 100 kB every 10 s is 864 MB a day; the interval of the restart is left out of the rate.
        let mut samples: Vec<Sample> = (0..6)
            .map(|n| sample(n, 1, MB, 0, 100_000 * n as u64))
            .collect();
        samples.extend((6..11).map(|n| sample(n, 2, MB, 0, 100_000 * (n as u64 - 5))));
        let s = series("node1", Role::Relay, samples);
        assert_eq!(written(&s), 500_000 + 500_000 + 100_000 - 100_000);
        let found = run_findings(&[s], None, &[]);
        let f = find(&found, "the two nodes' disk writes");
        assert!(!f.ok());
        assert!(f.detail.contains("864.0 MB a day"), "{}", f.detail);
        // A quiet node: 100 kB in a long enough run is under 100 MB a day.
        let quiet = (0..200)
            .map(|n| sample(n, 1, MB, 0, 50 * n as u64))
            .collect();
        let found = run_findings(&[series("node1", Role::Relay, quiet)], None, &[]);
        let f = find(&found, "the two nodes' disk writes");
        assert!(f.ok(), "{f:?}");
    }

    #[test]
    fn a_node_outside_the_count_has_no_budget() {
        let samples = (0..20).map(|n| sample(n, 1, 900 * MB, 0, 0)).collect();
        let found = run_findings(&[series("node7", Role::Relay, samples)], None, &[]);
        assert!(found.iter().all(|f| !f.label.contains("node7")));
    }

    #[test]
    fn the_percentile_is_nearest_rank() {
        let v: Vec<u64> = (1..=100).collect();
        assert_eq!(percentile(&v, 95), 95);
        assert_eq!(percentile(&v, 100), 100);
        assert_eq!(percentile(&[4], 95), 4);
        assert_eq!(percentile(&[], 95), 0);
    }

    #[test]
    fn a_torn_last_line_is_dropped_and_any_other_bad_line_is_named() {
        let dir = std::env::temp_dir().join(format!("hw-resources-{}", std::process::id()));
        let resources = dir.join("resources");
        std::fs::create_dir_all(&resources).unwrap();
        let line = r#"{"t":"2026-09-30T17:39:35.588Z","pid":7,"rss":1,"pss":null,"cpuMs":3,"written":null}"#;
        std::fs::write(
            resources.join("node1-relay.jsonl"),
            format!("{line}\n{line}\n{{\"t\":\"2026-09-30T17:4"),
        )
        .unwrap();
        std::fs::write(resources.join("notes.jsonl"), "x").unwrap();
        let read = read_series(&dir).unwrap();
        assert_eq!(read.len(), 1);
        assert_eq!(read[0].samples.len(), 2);
        assert_eq!(read[0].samples[0].pss, None);
        std::fs::write(resources.join("node1-relay.jsonl"), "{}\n").unwrap();
        assert!(read_series(&dir).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
