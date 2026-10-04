//! The run directory of a live run, as the devnet tooling writes it.
//!
//! - `run.json`: the run's name, network, seed, round length and stop height, the nodes (address,
//!   implementation, role, whether it is under test), the chains to judge (`corpora`: a corpus name
//!   and the directory of the archives it is imported from), the pair run (`pair`, for `h`: `r`), the
//!   minority of a partition (`minority`) and the route corpus's nodes (`api`); for a run with a
//!   Heartwood node, the relay image it started (`relayImage`), the SHA-256 of the binary in it
//!   (`relayBinarySha256`, the image's label) and the commit of the tree it was built from
//!   (`worktreeCommit`, `worktreeClean`);
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
//!   pause; `check-chain.json`: the tooling's check of the control's chain (schedule and supply);
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

/// A route corpus a run recorded: the node under test, the control and the run's directory of
/// its answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiCorpus {
    /// The node under test.
    pub left: String,
    /// The control node.
    pub right: String,
    /// The directory of its `answers.jsonl`, relative to the run.
    pub dir: String,
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
    /// The route corpora the run recorded after that one (`apiExtra`).
    pub api_extra: Vec<ApiCorpus>,
    /// Seconds a fault window lasts after its recovery.
    pub window_seconds: i64,
    /// `running`, `complete` or `failed`.
    pub status: String,
    /// The relay image the run started for its Heartwood nodes.
    pub relay_image: Option<String>,
    /// The SHA-256 (lowercase hex) of the relay binary in that image.
    pub relay_binary_sha256: Option<String>,
    /// The commit of the tree the relay was built from, and whether that tree had no change.
    pub worktree: Option<(String, bool)>,
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
        let api_extra = (v
            .get("apiExtra")
            .and_then(Value::as_array)
            .into_iter()
            .flatten())
        .filter_map(|a| {
            Some(ApiCorpus {
                left: text(a, "left")?,
                right: text(a, "right")?,
                dir: text(a, "dir")?,
            })
        })
        .collect();
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
            api_extra,
            window_seconds: v
                .get("faultWindowSeconds")
                .and_then(Value::as_i64)
                .unwrap_or(30),
            status: text(&v, "status").unwrap_or_default(),
            relay_image: text(&v, "relayImage").filter(|t| !t.is_empty()),
            relay_binary_sha256: text(&v, "relayBinarySha256").filter(|t| !t.is_empty()),
            worktree: text(&v, "worktreeCommit").map(|commit| {
                let clean = v.get("worktreeClean").and_then(Value::as_bool);
                (commit, clean.unwrap_or(false))
            }),
        })
    }

    /// Whether the run started a Heartwood relay (a dry run starts none).
    pub fn has_heartwood(&self) -> bool {
        !self.dry_run
            && self
                .nodes
                .iter()
                .any(|n| n.implementation == Implementation::Heartwood)
    }
}

/// The release binary of the tree a check runs in, relative to its directory: the relay that the
/// tree's `make relay-image` builds and packs into the image a live run starts.
pub const RELAY_BINARY: &str = "target/release/heartwood";

/// The SHA-256 (lowercase hex) of the file at `path`, read in pieces.
fn file_sha256(path: &Path) -> Result<String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut file = std::fs::File::open(path).map_err(|e| Error::io(path, e))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1 << 20];
    loop {
        let n = file.read(&mut buffer).map_err(|e| Error::io(path, e))?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

/// Whether the relay the run tested is `binary`, the release binary of the tree being judged: the
/// run's `relayBinarySha256` against the file's. `Ok` holds what matched; `Err` why the run cannot
/// count for this tree (another binary, no record of one, no binary to compare). `None` for a run
/// without a Heartwood relay.
pub fn relay_binding(
    manifest: &Manifest,
    binary: &Path,
) -> Option<std::result::Result<String, String>> {
    if !manifest.has_heartwood() {
        return None;
    }
    let image = manifest
        .relay_image
        .as_deref()
        .unwrap_or("an unnamed image");
    let Some(tested) = &manifest.relay_binary_sha256 else {
        return Some(Err(format!(
            "run.json records no relayBinarySha256: which relay {image} held is unknown"
        )));
    };
    let ours = match file_sha256(binary) {
        Ok(sha) => sha,
        Err(e) => {
            return Some(Err(format!(
                "no release binary of this tree to compare ({e}): build it with make relay-image"
            )));
        }
    };
    let built = manifest
        .worktree
        .as_ref()
        .map(|(commit, clean)| {
            format!(
                ", built from {commit}{}",
                if *clean { "" } else { " with changes" }
            )
        })
        .unwrap_or_default();
    Some(if &ours == tested {
        Ok(format!(
            "the relay the run tested ({image}, binary sha256 {tested}{built}) is this tree's release binary"
        ))
    } else {
        Err(format!(
            "the run tested the relay binary {tested} ({image}{built}); this tree's {} is {ours}: repeat the run with this tree's image (make relay-image, then make live)",
            binary.display()
        ))
    })
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

impl Step {
    /// The height the step started at, as the recipe recorded it (`startHeight`).
    pub fn start_height(&self) -> Option<u64> {
        self.record.get("startHeight").and_then(Value::as_u64)
    }

    /// A time the step's record holds under `key` (`end`, `contactNode3At`, ...) in its record or
    /// in its `detail` object, in milliseconds since the Unix epoch.
    pub fn time(&self, key: &str) -> Option<i64> {
        self.text(key).and_then(|t| parse_time(&t))
    }

    /// A number the step's record holds under `key`, in its record or in its `detail` object.
    pub fn number(&self, key: &str) -> Option<u64> {
        self.record
            .get(key)
            .or_else(|| self.record.get("detail").and_then(|d| d.get(key)))
            .and_then(Value::as_u64)
    }

    /// A text the step's record holds under `key`, in its record or in its `detail` object.
    pub fn text(&self, key: &str) -> Option<String> {
        self.record
            .get(key)
            .or_else(|| self.record.get("detail").and_then(|d| d.get(key)))
            .and_then(Value::as_str)
            .map(str::to_owned)
    }
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

    /// The window's class, the unit the mixed runs' counts compare in: the recipe's classes
    /// (`stall`, `restart`, `hold`, `isolation`, `rollback`, `start`, `network-start`,
    /// `not-started`) as they are, a kill, a stop and a marker restart as `restart`, a fresh
    /// node's sync as `start`, and any other fault (`partition`, ...) under its own name.
    pub fn class(&self) -> &str {
        match self.fault.as_str() {
            "sigint" | "sigkill" | "sigterm" | "marker-restart" => "restart",
            "fresh-sync" => "start",
            other => other,
        }
    }
}

/// The window that a line at `t` concerning `node` falls in: the one that started last among
/// those that concern the node and contain `t`.
pub fn window_at<'w>(windows: &'w [Window], node: &str, t: i64) -> Option<&'w Window> {
    windows
        .iter()
        .filter(|w| w.concerns(node) && w.contains(t))
        .max_by_key(|w| w.start)
}

/// The class of a line between two nodes the run's route table kept apart ([`kept_apart`]).
pub const UNREACHABLE: &str = "unreachable";

/// The window of a partition's minority after the heal: from the `heal` step's start to
/// `window_seconds` after the end of the `converged` step, for the nodes of `run.json`'s `minority`.
///
/// A partition leaves each side with the other side's nodes gone from its peer lists, and a
/// reference node rechecks the network only when its peers attest a majority of the delegates
/// ("Not enough peers available to check network health"). So the minority can keep its side of
/// the fork, unlisted by the majority's nodes, until a node of it falls back to its seeds (in run
/// `p`, the scripted restart of node 3's relay, which the recipe does 120 s after the heal when
/// the tips still differ). Its blocks and its listing in that span are the heal's, not a relay's
/// behaviour. Its class, `minority`, is one the baseline never has: its log lines are recorded,
/// as the partition's are.
pub fn minority_window(manifest: &Manifest, steps: &[Step]) -> Option<Window> {
    let (members, _) = manifest.minority.as_ref()?;
    let heal = steps.iter().find(|s| s.name == "heal")?.start?;
    let converged = steps.iter().find(|s| s.name == "converged")?.time("end")?;
    (converged >= heal).then(|| Window {
        fault: "minority".to_owned(),
        nodes: members.clone(),
        start: heal,
        end: converged.saturating_add(manifest.window_seconds.saturating_mul(1_000)),
    })
}

/// A span of a node's reach-only routes (`routes.jsonl`, written by the tooling's `reach-only`
/// and `reach-all`): the nodes it could reach from `start` to `end`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reach {
    /// The node whose routes were limited.
    pub node: String,
    /// The nodes it could still reach (none for an isolation).
    pub allowed: Vec<String>,
    /// When its routes were limited, in milliseconds since the Unix epoch.
    pub start: i64,
    /// `window_seconds` after its routes next changed, as a fault window ends after its
    /// recovery; the end of time when they never did.
    pub end: i64,
}

/// The reach-only spans of the run in `dir`, from `routes.jsonl` (none when it has no such file);
/// a change the tooling recorded as failed left the routes as they were.
pub fn reaches(dir: &Path, window_seconds: i64) -> Result<Vec<Reach>> {
    let path = dir.join("routes.jsonl");
    let mut out: Vec<Reach> = Vec::new();
    for record in read_lines(&path)? {
        if record.get("ok").and_then(Value::as_bool) == Some(false) {
            continue;
        }
        let (Some(node), Some(t)) = (
            text(&record, "node"),
            text(&record, "t").and_then(|t| parse_time(&t)),
        ) else {
            return Err(bad(
                &path,
                format!("a route change without a node or a time: {record}"),
            ));
        };
        let end = t.saturating_add(window_seconds.saturating_mul(1_000));
        for open in out
            .iter_mut()
            .filter(|r| r.node == node && r.end == i64::MAX)
        {
            open.end = end;
        }
        if text(&record, "action").as_deref() == Some("reach-only") {
            let allowed = text(&record, "allowed").unwrap_or_default();
            out.push(Reach {
                node,
                allowed: allowed
                    .split([',', ' '])
                    .filter(|n| !n.is_empty() && *n != "-")
                    .map(str::to_owned)
                    .collect(),
                start: t,
                end: i64::MAX,
            });
        }
    }
    Ok(out)
}

/// Whether the route table kept `a` and `b` apart at `t`: a reach-only span of either that leaves
/// the other out. A line one of them logged of the other then shows the route, not the peer.
pub fn kept_apart(reaches: &[Reach], a: &str, b: &str, t: i64) -> bool {
    reaches.iter().any(|r| {
        r.start <= t
            && t <= r.end
            && ((r.node == a && !r.allowed.iter().any(|n| n == b))
                || (r.node == b && !r.allowed.iter().any(|n| n == a)))
    })
}

/// The route table's spans as windows of class [`UNREACHABLE`], for the comparison of a run's
/// counts with a baseline's: each span of a node of `readers` or `named` concerns the nodes of
/// `named` it kept apart from a reader (a reader's span: the named nodes it could not reach; a
/// named node's span: that node).
pub fn reach_windows(reaches: &[Reach], readers: &[String], named: &[String]) -> Vec<Window> {
    reaches
        .iter()
        .filter_map(|r| {
            let reader = readers.contains(&r.node);
            let nodes: Vec<String> = named
                .iter()
                .filter(|n| **n == r.node || (reader && !r.allowed.contains(n)))
                .cloned()
                .collect();
            (!nodes.is_empty()).then(|| Window {
                fault: UNREACHABLE.to_owned(),
                nodes,
                start: r.start,
                end: r.end,
            })
        })
        .collect()
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

/// The directory of the crypto files of the node whose files are in `node_dir`, a directory of the
/// run in `run_dir`: its own `crypto/` (a node of the tooling's native runs), else its archived
/// configuration's `config/crypto/` (a node in a container), else the run's copy of the generated
/// network's, `network/crypto/`.
pub fn crypto_dir(run_dir: &Path, node_dir: &Path) -> PathBuf {
    [
        node_dir.join("crypto"),
        node_dir.join("config").join("crypto"),
    ]
    .into_iter()
    .find(|d| d.join("network.json").is_file())
    .unwrap_or_else(|| run_dir.join("network").join("crypto"))
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
            ..w.clone()
        };
        assert!(p.concerns("node1"));
        assert_eq!(p.class(), "partition");
        assert_eq!(w.class(), "restart");
    }

    #[test]
    fn a_line_belongs_to_the_last_window_that_holds_it() {
        let window = |fault: &str, start: i64, end: i64| Window {
            fault: fault.into(),
            nodes: vec!["node6".into()],
            start,
            end,
        };
        let all = [window("isolation", 0, 100), window("rollback", 40, 80)];
        assert_eq!(window_at(&all, "node6", 50).unwrap().class(), "rollback");
        assert_eq!(window_at(&all, "node6", 90).unwrap().class(), "isolation");
        assert!(window_at(&all, "node6", 101).is_none());
        assert!(window_at(&all, "node4", 50).is_none());
    }

    #[test]
    fn a_step_reads_its_height_and_details() {
        let record = serde_json::json!({"step": "r3", "node": "node6", "ok": true,
            "startHeight": 112, "contactNode3At": "1970-01-01T00:00:02.000Z",
            "detail": {"kept": 1, "outcome": "refused"}});
        let step = Step {
            name: "r3".into(),
            node: "node6".into(),
            ok: true,
            start: None,
            record,
        };
        assert_eq!(step.start_height(), Some(112));
        assert_eq!(step.time("contactNode3At"), Some(2_000));
        assert_eq!(step.number("kept"), Some(1));
        assert_eq!(step.text("outcome").as_deref(), Some("refused"));
        assert_eq!(step.text("missing"), None);
    }

    fn step_at(name: &str, start: &str, end: &str) -> Step {
        Step {
            name: name.into(),
            node: "all".into(),
            ok: true,
            start: parse_time(start),
            record: serde_json::json!({"step": name, "start": start, "end": end}),
        }
    }

    fn minority_manifest(run: &str) -> Manifest {
        Manifest {
            run: run.into(),
            network: "mixed-p".into(),
            dry_run: false,
            seed: "solar-devnet".into(),
            round_length: 53,
            block_time: 8,
            stop_height: 159,
            control: "node1".into(),
            nodes: Vec::new(),
            corpora: BTreeMap::new(),
            pair: None,
            minority: Some((
                vec!["node3".into(), "node4".into(), "node5".into()],
                "live-p-minority".into(),
            )),
            api: None,
            api_extra: Vec::new(),
            window_seconds: 30,
            status: "complete".into(),
            relay_image: None,
            relay_binary_sha256: None,
            worktree: None,
        }
    }

    #[test]
    fn the_minority_holds_a_window_from_the_heal_until_the_tips_converged() {
        let steps = [
            step_at(
                "heal",
                "2026-09-29T18:30:41.409Z",
                "2026-09-29T18:30:48.614Z",
            ),
            step_at(
                "converged",
                "2026-09-29T18:30:48.630Z",
                "2026-09-29T18:33:41.666Z",
            ),
        ];
        let w = minority_window(&minority_manifest("p"), &steps).expect("a window");
        assert_eq!(w.class(), "minority");
        assert_eq!(w.nodes, ["node3", "node4", "node5"]);
        assert_eq!(Some(w.start), parse_time("2026-09-29T18:30:41.409Z"));
        assert_eq!(Some(w.end), parse_time("2026-09-29T18:34:11.666Z"));
        assert!(!w.concerns("node1") && w.concerns("node4"));
        // No window without a heal, a convergence or a minority.
        assert_eq!(minority_window(&minority_manifest("p"), &steps[..1]), None);
        assert_eq!(minority_window(&minority_manifest("p"), &steps[1..]), None);
        let mut none = minority_manifest("p");
        none.minority = None;
        assert_eq!(minority_window(&none, &steps), None);
    }

    fn routes_file(lines: &[&str]) -> crate::store::ScratchDir {
        let dir = crate::store::ScratchDir::new("live-routes").unwrap();
        std::fs::write(dir.path().join("routes.jsonl"), lines.join("\n")).unwrap();
        dir
    }

    #[test]
    fn the_route_table_keeps_a_reach_only_node_apart_from_the_others() {
        let dir = routes_file(&[
            r#"{"t":"1970-01-01T00:00:10.000Z","node":"node4","action":"reach-only","allowed":"node1","height":20,"ok":true}"#,
            r#"{"t":"1970-01-01T00:01:40.000Z","node":"node4","action":"reach-all","allowed":"all","height":56,"ok":true}"#,
            r#"{"t":"1970-01-01T00:00:20.000Z","node":"node6","action":"reach-only","allowed":"-","height":21,"ok":true}"#,
            r#"{"t":"1970-01-01T00:00:30.000Z","node":"node5","action":"reach-only","allowed":"node2","height":22,"ok":false}"#,
        ]);
        let reaches = reaches(dir.path(), 30).unwrap();
        assert_eq!(reaches.len(), 2, "{reaches:?}");
        assert_eq!(reaches[0].allowed, ["node1"]);
        // node 4 reaches node 1 only, until 30 s after its routes opened at 100 s
        assert!(kept_apart(&reaches, "node4", "node2", 50_000));
        assert!(kept_apart(&reaches, "node2", "node4", 50_000));
        assert!(!kept_apart(&reaches, "node4", "node1", 50_000));
        assert!(kept_apart(&reaches, "node4", "node3", 130_000));
        assert!(!kept_apart(&reaches, "node4", "node3", 130_001));
        assert!(!kept_apart(&reaches, "node4", "node3", 9_999));
        // node 6 reaches no one from 20 s to the end; a failed change leaves the routes as they were
        assert!(kept_apart(&reaches, "node1", "node6", 10_000_000));
        assert!(!kept_apart(&reaches, "node5", "node3", 40_000));
        // For a comparison: the named nodes each span leaves a reader of the list without.
        let readers = ["node1".to_owned(), "node4".to_owned()];
        let named = ["node2".to_owned(), "node6".to_owned()];
        let windows = reach_windows(&reaches, &readers, &named);
        assert_eq!(windows.len(), 2);
        assert!(windows.iter().all(|w| w.class() == UNREACHABLE));
        assert_eq!(windows[0].nodes, ["node2", "node6"]);
        assert_eq!(windows[1].nodes, ["node6"]);
        assert!(reach_windows(&reaches, &["node2".to_owned()], &["node3".to_owned()]).is_empty());
    }

    #[test]
    fn a_node_reads_its_own_crypto_files_first() {
        let dir = crate::store::ScratchDir::new("live-crypto").unwrap();
        let run = dir.path();
        let put = |d: &Path| {
            std::fs::create_dir_all(d).unwrap();
            std::fs::write(d.join("network.json"), "{}").unwrap();
        };
        assert_eq!(
            crypto_dir(run, &run.join("nodes/node1")),
            run.join("network/crypto")
        );
        put(&run.join("nodes/node1/config/crypto"));
        assert_eq!(
            crypto_dir(run, &run.join("nodes/node1")),
            run.join("nodes/node1/config/crypto")
        );
        put(&run.join("nodes/node1/crypto"));
        assert_eq!(
            crypto_dir(run, &run.join("nodes/node1")),
            run.join("nodes/node1/crypto")
        );
    }
}
