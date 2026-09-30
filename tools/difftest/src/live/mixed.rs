//! The checks of a mixed run (`w0`, `w`, `p`, `s`: reference nodes beside Heartwood relays, and
//! `w0` their reference baseline): one chain with the judge's lines, the judge of node 1's chain,
//! the reference nodes' acceptance of every Heartwood relay they reach, the reference logs and the
//! Heartwood logs counted per fault window's class against the baseline, the propagation through a
//! Heartwood relay, the fork of run `p`, the relays' processes and listeners, and what is recorded
//! without a gate.
//!
//! Every hook catches its own errors and turns them into a failed case, so one unreadable file
//! fails its own check and no other.

mod accept;
mod logs;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::corpora::{self, Judged};
use super::digest::{self, JudgedChain, NodeDigest};
use super::forging;
use super::nodes;
use super::run::{self, Implementation, Manifest, Node, Step, Window};
use super::steps;
use super::{Cases, LAG_LIMIT_SECONDS, MixedRun};
use crate::corpus::{self, Corpus};
use crate::errors::{Error, Result};
use crate::levels::l2_forge::{self, Direction, ForgeOptions};
use crate::store;

pub(super) use accept::acceptance;
pub(super) use logs::{heartwood_logs, reference_logs};

/// The section of these checks.
pub(super) const SECTION: &str = "mixed";

/// The Heartwood nodes of the run.
pub(super) fn heartwood(manifest: &Manifest) -> Vec<&Node> {
    manifest
        .nodes
        .iter()
        .filter(|n| n.implementation == Implementation::Heartwood)
        .collect()
}

/// The reference nodes of the run.
pub(super) fn references(manifest: &Manifest) -> Vec<&Node> {
    manifest
        .nodes
        .iter()
        .filter(|n| n.implementation == Implementation::Reference)
        .collect()
}

/// The reference relay that joins with an empty store: run `w`'s node 5, run `s`'s node 4.
pub(super) fn fresh_node(manifest: &Manifest) -> Option<&'static str> {
    match manifest.run.as_str() {
        "w" => Some("node5"),
        "s" => Some("node4"),
        _ => None,
    }
}

/// The relay it syncs from, alone: run `w`'s node 4, run `s`'s node 1.
pub(super) fn serving_node(manifest: &Manifest) -> Option<&'static str> {
    match manifest.run.as_str() {
        "w" => Some("node4"),
        "s" => Some("node1"),
        _ => None,
    }
}

/// The first step named `name`.
pub(super) fn step<'s>(steps: &'s [Step], name: &str) -> Option<&'s Step> {
    steps.iter().find(|s| s.name == name)
}

/// The address of the node `name`.
pub(super) fn ip_of<'m>(manifest: &'m Manifest, name: &str) -> Option<&'m str> {
    manifest.node(name).map(|n| n.ip.as_str())
}

/// Run `f`; an error it returns is a failed case named `label`, not the end of the check.
pub(super) fn guard(
    cases: &mut Cases,
    section: &str,
    label: &str,
    f: impl FnOnce(&mut Cases) -> Result<()>,
) -> Result<()> {
    if let Err(e) = f(cases) {
        cases.fail(section, label, e.to_string());
    }
    Ok(())
}

/// The chains of the run, imported and judged.
pub(super) struct Chains {
    /// Each corpus of the manifest, by name.
    pub judged: BTreeMap<String, Judged>,
    /// The name of the final chain: the first that is not the minority's.
    pub last: String,
    /// The name of the minority's chain, when the run has one.
    pub minority: Option<String>,
}

impl Chains {
    /// The final chain.
    pub(super) fn last(&self) -> &Judged {
        &self.judged[&self.last]
    }

    /// The minority's chain.
    pub(super) fn minority(&self) -> Option<&Judged> {
        self.minority.as_ref().and_then(|m| self.judged.get(m))
    }
}

/// Import and judge every corpus of the run (a corpus already imported and judged is only read).
pub(super) fn chains(run: &MixedRun<'_>) -> Result<Chains> {
    let mut judged = BTreeMap::new();
    for (name, archives) in &run.manifest.corpora {
        judged.insert(
            name.clone(),
            corpora::prepare(
                run.run_dir,
                name,
                archives,
                &run.manifest.seed,
                run.corpus_root,
                run.oracle,
            )?,
        );
    }
    let minority = run.manifest.minority.as_ref().map(|(_, c)| c.clone());
    let last = judged
        .keys()
        .find(|n| Some(*n) != minority.as_ref())
        .cloned()
        .ok_or_else(|| Error::Usage("the run names no chain to judge".into()))?;
    Ok(Chains {
        judged,
        last,
        minority,
    })
}

/// A node's peer list at one poll.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Poll {
    /// When, in milliseconds since the Unix epoch.
    pub at: i64,
    /// The node that was asked.
    pub node: String,
    /// The peers it listed.
    pub peers: Vec<PeerEntry>,
}

/// A peer of a poll's list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PeerEntry {
    /// The peer's address.
    pub ip: String,
    /// The ports the list gives for an API of the peer (a `ports` key that names one).
    pub api: Vec<u64>,
}

impl Poll {
    /// Whether the poll lists `ip`.
    pub(super) fn lists(&self, ip: &str) -> bool {
        self.peers.iter().any(|p| p.ip == ip)
    }
}

/// The time of a record: `t` as a UTC text or as milliseconds.
fn record_time(v: &Value) -> Option<i64> {
    let t = v.get("t").or_else(|| v.get("ts"))?;
    t.as_i64().or_else(|| t.as_str().and_then(run::parse_time))
}

/// The file of the recipe's peer polls.
pub(super) const PEER_POLLS: &str = "peer-polls.jsonl";

/// The peer lists of [`PEER_POLLS`]: `{t, node, ok, peers: [ip | {ip, port?, ports?}]}` per poll,
/// with the machine's load beside them. A poll that got no list (`ok` false, or no `peers`: the
/// node was down or did not answer) is no poll of its list and is left out.
pub(super) fn polls(run_dir: &Path) -> Result<Vec<Poll>> {
    let mut out = Vec::new();
    for v in run::read_lines(&run_dir.join(PEER_POLLS))? {
        let (Some(at), Some(node)) = (record_time(&v), v.get("node").and_then(Value::as_str))
        else {
            continue;
        };
        if v.get("ok") == Some(&Value::Bool(false)) {
            continue;
        }
        let Some(listed) = v.get("peers").and_then(Value::as_array) else {
            continue;
        };
        let peers = listed
            .iter()
            .filter_map(|p| {
                if let Some(ip) = p.as_str() {
                    let ip = ip.split(':').next().unwrap_or(ip);
                    return Some(PeerEntry {
                        ip: ip.to_owned(),
                        api: Vec::new(),
                    });
                }
                let ip = p.get("ip").and_then(Value::as_str)?.to_owned();
                let api = p
                    .get("ports")
                    .and_then(Value::as_object)
                    .map(|o| {
                        o.iter()
                            .filter(|(k, _)| k.to_ascii_lowercase().contains("api"))
                            .filter_map(|(_, v)| v.as_u64())
                            .collect()
                    })
                    .unwrap_or_default();
                Some(PeerEntry { ip, api })
            })
            .collect();
        out.push(Poll {
            at,
            node: node.to_owned(),
            peers,
        });
    }
    out.sort_by_key(|p| p.at);
    Ok(out)
}

/// The baseline run: its directory, manifest and fault windows.
pub(super) struct Baseline {
    /// The run's directory.
    pub dir: PathBuf,
    /// Its manifest.
    pub manifest: Manifest,
    /// Its fault windows.
    pub windows: Vec<Window>,
}

/// The other reference runs of the baseline's recipe (`--spread`), loaded as the baseline is.
pub(super) fn spread(run: &MixedRun<'_>) -> Result<Vec<Baseline>> {
    run.spread
        .iter()
        .map(|dir| {
            let manifest = Manifest::load(dir)?;
            if manifest.has_heartwood() {
                return Err(Error::Usage(format!(
                    "--spread {}: no run of reference nodes only",
                    dir.display()
                )));
            }
            let windows = run::windows(dir, manifest.window_seconds)?;
            Ok(Baseline {
                dir: dir.clone(),
                manifest,
                windows,
            })
        })
        .collect()
}

/// The baseline of a mixed run: none for `w0` itself; for the others the run given, or an error
/// when none was given.
pub(super) fn baseline(run: &MixedRun<'_>) -> Result<Option<Baseline>> {
    if run.manifest.run == "w0" {
        return Ok(None);
    }
    let dir = run.baseline.ok_or_else(|| {
        Error::Usage("no baseline run given: compare with the w0 run (--compare)".into())
    })?;
    let manifest = Manifest::load(dir)?;
    let windows = run::windows(dir, manifest.window_seconds)?;
    Ok(Some(Baseline {
        dir: dir.to_path_buf(),
        manifest,
        windows,
    }))
}

// ---------------------------------------------------------------------------------------------
// One chain
// ---------------------------------------------------------------------------------------------

/// The height node 6's rollback command was given: the `rollback` step's `height` (or
/// `requested`), else the plan's 133.
pub(super) fn rollback_asked(steps: &[Step]) -> u64 {
    let step = step(steps, "rollback");
    step.and_then(|s| s.number("height").or_else(|| s.number("requested")))
        .unwrap_or(133)
}

/// The height node 6's rollback lands on, derived from the height the command was given: the last
/// block of the round before that height's round, as the reference's rollback does. The step's
/// own record is not read for it, so a record that names the height it was given `target` cannot
/// be taken for the landing.
pub(super) fn rollback_target(steps: &[Step], round_length: u32) -> u32 {
    let l = u64::from(round_length.max(1));
    u32::try_from((rollback_asked(steps).saturating_sub(1) / l) * l).unwrap_or(0)
}

/// Every Heartwood relay's last `apply` or `reapply` line per height of the final chain is the
/// judge's; run `w`'s node 6 also from the start after its rollback, whose lines begin at the
/// rollback's target and run without a gap to the final tip.
pub(super) fn one_chain(run: &MixedRun<'_>, cases: &mut Cases) -> Result<()> {
    guard(
        cases,
        SECTION,
        "one chain with the judge's lines",
        |cases| {
            let nodes = heartwood(run.manifest);
            if nodes.is_empty() {
                cases.record(
                SECTION,
                "no Heartwood relay in this run: the reference nodes' chains are compared with node 1's by the node checks",
            );
                return Ok(());
            }
            let chains = chains(run)?;
            let judged = vec![chains.last().chain.clone()];
            for node in nodes {
                let label = format!("{}: one chain with the judge's lines", node.name);
                let mut against = judged.clone();
                if let (Some((members, _)), Some(m)) = (&run.manifest.minority, chains.minority())
                    && members.contains(&node.name)
                {
                    against.push(m.chain.clone());
                }
                let dir = run.run_dir.join("nodes").join(&node.name).join("difftest");
                let read = match digest::read(&dir) {
                    Ok(d) if !d.files.is_empty() => d,
                    Ok(_) => {
                        cases.fail(
                            SECTION,
                            label,
                            format!("no digest file in {}", dir.display()),
                        );
                        continue;
                    }
                    Err(e) => {
                        cases.fail(SECTION, label, e.to_string());
                        continue;
                    }
                };
                verdict(cases, &label, &node.name, &read, &against);
                if run.manifest.run == "w" && node.name == "node6" {
                    after_rollback(run, cases, &read, &chains.last().chain)?;
                }
            }
            Ok(())
        },
    )
}

/// A digest judged: the first problems as failures, or one passing case.
fn verdict(cases: &mut Cases, label: &str, name: &str, read: &NodeDigest, against: &[JudgedChain]) {
    let judged = digest::judge(read, against);
    let mut problems = judged.problems.clone();
    if !judged.uncovered.is_empty() {
        problems.push(format!(
            "{} heights of the final chain without an apply line of its block, from {}",
            judged.uncovered.len(),
            judged.uncovered[0]
        ));
    }
    if problems.is_empty() {
        let compared: usize = judged.compared.values().sum();
        cases.pass(
            SECTION,
            format!(
                "{name}: {compared} digest lines in {} starts equal the judge's, every height of the final chain applied",
                read.files.len()
            ),
        );
    } else {
        for p in problems.iter().take(5) {
            cases.fail(SECTION, label, p.clone());
        }
        if problems.len() > 5 {
            cases.fail(
                SECTION,
                label,
                format!("... and {} more", problems.len() - 5),
            );
        }
    }
}

/// The start of node 6 that opens at the rollback's target, judged by itself.
fn after_rollback(
    run: &MixedRun<'_>,
    cases: &mut Cases,
    read: &NodeDigest,
    last: &JudgedChain,
) -> Result<()> {
    let label = "node6: the start after the rollback";
    let steps = run::steps(run.run_dir)?;
    let target = rollback_target(&steps, run.manifest.round_length);
    let mut seen = BTreeSet::new();
    let opening = read
        .lines
        .iter()
        .find(|l| seen.insert(l.start) && l.step() == "restart" && l.height() == target);
    let Some(opening) = opening else {
        cases.fail(
            SECTION,
            label,
            format!(
                "no start of node 6 opens with a restart line at the rollback's target {target}"
            ),
        );
        return Ok(());
    };
    let lines: Vec<_> = read
        .lines
        .iter()
        .filter(|l| l.start == opening.start)
        .cloned()
        .collect();
    let part = NodeDigest {
        lines,
        torn: Vec::new(),
        files: Vec::new(),
    };
    let judged = digest::judge(&part, std::slice::from_ref(last));
    let mut problems = judged.problems.clone();
    if !judged.uncovered.is_empty() {
        problems.push(format!(
            "{} heights after the target without an apply line, from {}",
            judged.uncovered.len(),
            judged.uncovered[0]
        ));
    }
    if problems.is_empty() {
        cases.pass(
            SECTION,
            format!(
                "node6: start {} opens at the rollback's target {target} and applies every block to the tip {}",
                opening.start,
                last.tip()
            ),
        );
    } else {
        for p in problems.iter().take(5) {
            cases.fail(SECTION, label, p.clone());
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// The judge
// ---------------------------------------------------------------------------------------------

/// Node 1's chain (its archive, or in run `s` its block files) judged, with every block accepted;
/// in run `s` the reference also rebuilds every block Heartwood forged.
pub(super) fn judge(run: &MixedRun<'_>, cases: &mut Cases) -> Result<()> {
    guard(cases, SECTION, "the judge of node 1's chain", |cases| {
        let chains = chains(run)?;
        for (name, judged) in &chains.judged {
            let label = format!("{name}: the reference processor accepted every block");
            let rows = run::read_lines(&judged.dir.join(corpus::TS_PROCESSOR))?;
            let accepted: BTreeSet<u64> = rows
                .iter()
                .filter(|r| r["step"] == "apply" && r["result"] == "Accepted")
                .filter_map(|r| r["h"].as_u64())
                .collect();
            let refused = rows
                .iter()
                .filter(|r| r.get("result").is_some_and(|x| x != "Accepted"))
                .count();
            let missing: Vec<u64> = (2..=u64::from(judged.tip))
                .filter(|h| !accepted.contains(h))
                .collect();
            if missing.is_empty() && refused == 0 {
                cases.pass(
                    SECTION,
                    format!(
                        "{name}: the reference processor accepted the schedule and economics of all {} blocks",
                        judged.tip.saturating_sub(1)
                    ),
                );
            } else {
                cases.fail(
                    SECTION,
                    label,
                    format!(
                        "{} heights without an accepted result (first {:?}), {refused} results that are not accepted",
                        missing.len(),
                        missing.first()
                    ),
                );
            }
        }
        if run.manifest.run == "s" {
            forge_replay(run, cases, &chains.last().dir)?;
        }
        Ok(())
    })
}

/// Run `s`: the reference rebuilds each block Heartwood's forgers made.
fn forge_replay(run: &MixedRun<'_>, cases: &mut Cases, dir: &Path) -> Result<()> {
    let label = "the reference rebuilds every block Heartwood forged";
    guard(cases, SECTION, label, |cases| {
        let loaded = Corpus::load(dir)?;
        let oracle = (run.oracle)()?;
        let mut server = oracle.server()?;
        let options = ForgeOptions {
            direction: Direction::Reference,
            to: None,
        };
        let report = store::guarded(|| l2_forge::run(&loaded, &options, Some(&mut server)))
            .map_err(|store::Unbuilt(m)| Error::NotYet(format!("not built yet: {m}")))??;
        if report.is_success() && report.compared() > 0 {
            cases.pass(
                SECTION,
                format!("{label}: {} cases compared", report.compared()),
            );
        } else {
            cases.fail(
                SECTION,
                label,
                format!(
                    "{} of {} forge cases failed",
                    report.failed(),
                    report.compared()
                ),
            );
        }
        Ok(())
    })
}

// ---------------------------------------------------------------------------------------------
// Propagation
// ---------------------------------------------------------------------------------------------

/// The windows of `relay` as windows of `node`, so a lag measure leaves them out.
fn as_windows_of(windows: &[Window], relay: &str, node: &str) -> Vec<Window> {
    windows
        .iter()
        .filter(|w| w.concerns(relay))
        .map(|w| Window {
            nodes: vec![node.to_owned()],
            ..w.clone()
        })
        .collect()
}

/// The median and the maximum of the block lags of `node` against the control, in seconds, with
/// the count; the heights inside the node's windows and, when given, `relay`'s are left out.
pub(super) fn lag_stats(
    dir: &Path,
    manifest: &Manifest,
    windows: &[Window],
    node: &str,
    relay: Option<&str>,
    from: i64,
) -> Result<Option<(f64, f64, usize)>> {
    let arrivals = steps::arrivals(&dir.join("propagation.jsonl"))?;
    let mut all = windows.to_vec();
    if let Some(relay) = relay {
        all.extend(as_windows_of(windows, relay, node));
    }
    let lags = steps::lags(&arrivals, node, &manifest.control, &all, from);
    if lags.is_empty() {
        return Ok(None);
    }
    let mut sorted: Vec<f64> = lags.iter().map(|(_, l)| *l).collect();
    sorted.sort_by(f64::total_cmp);
    Ok(Some((
        sorted[sorted.len() / 2],
        sorted[sorted.len() - 1],
        sorted.len(),
    )))
}

/// Transactions and blocks through a Heartwood relay, both ways: the batch sent to the fresh node
/// reached every other pool within a block time, and the fresh node held each block within the lag
/// limit of the control, leaving out the windows of its own and of the relay it syncs from.
pub(super) fn propagation(run: &MixedRun<'_>, cases: &mut Cases) -> Result<()> {
    guard(
        cases,
        SECTION,
        "the propagation through a Heartwood relay",
        |cases| {
            let m = run.manifest;
            let (Some(fresh), Some(serving)) = (fresh_node(m), serving_node(m)) else {
                let why = if m.run == "p" {
                    "the propagation of run p is covered by its fork checks"
                } else {
                    "run w0 is the baseline: its propagation is what run w's is compared with"
                };
                cases.record(SECTION, why);
                return Ok(());
            };
            let steps = run::steps(run.run_dir)?;
            let batch = format!("batch-{fresh}");
            match step(&steps, &batch) {
                None => cases.fail(
                    SECTION,
                    format!("the batch through {fresh}"),
                    format!("no {batch} step"),
                ),
                Some(s) => {
                    let limit = i64::from(m.block_time) * 1_000;
                    let relays = s
                        .record
                        .pointer("/detail/relays")
                        .and_then(Value::as_object);
                    let mut problems = Vec::new();
                    if !s.ok {
                        problems.push(format!("the step {batch} did not meet its expectation"));
                    }
                    match relays {
                        None => {
                            problems.push("the step records no relays (detail.relays)".to_owned())
                        }
                        Some(relays) => {
                            for node in m
                                .nodes
                                .iter()
                                .filter(|n| n.name != s.node && n.name != fresh)
                            {
                                match relays.get(&node.name).and_then(|r| r.get("seenAllMs")).and_then(Value::as_i64) {
                                None => problems.push(format!("{}: never saw the whole batch", node.name)),
                                Some(ms) if ms >= limit => problems.push(format!(
                                    "{}: saw the batch after {ms} ms, not within a block time ({limit} ms)",
                                    node.name
                                )),
                                Some(_) => {}
                            }
                            }
                        }
                    }
                    if problems.is_empty() {
                        cases.pass(
                            SECTION,
                            format!(
                                "{batch}: the batch reached every other pool within a block time"
                            ),
                        );
                    } else {
                        for p in problems {
                            cases.fail(SECTION, format!("the batch through {fresh}"), p);
                        }
                    }
                }
            }
            // The blocks are timed from the batch on, which follows the node's sync.
            let from = step(&steps, &batch).and_then(|s| s.start).unwrap_or(0);
            let label = format!("{fresh}: every block within {LAG_LIMIT_SECONDS} s of node 1's");
            let arrivals = steps::arrivals(&run.run_dir.join("propagation.jsonl"))?;
            let mut all = run.windows.to_vec();
            all.extend(as_windows_of(run.windows, serving, fresh));
            let lags = steps::lags(&arrivals, fresh, &m.control, &all, from);
            if lags.is_empty() {
                cases.fail(
                    SECTION,
                    label,
                    "no block arrival of the node was recorded (propagation.jsonl)",
                );
            } else {
                let late: Vec<String> = lags
                    .iter()
                    .filter(|(_, l)| *l > LAG_LIMIT_SECONDS)
                    .map(|(h, l)| format!("{h} ({l:.1} s)"))
                    .collect();
                if late.is_empty() {
                    cases.pass(
                    SECTION,
                    format!(
                        "{fresh}: {} blocks within {LAG_LIMIT_SECONDS} s of node 1's, outside its windows and {serving}'s",
                        lags.len()
                    ),
                );
                } else {
                    cases.fail(
                        SECTION,
                        label,
                        format!(
                            "{} late blocks: {}",
                            late.len(),
                            late.iter().take(20).cloned().collect::<Vec<_>>().join(", ")
                        ),
                    );
                }
            }
            Ok(())
        },
    )
}

// ---------------------------------------------------------------------------------------------
// The fork
// ---------------------------------------------------------------------------------------------

/// The ids of the transactions of the corpus in `dir`.
fn transaction_ids(dir: &Path) -> Result<BTreeSet<String>> {
    let corpus = Corpus::load(dir)?;
    let mut out = BTreeSet::new();
    for b in &corpus.blocks {
        let block = heartwood_crypto::Block::decode(&b.bytes, &corpus.milestones)
            .map_err(|e| Error::Usage(format!("block {}: {e}", b.height)))?;
        for id in block.transaction_ids() {
            out.insert(id.to_hex());
        }
    }
    Ok(out)
}

/// When node 4 left the fork, recorded beside the steps that reconnected nodes after the heal
/// (node 4 itself among them, of either implementation): the heal passed through node 4 alone
/// only when it came first. A reference node whose peers are the minority's cannot come first (its
/// health check needs a majority of the attested delegates among its peers), and a Heartwood relay
/// behaves as it does.
fn node4_left_the_fork(run: &MixedRun<'_>, steps: &[Step], cases: &mut Cases) -> Result<()> {
    let heartwood4 = heartwood(run.manifest).iter().any(|n| n.name == "node4");
    let dir = match heartwood4 {
        true => run.run_dir.join("nodes/node4/log"),
        false => run.run_dir.join("logs/node4"),
    };
    let heal = step(steps, "heal").and_then(|s| s.start);
    let detected = (nodes::read_logs(&dir)?.into_iter())
        .filter(|l| l.text.contains("Fork detected"))
        .filter_map(|l| l.at)
        .find(|&at| heal.is_none_or(|h| at >= h));
    let Some(at) = detected else {
        let why = "node 4's log has no \"Fork detected\" line after the heal";
        cases.fail(SECTION, "node 4 left the minority's chain", why);
        return Ok(());
    };
    let before: Vec<String> = (steps.iter())
        .filter(|s| s.name.starts_with("reconnect-"))
        .filter_map(|s| Some((s.name.as_str(), s.start.filter(|&t| t < at)?)))
        .map(|(name, t)| format!("{name} at {}", run::format_time(t)))
        .collect();
    let when = run::format_time(at);
    cases.record(
        SECTION,
        if before.is_empty() {
            format!("node 4 detected the fork at {when}, before any reconnect step: the heal passed through it")
        } else {
            format!(
                "node 4 detected the fork at {when}, after {}: the heal did not pass through node 4 alone",
                before.join(" and ")
            )
        },
    );
    Ok(())
}

/// Run `p`: the minority's blocks through node 4 only, both sides on the majority's chain after
/// the heal, and when node 4 left the fork beside the steps that reconnected nodes.
pub(super) fn fork(run: &MixedRun<'_>, cases: &mut Cases) -> Result<()> {
    guard(
        cases,
        SECTION,
        "the minority's blocks through a Heartwood relay, and the heal",
        |cases| {
            let m = run.manifest;
            if m.run != "p" {
                return Ok(());
            }
            let chains = chains(run)?;
            let Some(minority) = chains.minority() else {
                cases.fail(SECTION, "the fork", "the run records no minority chain");
                return Ok(());
            };
            let last = chains.last();
            let node4 = ip_of(m, "node4").unwrap_or("").to_owned();
            // The minority's own blocks: those the final chain does not hold.
            let own: BTreeMap<u64, String> = minority
                .chain
                .forward
                .iter()
                .filter_map(|(h, l)| {
                    let id = l.get("id").and_then(Value::as_str)?;
                    (!last.chain.holds(*h, id)).then(|| (u64::from(*h), id.to_owned()))
                })
                .collect();
            // Node 5's receipts of those blocks: from node 4 alone.
            let text = nodes::read_log_text(&run.run_dir.join("logs/node5"))?;
            let receipts = forging::receipts_of("node5", &text);
            let of_own: Vec<_> = receipts
                .iter()
                .filter(|r| own.get(&r.height).is_some_and(|id| *id == r.id))
                .collect();
            let strangers: BTreeSet<&str> = of_own
                .iter()
                .map(|r| r.from.as_str())
                .filter(|f| *f != node4)
                .collect();
            if own.is_empty() {
                cases.fail(
                    SECTION,
                    "node 5's minority blocks came from node 4 only",
                    "the minority has no block of its own",
                );
            } else if of_own.is_empty() {
                cases.fail(
                    SECTION,
                    "node 5's minority blocks came from node 4 only",
                    "node 5's log shows no receipt of a minority block",
                );
            } else if strangers.is_empty() {
                cases.pass(
                    SECTION,
                    format!(
                        "node 5 received {} minority blocks, every one from node 4",
                        of_own.len()
                    ),
                );
            } else {
                cases.fail(
                    SECTION,
                    "node 5's minority blocks came from node 4 only",
                    format!(
                        "received from {}",
                        strangers.into_iter().collect::<Vec<_>>().join(", ")
                    ),
                );
            }
            // Its peer list: node 4 alone, until the recipe reopened its routes after the heal (a
            // node 5 left on the minority's tip: `reconnect-node5`). The reconnects of either
            // implementation are judged against the recipe's rule with the run's steps.
            let steps = run::steps(run.run_dir)?;
            let reopened = step(&steps, "reconnect-node5").and_then(|s| s.start);
            let polls = polls(run.run_dir)?;
            let mine: Vec<&Poll> = polls
                .iter()
                .filter(|p| p.node == "node5" && reopened.is_none_or(|t| p.at < t))
                .collect();
            let odd: BTreeSet<&str> = mine
                .iter()
                .flat_map(|p| p.peers.iter().map(|e| e.ip.as_str()))
                .filter(|ip| *ip != node4)
                .collect();
            if mine.is_empty() {
                cases.fail(SECTION, "node 5's peer list", "no poll of node 5's peers");
            } else if odd.is_empty() {
                cases.pass(
                    SECTION,
                    format!(
                        "node 5's peer list held node 4 alone in {} polls",
                        mine.len()
                    ),
                );
            } else {
                cases.fail(
                    SECTION,
                    "node 5's peer list",
                    format!(
                        "also listed {}",
                        odd.into_iter().collect::<Vec<_>>().join(", ")
                    ),
                );
            }
            // The heal: both sides on the final tip.
            match step(&steps, "converged") {
                Some(s) if s.ok => cases.pass(SECTION, "the sides converged after the heal"),
                Some(_) => cases.fail(
                    SECTION,
                    "the sides converged after the heal",
                    "the converged step failed",
                ),
                None => cases.fail(
                    SECTION,
                    "the sides converged after the heal",
                    "no converged step",
                ),
            }
            node4_left_the_fork(run, &steps, cases)?;
            let final_json = run::read_json(&run.run_dir.join("final.json"))?;
            let want = Corpus::load(&last.dir)?
                .block_id(last.tip)
                .map(|id| id.to_hex())
                .unwrap_or_default();
            for node in ["node4", "node5"] {
                let tip = final_json
                    .pointer(&format!("/tips/{node}/id"))
                    .and_then(Value::as_str);
                if tip == Some(want.as_str()) {
                    cases.pass(
                        SECTION,
                        format!("{node} ends on the majority's tip {}", last.tip),
                    );
                } else {
                    cases.fail(
                        SECTION,
                        format!("{node} ends on the majority's tip"),
                        format!("its tip is {tip:?}, the final chain's {want}"),
                    );
                }
            }
            // Node 4's lines hold both chains.
            if !heartwood(m).iter().any(|n| n.name == "node4") {
                cases.record(
                    SECTION,
                    "node 4 is a reference relay (a dry run of the recipe): it has no digest lines, and its archive is judged by the node checks",
                );
                rejections(run, cases, &chains)?;
                return conflict(run, cases, &steps, &last.dir);
            }
            let read = digest::read(&run.run_dir.join("nodes/node4/difftest"))?;
            let judged = digest::judge(&read, &[last.chain.clone(), minority.chain.clone()]);
            let on_minority: usize = judged
                .compared
                .iter()
                .filter(|((c, _), _)| *c == minority.chain.name)
                .map(|(_, n)| *n)
                .sum();
            if judged.problems.is_empty() && on_minority > 0 {
                cases.pass(SECTION, format!("node 4's digest holds {on_minority} lines of the minority's chain and equals the judge's"));
            } else {
                cases.fail(
                    SECTION,
                    "node 4's digest across the fork",
                    format!(
                        "{on_minority} lines of the minority's chain; problems: {}",
                        judged.problems.first().cloned().unwrap_or_default()
                    ),
                );
            }
            rejections(run, cases, &chains)?;
            conflict(run, cases, &steps, &last.dir)
        },
    )
}

/// Node 5's refused and undone blocks: none of the final chain's; the undone are the minority's.
/// A line without the block's id (the reference logs "Undoing block <height>") is judged by its
/// height: an undone height is one where the minority's chain holds a block of its own.
fn rejections(run: &MixedRun<'_>, cases: &mut Cases, chains: &Chains) -> Result<()> {
    let path = run.run_dir.join("rejections/node5.jsonl");
    let Ok(lines) = run::read_lines(&path) else {
        cases.record(
            SECTION,
            "node 5 wrote no record of refused or undone blocks",
        );
        return Ok(());
    };
    if lines.is_empty() {
        cases.record(SECTION, "node 5 refused and undid no block");
        return Ok(());
    }
    let mut problems = Vec::new();
    for l in &lines {
        let kind = l.get("kind").and_then(Value::as_str).unwrap_or("");
        let id = l.get("id").and_then(Value::as_str).unwrap_or("");
        let h = l
            .get("h")
            .or_else(|| l.get("height"))
            .and_then(Value::as_u64)
            .and_then(|h| u32::try_from(h).ok())
            .unwrap_or(0);
        if id.is_empty() {
            let own = chains.minority().is_some_and(|m| {
                m.chain
                    .forward
                    .get(&h)
                    .and_then(|l| l.get("id").and_then(Value::as_str))
                    .is_some_and(|mid| !chains.last().chain.holds(h, mid))
            });
            if kind != "undone" || !own {
                problems.push(format!(
                    "node 5 {kind} a block at {h}, where the minority's chain holds no block of its own"
                ));
            }
        } else if chains.last().chain.holds(h, id) {
            problems.push(format!(
                "node 5 {kind} block {id} at {h}, which the final chain holds"
            ));
        } else if kind == "undone" && !chains.minority().is_some_and(|m| m.chain.holds(h, id)) {
            problems.push(format!(
                "node 5 undid block {id} at {h}, which the minority's chain does not hold"
            ));
        }
    }
    if problems.is_empty() {
        cases.pass(
            SECTION,
            format!(
                "node 5's {} refused or undone blocks are none of the final chain's",
                lines.len()
            ),
        );
    } else {
        for p in problems.iter().take(5) {
            cases.fail(SECTION, "node 5's refused and undone blocks", p.clone());
        }
    }
    Ok(())
}

/// The conflicting transfers: the majority's is in the final chain, the minority's is not.
fn conflict(run: &MixedRun<'_>, cases: &mut Cases, steps: &[Step], dir: &Path) -> Result<()> {
    let label =
        "the conflicting transfers: the majority's is in the final chain, the minority's is not";
    let Some(s) = step(steps, "conflict-outcome").or_else(|| step(steps, "conflict")) else {
        cases.fail(SECTION, label, "no conflict step");
        return Ok(());
    };
    let _ = run;
    // The ids the step names, else the recipe's conflict step: `a` submitted through node 1 (the
    // majority's side), `b` through node 5 (the minority's).
    let submitted = |side: &str| {
        step(steps, "conflict")
            .and_then(|c| c.record.pointer(&format!("/detail/{side}/id")))
            .and_then(Value::as_str)
            .map(str::to_owned)
    };
    let (Some(majority), Some(minority)) = (
        s.text("majority").or_else(|| submitted("a")),
        s.text("minority").or_else(|| submitted("b")),
    ) else {
        cases.fail(
            SECTION,
            label,
            "the step records no majority and minority transaction ids",
        );
        return Ok(());
    };
    let ids = transaction_ids(dir)?;
    match (ids.contains(&majority), ids.contains(&minority)) {
        (true, false) => cases.pass(SECTION, label),
        (a, b) => cases.fail(
            SECTION,
            label,
            format!("the final chain holds the majority's transfer: {a}; the minority's: {b}"),
        ),
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// The processes
// ---------------------------------------------------------------------------------------------

/// The relay's peer-to-peer port.
const P2P_PORT: u64 = 6002;

/// The address of Docker's embedded name server, which listens in every container's namespace from
/// a socket no process of the container holds.
const DOCKER_DNS: &str = "127.0.0.11";

/// What one Heartwood node's process report shows: the problems, and what is recorded.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct Listening {
    /// What fails the check.
    pub problems: Vec<String>,
    /// What is recorded without a gate.
    pub records: Vec<String>,
}

/// Judge the process report of the Heartwood node `node`: the recipe's scan of its container
/// (`processes/<node>.jsonl`), one line per process (`{"kind": "process", "pid", "ppid",
/// "cmdline"}`) and per listening socket (`{"kind": "listener", "addr", "port", "pid"}`). The relay
/// process (`heartwood relay run`, with its children) listens on the peer-to-peer port, and a
/// relay-only node only on its own address there; a forging node's forger process (`heartwood
/// forger run`) exists and listens on nothing; no socket is held by no process but Docker's name
/// server. A forging node's relay listens on every address so that its forger reaches it over the
/// loopback (recorded), and the relay's other listeners (the API) are recorded.
pub(super) fn listening(node: &Node, lines: &[Value]) -> Listening {
    let mut out = Listening::default();
    let name = &node.name;
    let mut parents: BTreeMap<u64, (u64, String)> = BTreeMap::new();
    let mut listeners: Vec<(String, u64, u64)> = Vec::new();
    for l in lines {
        let number = |k: &str| l.get(k).and_then(Value::as_u64).unwrap_or(0);
        match l.get("kind").and_then(Value::as_str) {
            Some("process") => {
                let cmdline = l.get("cmdline").and_then(Value::as_str).unwrap_or("");
                parents.insert(number("pid"), (number("ppid"), cmdline.to_owned()));
            }
            Some("listener") => {
                let addr = l.get("addr").and_then(Value::as_str).unwrap_or("");
                listeners.push((addr.to_owned(), number("port"), number("pid")));
            }
            _ => {}
        }
    }
    if parents.is_empty() {
        out.problems
            .push(format!("{name}: no process in its report"));
        return out;
    }
    // The role of a process: the first of it and its ancestors that runs a role.
    let role = |pid: u64| -> Option<&'static str> {
        let mut at = pid;
        for _ in 0..64 {
            let (parent, cmdline) = parents.get(&at)?;
            let words: Vec<&str> = cmdline.split_whitespace().collect();
            if words.contains(&"run") {
                if words.contains(&"forger") {
                    return Some("forger");
                }
                if words.contains(&"relay") {
                    return Some("relay");
                }
            }
            at = *parent;
        }
        None
    };
    let has = |wanted: &str| parents.keys().any(|pid| role(*pid) == Some(wanted));
    if !has("relay") {
        out.problems.push(format!("{name}: no relay process"));
    }
    if node.forger && !has("forger") {
        out.problems.push(format!("{name}: no forger process"));
    }
    let mut p2p = Vec::new();
    for (addr, port, pid) in &listeners {
        match (*pid, role(*pid)) {
            (0, _) if addr == DOCKER_DNS => {}
            (0, _) => out.problems.push(format!(
                "{name}: a socket on {addr}:{port} that no process of the container holds"
            )),
            (_, Some("forger")) => out
                .problems
                .push(format!("{name}: its forger listens on {addr}:{port}")),
            (_, Some("relay")) if *port == P2P_PORT => p2p.push(addr.clone()),
            (_, Some("relay")) => out
                .records
                .push(format!("{name}: its relay also listens on {addr}:{port}")),
            _ => {}
        }
    }
    if p2p.is_empty() {
        out.problems.push(format!(
            "{name}: its relay does not listen on the peer-to-peer port {P2P_PORT}"
        ));
    } else if node.forger {
        out.records.push(format!(
            "{name}: its relay listens on {} port {P2P_PORT}, where its forger reaches it over the loopback",
            p2p.join(", ")
        ));
    } else {
        for addr in p2p.iter().filter(|a| **a != node.ip) {
            out.problems.push(format!(
                "{name}: its relay listens on {addr}:{P2P_PORT}, not on its own address {} only",
                node.ip
            ));
        }
    }
    out
}

/// Every Heartwood relay listens on its node address only; run `s`'s forgers listen on nothing.
pub(super) fn processes(run: &MixedRun<'_>, cases: &mut Cases) -> Result<()> {
    guard(
        cases,
        SECTION,
        "the processes and their listeners",
        |cases| {
            let nodes = heartwood(run.manifest);
            if nodes.is_empty() {
                cases.record(
                    SECTION,
                    "no Heartwood relay in this run: no listener to check",
                );
                return Ok(());
            }
            let mut problems = Vec::new();
            for node in &nodes {
                let path = run
                    .run_dir
                    .join("processes")
                    .join(format!("{}.jsonl", node.name));
                let lines = match run::read_lines(&path) {
                    Ok(lines) => lines,
                    Err(e) => {
                        problems.push(format!("{}: no process report ({e})", node.name));
                        continue;
                    }
                };
                let judged = listening(node, &lines);
                problems.extend(judged.problems);
                for r in judged.records {
                    cases.record(SECTION, r);
                }
            }
            if problems.is_empty() {
                cases.pass(
                    SECTION,
                    format!(
                        "{} Heartwood nodes: each relay listens on the peer-to-peer port (a relay-only node on its own address only), each forger on nothing",
                        nodes.len()
                    ),
                );
            } else {
                for p in problems.iter().take(10) {
                    cases.fail(SECTION, "the processes and their listeners", p.clone());
                }
            }
            Ok(())
        },
    )
}

// ---------------------------------------------------------------------------------------------
// Records
// ---------------------------------------------------------------------------------------------

/// The seconds a step records.
fn seconds(steps: &[Step], name: &str) -> Option<f64> {
    let s = step(steps, name)?;
    s.record
        .get("seconds")
        .or_else(|| s.record.pointer("/detail/seconds"))
        .and_then(Value::as_f64)
}

/// The records that are not gated: the sync and re-sync times, the block lags, the cleanse
/// infractions, the probe's timings and the recipe's workarounds.
pub(super) fn recorded(run: &MixedRun<'_>, cases: &mut Cases) -> Result<()> {
    guard(cases, SECTION, "the records", |cases| {
        let m = run.manifest;
        let steps = run::steps(run.run_dir)?;
        let base = baseline(run).unwrap_or_default();
        let base_steps = match &base {
            Some(b) => run::steps(&b.dir).ok(),
            None => None,
        };
        let against = |f: &dyn Fn(&[Step]) -> Option<String>| -> String {
            base_steps
                .as_deref()
                .and_then(f)
                .map_or(String::new(), |t| format!(" (w0: {t})"))
        };
        if let Some(s) = seconds(&steps, "node5-synced") {
            let w0 = against(&|st| seconds(st, "node5-synced").map(|v| format!("{v} s")));
            cases.record(
                SECTION,
                format!("node 5 synced from an empty store in {s} s{w0}"),
            );
        }
        if let Some(s) = seconds(&steps, "rollback-open") {
            let w0 = against(&|st| seconds(st, "rollback-open").map(|v| format!("{v} s")));
            cases.record(SECTION, format!("node 6 re-synced to the tip in {s} s{w0}"));
        }
        let mut lag_nodes: Vec<String> = heartwood(m).iter().map(|n| n.name.clone()).collect();
        lag_nodes.extend(fresh_node(m).map(str::to_owned));
        for node in lag_nodes {
            let from = step(&steps, &format!("batch-{node}"))
                .and_then(|s| s.start)
                .unwrap_or(0);
            let ours = lag_stats(run.run_dir, m, run.windows, &node, serving_node(m), from)?;
            let theirs = match &base {
                Some(b) => lag_stats(
                    &b.dir,
                    &b.manifest,
                    &b.windows,
                    &node,
                    serving_node(m),
                    from,
                )
                .ok()
                .flatten(),
                None => None,
            };
            let show = |s: Option<(f64, f64, usize)>| {
                s.map_or("no block measured".to_owned(), |(med, max, n)| {
                    format!("median {med:.1} s, max {max:.1} s over {n} blocks")
                })
            };
            cases.record(
                SECTION,
                format!(
                    "{node}: block lag {}{}",
                    show(ours),
                    base.as_ref()
                        .map_or(String::new(), |_| format!(" (w0: {})", show(theirs)))
                ),
            );
        }
        // The cleanse's infractions in the reference logs that name a Heartwood relay.
        let count = |dir: &Path, manifest: &Manifest, ips: &[String]| -> usize {
            references(manifest)
                .iter()
                .filter_map(|n| nodes::read_logs(&dir.join("logs").join(&n.name)).ok())
                .flatten()
                .filter(|l| {
                    (l.text.contains("Removed") || l.text.contains("Suspend"))
                        && ips.iter().any(|ip| l.text.contains(ip.as_str()))
                })
                .count()
        };
        let ips: Vec<String> = heartwood(m).iter().map(|n| n.ip.clone()).collect();
        if !ips.is_empty() {
            let theirs = base.as_ref().map(|b| {
                let names: Vec<String> = heartwood(m).iter().map(|n| n.name.clone()).collect();
                let ips: Vec<String> = names
                    .iter()
                    .filter_map(|n| ip_of(&b.manifest, n).map(str::to_owned))
                    .collect();
                count(&b.dir, &b.manifest, &ips)
            });
            cases.record(
                SECTION,
                format!(
                    "cleanse infractions naming a Heartwood relay in the reference logs: {}{}",
                    count(run.run_dir, m, &ips),
                    theirs.map_or(String::new(), |t| format!(" (w0: {t} for the same nodes)"))
                ),
            );
        }
        // The probe's output.
        if let Ok(entries) = std::fs::read_dir(run.run_dir.join("probe")) {
            let mut files: Vec<_> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
            files.sort();
            for f in files
                .iter()
                .filter(|f| f.extension().is_some_and(|e| e == "jsonl"))
            {
                let lines = run::read_lines(f).map(|l| l.len()).unwrap_or(0);
                cases.record(
                    SECTION,
                    format!(
                        "probe output {}: {lines} records",
                        f.file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default()
                    ),
                );
            }
        }
        for s in &steps {
            if s.name == "reconnect-minority"
                || s.name.starts_with("reconnect-node")
                || s.record.get("workaround").is_some()
            {
                let why = s
                    .text("workaround")
                    .or_else(|| s.text("note"))
                    .unwrap_or_else(|| "the recipe reconnected the minority".to_owned());
                cases.record(
                    SECTION,
                    format!("workaround in step {} of {}: {why}", s.name, s.node),
                );
            }
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn node(name: &str, ip: &str, forger: bool) -> Node {
        Node {
            name: name.into(),
            ip: ip.into(),
            implementation: Implementation::Heartwood,
            forger,
            under_test: true,
        }
    }

    fn process(pid: u64, ppid: u64, cmdline: &str) -> Value {
        json!({"kind": "process", "pid": pid, "ppid": ppid, "comm": "x", "cmdline": cmdline})
    }

    fn listener(addr: &str, port: u64, pid: u64) -> Value {
        json!({"kind": "listener", "proto": "tcp", "addr": addr, "port": port, "pid": pid})
    }

    /// A container's report: the entrypoint, the relay, and the forger when `forger`.
    fn report(forger: bool, extra: &[Value]) -> Vec<Value> {
        let mut out = vec![
            process(1, 0, "/usr/bin/tini -s -- /opt/heartwood/entrypoint.sh"),
            process(7, 1, "bash /opt/heartwood/entrypoint.sh"),
            process(34, 7, "heartwood relay run --token solar --network testnet"),
            listener("127.0.0.11", 42373, 0),
            listener("0.0.0.0", 6003, 34),
        ];
        if forger {
            out.push(process(
                112,
                7,
                "heartwood forger run --token solar --network testnet",
            ));
        }
        out.extend_from_slice(extra);
        out
    }

    #[test]
    fn a_forging_node_has_a_listening_relay_and_a_silent_forger() {
        let one = node("node1", "10.84.0.11", true);
        let judged = listening(&one, &report(true, &[listener("0.0.0.0", 6002, 34)]));
        assert_eq!(judged.problems, Vec::<String>::new());
        assert!(
            judged
                .records
                .iter()
                .any(|r| r.contains("over the loopback")),
            "{judged:?}"
        );
        let judged = listening(
            &one,
            &report(
                true,
                &[
                    listener("0.0.0.0", 6002, 34),
                    listener("127.0.0.1", 4000, 112),
                ],
            ),
        );
        assert!(
            judged
                .problems
                .iter()
                .any(|p| p.contains("forger listens on 127.0.0.1:4000")),
            "{judged:?}"
        );
        let judged = listening(&one, &report(false, &[listener("0.0.0.0", 6002, 34)]));
        assert!(
            judged
                .problems
                .iter()
                .any(|p| p.contains("no forger process")),
            "{judged:?}"
        );
        assert!(
            listening(&one, &[]).problems[0].contains("no process"),
            "an empty report is no evidence"
        );
    }

    #[test]
    fn a_relay_only_node_listens_on_its_own_address() {
        let four = node("node4", "10.82.0.14", false);
        let at = |ip: &str| listening(&four, &report(false, &[listener(ip, 6002, 34)]));
        assert!(at("10.82.0.14").problems.is_empty());
        let found = at("0.0.0.0").problems;
        assert!(
            found.iter().any(|p| p.contains("not on its own address")),
            "{found:?}"
        );
        let found = listening(&four, &report(false, &[])).problems;
        assert!(
            found
                .iter()
                .any(|p| p.contains("does not listen on the peer-to-peer port")),
            "{found:?}"
        );
        let found = listening(
            &four,
            &report(
                false,
                &[
                    listener("10.82.0.14", 6002, 34),
                    listener("10.82.0.14", 9000, 0),
                ],
            ),
        )
        .problems;
        assert!(
            found
                .iter()
                .any(|p| p.contains("no process of the container holds")),
            "{found:?}"
        );
    }
}
