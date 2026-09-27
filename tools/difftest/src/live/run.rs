//! The run directory of a live run, as the devnet tooling writes it.
//!
//! - `run.json`: the run's name, network, seed, round length and stop height, the nodes (address,
//!   implementation, role, whether it is under test), the chains to judge (`corpora`: a corpus name
//!   and the directory of the archives it is imported from), the pair run (`pair`, for `h`: `r`), the
//!   minority of a partition (`minority`) and the route corpus's nodes (`api`);
//! - `steps.jsonl`: one line per scripted step, `{step, node, start, end, ok, ...}`;
//! - `faults.jsonl`: one line per scripted fault, `{fault, nodes, start, recovered}`; its window runs
//!   from `start` to `faultWindowSeconds` (30) after `recovered`, when the node answered its API
//!   again or the partition healed;
//! - `final.json`: every node's last block at the pause;
//! - `propagation.jsonl`: every change of a node's last block, `{t, node, h, id}`;
//! - `bans.jsonl`: the reference nodes' "Banning" lines, `{node, ts, ip, duration, reason, line}`;
//! - `archives/<dir>/<stamp>/`: the devnet tooling's archives (node 1's, and more);
//! - `nodes/<node>/`: a Heartwood node's state at the end: `data/` (its store), `difftest/`
//!   (`digest-NNNN.jsonl` and `blocks-NNNN.bin`, one pair per start of the relay), `log/`;
//! - `logs/<node>/`: a reference node's logs; `rejections/<node>.jsonl`: each node's refused and
//!   undone blocks;
//! - `txgen/<name>/`: the scenario generator's records; `wallets.json`: the scenario wallets at the
//!   pause; `check-chain.json`: the tooling's check of node 1's chain (schedule and supply);
//! - `api/`: the route corpus's answers (its `answers.jsonl` files); `rate-limit.jsonl`: the
//!   rate limit's answers, sent from an address that no node whitelists.
//!
//! Times are UTC, `YYYY-MM-DDTHH:MM:SS[.fff]Z`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::errors::{Error, Result};

/// The format string of `run.json`.
pub const FORMAT: &str = "heartwood-live-run/1";

/// Which implementation a node runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Implementation {
    /// The reference implementation.
    Reference,
    /// A Heartwood relay.
    Heartwood,
}

/// A node of the run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    /// `nodeK`.
    pub name: String,
    /// Its address on the run's network.
    pub ip: String,
    /// Its implementation.
    pub implementation: Implementation,
    /// Whether it forges (a reference node with delegates) or only relays.
    pub forger: bool,
    /// Whether the run tests it (the observers).
    pub under_test: bool,
}

/// A run's `run.json`.
#[derive(Debug, Clone, PartialEq)]
pub struct Manifest {
    /// `r`, `h`, `m1` or `m2`.
    pub run: String,
    /// The devnet's name.
    pub network: String,
    /// Whether the nodes under test were reference relays (a dry run of the recipe).
    pub dry_run: bool,
    /// The chain's key-derivation seed.
    pub seed: String,
    /// Blocks per round.
    pub round_length: u32,
    /// Seconds per slot.
    pub block_time: u32,
    /// The height the run stops at (at most the end of round 5).
    pub stop_height: u32,
    /// The node whose chain is the run's corpus.
    pub control: String,
    /// The nodes.
    pub nodes: Vec<Node>,
    /// The chains to judge: corpus name and the directory of its archives, relative to the run.
    pub corpora: BTreeMap<String, String>,
    /// The run a pair comparison reads, when there is one (`h` reads `r`).
    pub pair: Option<String>,
    /// The minority of a partition and its corpus, when there is one.
    pub minority: Option<(Vec<String>, String)>,
    /// The nodes of the route corpus (under test, control), when the run records one.
    pub api: Option<(String, String)>,
    /// Seconds a fault window lasts after its recovery.
    pub window_seconds: i64,
    /// `running`, `complete` or `failed`.
    pub status: String,
}

impl Manifest {
    /// The node called `name`.
    pub fn node(&self, name: &str) -> Option<&Node> {
        self.nodes.iter().find(|n| n.name == name)
    }

    /// The nodes under test.
    pub fn under_test(&self) -> impl Iterator<Item = &Node> {
        self.nodes.iter().filter(|n| n.under_test)
    }
}

fn read_text(path: &Path) -> Result<String> {
    std::fs::read_to_string(path).map_err(|e| Error::io(path, e))
}

fn bad(path: &Path, message: impl Into<String>) -> Error {
    Error::Corpus {
        path: path.to_path_buf(),
        message: message.into(),
    }
}

/// The JSON lines of `path`; a missing file is no lines.
pub fn read_lines(path: &Path) -> Result<Vec<Value>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for (index, line) in read_text(path)?.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        out.push(serde_json::from_str(line).map_err(|e| Error::Parse {
            path: path.to_path_buf(),
            line: index + 1,
            message: e.to_string(),
        })?);
    }
    Ok(out)
}

/// The JSON document of `path`, or its last line when it holds several.
pub fn read_json(path: &Path) -> Result<Value> {
    let text = read_text(path)?;
    serde_json::from_str(&text)
        .or_else(|_| {
            serde_json::from_str(text.lines().rfind(|l| !l.trim().is_empty()).unwrap_or(""))
        })
        .map_err(|e| bad(path, format!("not JSON: {e}")))
}

fn text(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_owned)
}

fn number(value: &Value, key: &str) -> Option<u32> {
    value
        .get(key)
        .and_then(Value::as_u64)
        .and_then(|n| u32::try_from(n).ok())
}

impl Manifest {
    /// Load `run.json` of the run in `dir`.
    pub fn load(dir: &Path) -> Result<Manifest> {
        let path = dir.join("run.json");
        let v = read_json(&path)?;
        if text(&v, "format").as_deref() != Some(FORMAT) {
            return Err(bad(&path, format!("not a {FORMAT} file")));
        }
        let need = |key: &str| text(&v, key).ok_or_else(|| bad(&path, format!("no {key}")));
        let mut nodes = Vec::new();
        for n in v
            .get("nodes")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let implementation = match text(n, "implementation").as_deref() {
                Some("heartwood") => Implementation::Heartwood,
                Some("reference") => Implementation::Reference,
                other => return Err(bad(&path, format!("a node's implementation is {other:?}"))),
            };
            nodes.push(Node {
                name: text(n, "name").ok_or_else(|| bad(&path, "a node without a name"))?,
                ip: text(n, "ip").ok_or_else(|| bad(&path, "a node without an address"))?,
                implementation,
                forger: text(n, "role").as_deref() == Some("forger"),
                under_test: n.get("underTest").and_then(Value::as_bool).unwrap_or(false),
            });
        }
        let corpora = v
            .get("corpora")
            .and_then(Value::as_object)
            .map(|o| {
                o.iter()
                    .filter_map(|(k, d)| d.as_str().map(|d| (k.clone(), d.to_owned())))
                    .collect()
            })
            .unwrap_or_default();
        let minority = v.get("minority").and_then(|m| {
            let nodes = m
                .get("nodes")?
                .as_array()?
                .iter()
                .filter_map(|n| n.as_str().map(str::to_owned))
                .collect();
            Some((nodes, text(m, "corpus")?))
        });
        let api = v
            .get("api")
            .and_then(|a| Some((text(a, "left")?, text(a, "right")?)));
        Ok(Manifest {
            run: need("run")?,
            network: need("network")?,
            dry_run: v.get("dryRun").and_then(Value::as_bool).unwrap_or(false),
            seed: need("seed")?,
            round_length: number(&v, "roundLength").unwrap_or(53),
            block_time: number(&v, "blockTime").unwrap_or(8),
            stop_height: number(&v, "stopHeight").unwrap_or(265),
            control: text(&v, "control").unwrap_or_else(|| "node1".into()),
            nodes,
            corpora,
            pair: text(&v, "pair"),
            minority,
            api,
            window_seconds: v
                .get("faultWindowSeconds")
                .and_then(Value::as_i64)
                .unwrap_or(30),
            status: text(&v, "status").unwrap_or_default(),
        })
    }
}

/// A scripted step of `steps.jsonl`.
#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    /// The step's name (`vote-after-node4`, `txgen-once`, ...).
    pub name: String,
    /// The node it concerns, or `all`.
    pub node: String,
    /// Whether it met its expectation.
    pub ok: bool,
    /// When it started, in milliseconds since the Unix epoch.
    pub start: Option<i64>,
    /// The whole line.
    pub record: Value,
}

/// The steps of the run in `dir`.
pub fn steps(dir: &Path) -> Result<Vec<Step>> {
    Ok(read_lines(&dir.join("steps.jsonl"))?
        .into_iter()
        .map(|record| Step {
            name: text(&record, "step").unwrap_or_default(),
            node: text(&record, "node").unwrap_or_default(),
            ok: record.get("ok").and_then(Value::as_bool).unwrap_or(false),
            start: text(&record, "start").and_then(|t| parse_time(&t)),
            record,
        })
        .collect())
}

/// A scripted fault's window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Window {
    /// The fault (`sigkill`, `restart`, `partition`, ...).
    pub fault: String,
    /// The nodes it concerns.
    pub nodes: Vec<String>,
    /// Its start, in milliseconds since the Unix epoch.
    pub start: i64,
    /// The end of its window: `window_seconds` after its recovery.
    pub end: i64,
}

impl Window {
    /// Whether `t` (milliseconds) is inside the window.
    pub fn contains(&self, t: i64) -> bool {
        self.start <= t && t <= self.end
    }

    /// Whether the window is a partition's, which concerns every node.
    pub fn is_partition(&self) -> bool {
        self.fault == "partition"
    }

    /// Whether the window concerns `node`.
    pub fn concerns(&self, node: &str) -> bool {
        self.is_partition() || self.nodes.iter().any(|n| n == node)
    }
}

/// The fault windows of the run in `dir`; a fault without a recovery lasts to the end of time.
pub fn windows(dir: &Path, window_seconds: i64) -> Result<Vec<Window>> {
    let path = dir.join("faults.jsonl");
    let mut out = Vec::new();
    for record in read_lines(&path)? {
        let start = text(&record, "start")
            .and_then(|t| parse_time(&t))
            .ok_or_else(|| bad(&path, format!("a fault without a start: {record}")))?;
        let end = text(&record, "recovered")
            .and_then(|t| parse_time(&t))
            .map_or(i64::MAX, |t| t.saturating_add(window_seconds * 1_000));
        out.push(Window {
            fault: text(&record, "fault").unwrap_or_default(),
            nodes: record
                .get("nodes")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|n| n.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default(),
            start,
            end,
        });
    }
    Ok(out)
}

/// The directories of the archives under `dir`, in the order they were written (their names are
/// UTC stamps).
pub fn archives(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| Error::io(dir, e))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_dir() && p.join("manifest.json").exists())
        .collect();
    out.sort();
    Ok(out)
}

/// Milliseconds since the Unix epoch of a UTC time `YYYY-MM-DDTHH:MM:SS[.fff]Z` (also with a space
/// for the `T`); `None` when `text` is not one.
pub fn parse_time(text: &str) -> Option<i64> {
    let t = text.trim().strip_suffix('Z')?;
    let (date, clock) = t.split_once(['T', ' '])?;
    let mut d = date.splitn(3, '-').map(str::parse::<i64>);
    let (year, month, day) = (d.next()?.ok()?, d.next()?.ok()?, d.next()?.ok()?);
    let (hms, fraction) = clock.split_once('.').unwrap_or((clock, ""));
    let mut c = hms.splitn(3, ':').map(str::parse::<i64>);
    let (hour, minute, second) = (c.next()?.ok()?, c.next()?.ok()?, c.next()?.ok()?);
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    let millis = if fraction.is_empty() {
        0
    } else {
        if !fraction.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let digits: String = fraction.chars().chain("000".chars()).take(3).collect();
        digits.parse::<i64>().ok()?
    };
    // Days from the civil date (the proleptic Gregorian calendar).
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(((days * 24 + hour) * 60 + minute) * 60_000 + second * 1_000 + millis)
}

/// The UTC time `YYYY-MM-DDTHH:MM:SS.fffZ` of `ms` milliseconds since the Unix epoch.
pub fn format_time(ms: i64) -> String {
    let days = ms.div_euclid(86_400_000);
    let rest = ms.rem_euclid(86_400_000);
    // The civil date of a day count (the proleptic Gregorian calendar).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        rest / 3_600_000,
        rest / 60_000 % 60,
        rest / 1_000 % 60,
        rest % 1_000
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_are_read_as_utc() {
        assert_eq!(parse_time("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_time("1970-01-01T00:00:01.5Z"), Some(1_500));
        assert_eq!(
            parse_time("2026-09-27T10:24:19.332Z"),
            Some(1_790_504_659_332)
        );
        assert_eq!(
            parse_time("2026-09-27 10:24:19.332Z"),
            parse_time("2026-09-27T10:24:19.332Z")
        );
        assert_eq!(parse_time("2000-02-29T12:00:00Z"), Some(951_825_600_000));
        for t in [
            "1970-01-01T00:00:00.000Z",
            "2000-02-29T12:00:00.000Z",
            "2026-09-27T10:24:19.332Z",
            "2100-12-31T23:59:59.999Z",
        ] {
            assert_eq!(format_time(parse_time(t).unwrap()), t);
        }
        for bad in [
            "",
            "2026-09-27T10:24:19",
            "2026-13-01T00:00:00Z",
            "x",
            "2026-09-27T10:24:19.3a2Z",
        ] {
            assert_eq!(parse_time(bad), None, "{bad}");
        }
    }

    #[test]
    fn windows_end_after_the_recovery() {
        let w = Window {
            fault: "sigkill".into(),
            nodes: vec!["node4".into()],
            start: 1_000,
            end: 31_000,
        };
        assert!(w.contains(1_000) && w.contains(31_000) && !w.contains(31_001));
        assert!(w.concerns("node4") && !w.concerns("node1"));
        let p = Window {
            fault: "partition".into(),
            ..w
        };
        assert!(p.concerns("node1"));
    }
}
