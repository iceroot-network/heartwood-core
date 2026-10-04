//! The mixed forging runs' checks (`mixed-short`, `mixed`, `mixed-split`): every forger produced
//! in every round outside its fault windows; the scripted votes admitted on both sides, none for a
//! restarted relay's own delegates before the next round's start; a partition's sides and their
//! convergence after the heal; the attestation evidence (after round 1 every reference relay lists
//! a version for every Heartwood-hosted delegate, and every Heartwood relay's network-state record
//! counts all 53 delegates before a partition); and the schedule. The reference relays' ban lines
//! that name a Heartwood address, and the Heartwood relays' violations against a reference node,
//! are judged with every run's bans and nodes.
//!
//! A reference node writes no block file, so the blocks of the chain are the Heartwood relays':
//! each holds every block it applied, the blocks of an abandoned branch included. A delegate's
//! blocks are found by its public key, and a validator's delegates come from `run.json`.
//!
//! In the mixed run, a Heartwood relay of a partition's minority may apply a block its side forged
//! in the slots before the relay dropped the peers it lost: no judged chain holds it. Such a block
//! is recorded by the node's digest check, and judged here: the minority's branch (the final
//! chain's blocks up to the fork and the minority's blocks) is built from a node's block files,
//! imported and judged by the oracle with its processor results, and the node's lines of those
//! blocks are compared with the branch's as any other line is with its chain
//! ([`minority_branches`]). A run without such a block judges no branch.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde_json::Value;

use super::digest::{self, JudgedChain, NodeDigest, RecordedBlocks};
use super::forging::{self, Answer, QUORUM, Recorded, Slots, Validator};
use super::run::{self, Implementation, Manifest, Step, Window};
use super::{Cases, ForgingRun, corpora, mixed, schedule};
use crate::corpus::TS_PROCESSOR;
use crate::errors::{Error, Result};
use crate::store::ScratchDir;

const SECTION: &str = "mixed-forging";

/// The slots after a partition starts that a side without the delegates' reach may still forge in:
/// its relay drops the peers it can no longer reach one by one.
const MAX_GRACE_SLOTS: i64 = 4;

/// What the checks read of the run: its validators, the blocks the Heartwood relays applied and
/// the chain's slots.
pub(super) struct Data {
    pub(super) validators: Vec<Validator>,
    /// The validators whose relay is a Heartwood node.
    pub(super) hosted: Vec<String>,
    pub(super) blocks: BTreeMap<String, Recorded>,
    pub(super) answers: Vec<Answer>,
    pub(super) slots: Slots,
    pub(super) reach: u64,
    pub(super) problems: Vec<String>,
}

impl Data {
    /// The validator that holds the delegate `key`.
    pub(super) fn holder(&self, key: &str) -> Option<&Validator> {
        self.validators.iter().find(|v| v.keys.contains_key(key))
    }

    /// The start of `slot`, in milliseconds since the Unix epoch.
    pub(super) fn slot_start(&self, slot: i64) -> i64 {
        self.slots.epoch_ms + slot * self.slots.slot_ms
    }
}

pub(super) fn load(run: &ForgingRun<'_>) -> Result<Data> {
    load_records(run.run_dir, run.manifest)
}

fn load_records(dir: &Path, m: &Manifest) -> Result<Data> {
    let mut heartwood = m.clone();
    heartwood
        .nodes
        .retain(|n| n.implementation == Implementation::Heartwood);
    let milestones = forging::milestones(dir, m)?;
    let epoch_ms = heartwood_crypto::crypto::slots::epoch_unix_ms(milestones.at(1).epoch())
        .ok_or_else(|| Error::Usage("the chain's epoch is not a time".into()))?;
    let (blocks, mut problems) = forging::recorded_blocks(dir, &heartwood, &milestones)?;
    let (answers, more) = forging::answers(dir, &heartwood)?;
    problems.extend(more);
    let validators = forging::validators(dir)?;
    let hosted = validators
        .iter()
        .filter(|v| heartwood.node(&v.name).is_some())
        .map(|v| v.name.clone())
        .collect();
    let json = run::read_json(&dir.join("run.json"))?;
    Ok(Data {
        validators,
        hosted,
        blocks,
        answers,
        slots: Slots {
            epoch_ms,
            slot_ms: i64::from(m.block_time.max(1)) * 1_000,
        },
        reach: json["minimumDelegateReach"].as_u64().unwrap_or(QUORUM),
        problems,
    })
}

/// The mixed forging runs' checks.
pub(super) fn check(run: &ForgingRun<'_>, cases: &mut Cases) -> Result<()> {
    let m = run.manifest;
    let steps = run::steps(run.run_dir)?;
    if m.dry_run
        || !m
            .nodes
            .iter()
            .any(|n| n.implementation == Implementation::Heartwood)
    {
        cases.fail(
            SECTION,
            "a mixed forging run with Heartwood nodes",
            "its run.json names no Heartwood node, or a dry run: no block file or record to judge",
        );
        return Ok(());
    }
    let data = match load(run) {
        Ok(data) => data,
        Err(e) => {
            cases.fail(
                SECTION,
                "the Heartwood nodes' block files and records",
                e.to_string(),
            );
            return Ok(());
        }
    };
    if !data.problems.is_empty() {
        for p in data.problems.iter().take(6) {
            cases.fail(
                SECTION,
                "the Heartwood nodes' block files and records",
                p.clone(),
            );
        }
        return Ok(());
    }
    verdict(
        cases,
        "every forger produced in every round outside its fault windows",
        production(run, &data),
    );
    verdict(
        cases,
        "every Heartwood-hosted block had a quorum answer in its slot",
        quorum_before(&data),
    );
    verdict(
        cases,
        "every Heartwood relay counted all 53 delegates before the partition",
        counts(run, &data, &steps),
    );
    verdict(
        cases,
        "the votes were admitted, and none was placed for a restarted relay's own delegates before the next round",
        votes(run, &data, &steps),
    );
    verdict(
        cases,
        "every reference relay lists a version for every Heartwood-hosted delegate",
        evidence(run, &data),
    );
    if mixed::step(&steps, "partition").is_some() {
        let sides = partition(run, &data, &steps, cases);
        verdict(
            cases,
            "the partition's sides forged as their delegates' reach allows",
            sides,
        );
        minority_branches(run, &steps, cases);
    } else {
        cases.record(SECTION, "no partition in this run");
    }
    verdict(cases, "one tip at the end", one_tip(run));
    let m = run.manifest;
    verdict(
        cases,
        "every step in its planned round and order",
        schedule::problems(&m.run, &steps, m.round_length, m.stop_height),
    );
    Ok(())
}

fn verdict(cases: &mut Cases, label: &str, problems: Vec<String>) {
    if problems.is_empty() {
        cases.pass(SECTION, label);
    }
    for p in problems.iter().take(12) {
        cases.fail(SECTION, label, p.clone());
    }
    if problems.len() > 12 {
        cases.fail(
            SECTION,
            label,
            format!("... and {} more", problems.len() - 12),
        );
    }
}

/// The rounds the run completed: those that end at or before its stop height.
fn rounds(m: &Manifest) -> u32 {
    m.stop_height / m.round_length.max(1)
}

/// Every validator produced a block in every round, outside its fault windows, unless its windows
/// leave no slot of the round free.
pub(super) fn production(run: &ForgingRun<'_>, data: &Data) -> Vec<String> {
    let m = run.manifest;
    let length = m.round_length.max(1);
    let mut problems = Vec::new();
    for round in 1..=rounds(m) {
        let (first, last) = ((round - 1) * length + 1, round * length);
        let slots: Vec<i64> = data
            .blocks
            .values()
            .filter(|b| (first.max(2)..=last).contains(&b.height))
            .map(|b| b.slot)
            .collect();
        let (Some(low), Some(high)) = (slots.iter().min(), slots.iter().max()) else {
            problems.push(format!(
                "round {round}: no block in the Heartwood relays' files"
            ));
            continue;
        };
        for v in &data.validators {
            let mine: Vec<&Window> = run.windows.iter().filter(|w| w.concerns(&v.name)).collect();
            let free = |slot: i64| {
                let t = data.slot_start(slot);
                !mine.iter().any(|w| w.contains(t))
            };
            let produced = data
                .blocks
                .values()
                .filter(|b| (first.max(2)..=last).contains(&b.height))
                .any(|b| v.keys.contains_key(&b.key) && free(b.slot));
            if !produced && (*low..=*high).any(free) {
                problems.push(format!(
                    "{}: no block in round {round} (heights {first} to {last}) outside its fault windows",
                    v.name
                ));
            }
        }
    }
    problems
}

/// Whether the reference's forger forges on `answer` (`isForgingAllowed`): the network state is
/// the default one (not below the run's `minimumDelegateReach`, not a cold start), and it can forge
/// (`canForge`): of the delegates counted either way, at least the milestone's quorum (27 of 53)
/// agree, or all of them when fewer are counted. A partition side of 21 delegates thus forges when
/// all 21 agree; with 27 or more counted, 27 must agree whatever the run's reach.
fn allows_forging(answer: &Answer) -> bool {
    let counted = answer.has_quorum + answer.no_quorum;
    answer.status == Some(0) && counted > 0 && answer.has_quorum >= QUORUM.min(counted)
}

/// Every block a Heartwood-hosted delegate forged followed, in its slot, an answer of its own
/// relay at the height below it on which the reference's forger forges ([`allows_forging`]).
pub(super) fn quorum_before(data: &Data) -> Vec<String> {
    let mut problems = Vec::new();
    for b in data.blocks.values() {
        let Some(holder) = data.holder(&b.key) else {
            problems.push(format!(
                "block {} at height {}: its generator is no validator's delegate",
                b.id, b.height
            ));
            continue;
        };
        if !data.hosted.contains(&holder.name) {
            continue;
        }
        let evidence = data.answers.iter().any(|a| {
            a.relay == holder.name
                && data.slots.at(a.at) == b.slot
                && a.node_height == Some(u64::from(b.height) - 1)
                && allows_forging(a)
        });
        if !evidence {
            problems.push(format!(
                "block {} at height {} (slot {}): no answer of {}'s relay in its slot at height {} on which the reference forges (the default status, and hasQuorum at least {QUORUM} or every delegate counted)",
                b.id, b.height, b.slot, holder.name, b.height - 1
            ));
        }
    }
    problems
}

/// The start of the run's first partition, when it has one.
fn partition_start(windows: &[Window]) -> Option<i64> {
    windows
        .iter()
        .filter(|w| w.is_partition())
        .map(|w| w.start)
        .min()
}

/// Every Heartwood relay counted all 53 delegates in one answer before the partition.
pub(super) fn counts(run: &ForgingRun<'_>, data: &Data, _steps: &[Step]) -> Vec<String> {
    let cutoff = partition_start(run.windows).unwrap_or(i64::MAX);
    let hosted: Vec<Validator> = data
        .validators
        .iter()
        .filter(|v| data.hosted.contains(&v.name))
        .cloned()
        .collect();
    let delegates = data.validators.iter().map(|v| v.keys.len() as u64).sum();
    forging::full_counts(&data.answers, &hosted, cutoff, None, delegates)
}

/// The steps `vote-s18a` and `vote-s18b` were admitted, and no vote was placed for a delegate of a
/// relay restarted in the same round before the next round began.
fn votes(run: &ForgingRun<'_>, data: &Data, steps: &[Step]) -> Vec<String> {
    let mut problems = Vec::new();
    let length = u64::from(run.manifest.round_length.max(1));
    for name in ["vote-s18a", "vote-s18b"] {
        match mixed::step(steps, name) {
            Some(s)
                if s.ok
                    && s.record
                        .pointer("/admitted")
                        .or_else(|| s.record.pointer("/detail/admitted"))
                        .and_then(Value::as_bool)
                        != Some(false) => {}
            Some(_) => problems.push(format!("{name}: the vote was not admitted")),
            None => problems.push(format!("no {name} step")),
        }
    }
    let restarts: Vec<(&Step, &str)> = steps
        .iter()
        .filter_map(|s| s.name.strip_prefix("restart-").map(|node| (s, node)))
        .collect();
    for vote in steps.iter().filter(|s| s.name.starts_with("vote-")) {
        let Some(delegate) = vote.text("delegate") else {
            continue;
        };
        let owner = data
            .validators
            .iter()
            .find(|v| v.keys.values().any(|u| *u == delegate) || v.keys.contains_key(&delegate));
        let (Some(owner), Some(at), Some(h)) = (owner, vote.start, vote.start_height()) else {
            continue;
        };
        for (restart, node) in &restarts {
            let (Some(rt), Some(rh)) = (restart.start, restart.start_height()) else {
                continue;
            };
            if *node == owner.name
                && rt <= at
                && (rh.saturating_sub(1)) / length == h.saturating_sub(1) / length
            {
                problems.push(format!(
                    "{}: a vote for {delegate}, a delegate of {node}, at height {h}, after {node}'s relay restarted in the same round",
                    vote.name
                ));
            }
        }
    }
    problems
}

/// `attest/<node>.json`: the delegates a reference relay lists with the `version` each reports.
fn evidence(run: &ForgingRun<'_>, data: &Data) -> Vec<String> {
    let mut problems = Vec::new();
    let names: Vec<&str> = data
        .validators
        .iter()
        .filter(|v| data.hosted.contains(&v.name))
        .flat_map(|v| v.keys.values().map(String::as_str))
        .collect();
    for n in mixed::references(run.manifest) {
        let path = run.run_dir.join("attest").join(format!("{}.json", n.name));
        let Ok(json) = run::read_json(&path) else {
            problems.push(format!(
                "{}: no attestation evidence ({})",
                n.name,
                path.display()
            ));
            continue;
        };
        let listed: BTreeMap<&str, &str> = json["delegates"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|d| Some((d["username"].as_str()?, d["version"].as_str().unwrap_or(""))))
            .collect();
        for name in &names {
            match listed.get(name) {
                Some(v) if !v.is_empty() => {}
                Some(_) => problems.push(format!(
                    "{}: lists no version for the Heartwood-hosted delegate {name}",
                    n.name
                )),
                None => problems.push(format!(
                    "{}: does not list the Heartwood-hosted delegate {name}",
                    n.name
                )),
            }
        }
    }
    problems
}

/// The window of a partition's minority after the heal in a run that declares no minority (the
/// mixed run): the nodes of every side of the `partition` step but its largest, from the `heal`
/// step's start to `window_seconds` after the end of the `converged` step, as
/// [`run::minority_window`] gives it for a declared one. Until the recipe sees one tip (and
/// restarts a relay still behind 60 s after the heal) the minority's late blocks and its lines are
/// the heal's, not a relay's.
pub(super) fn heal_window(manifest: &Manifest, steps: &[Step]) -> Option<Window> {
    if manifest.minority.is_some() {
        return None;
    }
    let sides = sides(steps.iter().find(|s| s.name == "partition")?)?;
    let largest = sides.iter().map(Vec::len).max()?;
    let first_largest = sides.iter().position(|side| side.len() == largest)?;
    let nodes: Vec<String> = sides
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != first_largest)
        .flat_map(|(_, side)| side.iter().cloned())
        .collect();
    let heal = steps.iter().find(|s| s.name == "heal")?.start?;
    let converged = steps.iter().find(|s| s.name == "converged")?.time("end")?;
    (!nodes.is_empty() && converged >= heal).then(|| Window {
        fault: "minority".to_owned(),
        nodes,
        start: heal,
        end: converged.saturating_add(manifest.window_seconds.saturating_mul(1_000)),
    })
}

/// The sides a `partition` step recorded, as node names.
fn sides(step: &Step) -> Option<Vec<Vec<String>>> {
    let groups = step
        .record
        .get("groups")
        .or_else(|| step.record.pointer("/detail/groups"))?;
    let parsed;
    let groups = match groups {
        Value::String(s) => {
            parsed = serde_json::from_str::<Value>(s).ok()?;
            &parsed
        }
        other => other,
    };
    groups
        .as_array()?
        .iter()
        .map(|g| {
            g.as_array().map(|a| {
                a.iter()
                    .filter_map(|n| n.as_str().map(str::to_owned))
                    .collect()
            })
        })
        .collect()
}

/// A minority relay's bounded interval for counting peers lost in the partition.
struct GraceWindow {
    from: i64,
    until: i64,
    keys: BTreeSet<String>,
    bound: String,
}

impl GraceWindow {
    fn contains(&self, data: &Data, block: &Recorded) -> bool {
        self.keys.contains(&block.key)
            && (self.from..self.until).contains(&data.slot_start(block.slot))
    }
}

fn grace_windows(data: &Data, steps: &[Step], windows: &[Window]) -> BTreeMap<String, GraceWindow> {
    let Some(step) = mixed::step(steps, "partition") else {
        return BTreeMap::new();
    };
    let (Some(groups), Some(from), Some(heal)) = (
        sides(step),
        partition_start(windows),
        mixed::step(steps, "heal").and_then(|s| s.start),
    ) else {
        return BTreeMap::new();
    };
    let cap = from.saturating_add(MAX_GRACE_SLOTS.saturating_mul(data.slots.slot_ms));
    let mut out = BTreeMap::new();
    for side in groups {
        let keys: BTreeSet<String> = data
            .validators
            .iter()
            .filter(|v| side.contains(&v.name))
            .flat_map(|v| v.keys.keys().cloned())
            .collect();
        if keys.len() as u64 >= data.reach {
            continue;
        }
        for node in side {
            let answer = data
                .answers
                .iter()
                .filter(|a| {
                    a.relay == node
                        && a.at >= from
                        && (a.status.is_some_and(|s| s != 0) || a.has_quorum < data.reach)
                })
                .min_by_key(|a| a.at);
            let (until, bound) = match answer {
                Some(a) if a.at <= cap && a.at <= heal => (
                    a.at,
                    "first network-state answer below reach or with nonzero status",
                ),
                _ if heal < cap => (heal, "heal before the four-slot cap"),
                _ => (cap, "four-slot cap; no earlier loss-of-quorum answer"),
            };
            out.insert(
                node,
                GraceWindow {
                    from,
                    until,
                    keys: keys.clone(),
                    bound: format!("{} ({bound})", run::format_time(until)),
                },
            );
        }
    }
    out
}

/// Unjudged minority blocks that may be recorded in each relay's digest for the mixed run only.
pub(super) fn minority_records(
    dir: &Path,
    manifest: &Manifest,
    windows: &[Window],
    steps: &[Step],
) -> Result<BTreeMap<String, super::digest::RecordedBlocks>> {
    if manifest.run != "mixed"
        || manifest.minority.is_some()
        || mixed::step(steps, "partition").is_none()
    {
        return Ok(BTreeMap::new());
    }
    let data = load_records(dir, manifest)?;
    if !data.problems.is_empty() {
        return Err(Error::Usage(format!(
            "cannot establish the minority window: {}",
            data.problems.join("; ")
        )));
    }
    Ok(records_in_windows(
        &data,
        &grace_windows(&data, steps, windows),
    ))
}

fn records_in_windows(
    data: &Data,
    windows: &BTreeMap<String, GraceWindow>,
) -> BTreeMap<String, super::digest::RecordedBlocks> {
    windows
        .iter()
        .map(|(node, window)| {
            let blocks = data
                .blocks
                .values()
                .filter(|b| {
                    window.contains(data, b)
                        && data.holder(&b.key)
                            .and_then(|v| windows.get(&v.name))
                            .is_some_and(|w| w.contains(data, b))
                })
                .map(|b| {
                    let reason = format!(
                        "minority delegate forged during peer loss detection; {node} window ends at {}; judged on the minority's branch in the mixed-forging section",
                        window.bound
                    );
                    ((b.height, b.id.clone()), reason)
                })
                .collect();
            (node.clone(), blocks)
        })
        .collect()
}

/// The label of the minority branch judge's cases.
const BRANCH: &str = "the minority's blocks judged on their branch";

/// Judge every minority block a Heartwood relay of the mixed run applied that no judged chain
/// holds ([`minority_records`]): for each node with a line of such a block, the branches that end
/// at its blocks are built from its own block files ([`branch_ends`]), imported with
/// [`corpora::prepare`] and judged by the oracle with its processor results; each branch's anchors
/// must hold and the reference's processor must accept every block of it; and each node's lines
/// of those blocks must equal the branch's lines ([`digest::judge_branches`]). A run in which no
/// node wrote a line of such a block adds no case and asks no oracle.
fn minority_branches(run: &ForgingRun<'_>, steps: &[Step], cases: &mut Cases) {
    let records = match minority_records(run.run_dir, run.manifest, run.windows, steps) {
        Ok(records) => records,
        Err(e) => {
            cases.fail(SECTION, BRANCH, e.to_string());
            return;
        }
    };
    // The nodes' lines of their recorded blocks.
    let mut wrote: BTreeMap<String, (NodeDigest, RecordedBlocks)> = BTreeMap::new();
    for (node, blocks) in records.iter().filter(|(_, b)| !b.is_empty()) {
        let dir = run.run_dir.join("nodes").join(node).join("difftest");
        let read = match digest::read(&dir) {
            Ok(read) => read,
            Err(e) => {
                cases.fail(SECTION, format!("{node}: {BRANCH}"), e.to_string());
                continue;
            }
        };
        let mine: RecordedBlocks = blocks
            .iter()
            .filter(|((h, id), _)| read.lines.iter().any(|l| l.height() == *h && l.id() == id))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        if !mine.is_empty() {
            wrote.insert(node.clone(), (read, mine));
        }
    }
    let mut branches: Vec<JudgedChain> = Vec::new();
    for (node, (_, mine)) in &wrote {
        let open: RecordedBlocks = mine
            .iter()
            .filter(|((h, id), _)| !branches.iter().any(|b| b.holds(*h, id)))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        if open.is_empty() {
            continue;
        }
        match judge_branches_of(run, node, &open) {
            Ok(judged) => {
                for (branch, problems) in judged {
                    let label = format!("{}: {BRANCH}", branch.chain.name);
                    if problems.is_empty() {
                        // As the final chain's case: the anchors that do not apply are named.
                        let absent = if branch.absent.is_empty() {
                            String::new()
                        } else {
                            format!(
                                "; {} do not apply to a corpus of relay block files",
                                branch.absent.join(" and ")
                            )
                        };
                        cases.pass(
                            SECTION,
                            format!(
                                "{label}: {node}'s block files to height {} imported and judged ({}), the anchors hold and the reference's processor accepted every block{absent}",
                                branch.tip,
                                match (branch.imported, branch.judged) {
                                    (true, true) => "imported and judged now",
                                    (false, true) => "judged now",
                                    (_, false) => "judged before",
                                }
                            ),
                        );
                    }
                    for p in problems {
                        cases.fail(SECTION, label.clone(), p);
                    }
                    branches.push(branch.chain);
                }
            }
            Err(e) => cases.fail(SECTION, format!("{node}: {BRANCH}"), e.to_string()),
        }
    }
    for (node, (read, mine)) in &wrote {
        let judged = digest::judge_branches(read, &branches, mine);
        let label = format!("{node}: its lines of the minority's blocks equal their branch's");
        if judged.problems.is_empty() {
            let counts: Vec<String> = judged
                .compared
                .iter()
                .map(|((chain, step), n)| format!("{n} {step} lines of {chain}"))
                .collect();
            cases.pass(SECTION, format!("{label} ({})", counts.join(", ")));
        }
        verdict_list(cases, &label, &judged.problems);
    }
}

/// `problems` as failed cases of `label`, at most twelve, then a count of the rest.
fn verdict_list(cases: &mut Cases, label: &str, problems: &[String]) {
    for p in problems.iter().take(12) {
        cases.fail(SECTION, label, p.clone());
    }
    if problems.len() > 12 {
        cases.fail(
            SECTION,
            label,
            format!("... and {} more", problems.len() - 12),
        );
    }
}

/// One whole line of a node's block files: its height and id, and its bytes with the newline.
type BlockLine = (u32, String, Vec<u8>);

/// The whole lines of the block files of the node directory `dir`, in the order of its starts
/// (a torn last line dropped, as [`crate::corpus_from_blocks`] drops it).
fn block_lines(dir: &Path) -> Result<Vec<BlockLine>> {
    let records = dir.join("difftest");
    let mut files: Vec<(u32, std::path::PathBuf)> = std::fs::read_dir(&records)
        .map_err(|e| Error::io(&records, e))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter_map(|p| {
            let n = p
                .file_name()?
                .to_str()?
                .strip_prefix("blocks-")?
                .strip_suffix(".bin")?
                .parse()
                .ok()?;
            Some((n, p))
        })
        .collect();
    files.sort();
    let mut out = Vec::new();
    for (_, path) in files {
        let bytes = std::fs::read(&path).map_err(|e| Error::io(&path, e))?;
        for (index, part) in bytes.split_inclusive(|b| *b == b'\n').enumerate() {
            if !part.ends_with(b"\n") {
                continue;
            }
            let v: Value = serde_json::from_slice(part).map_err(|e| Error::Parse {
                path: path.clone(),
                line: index + 1,
                message: e.to_string(),
            })?;
            let (Some(h), Some(id)) = (
                v["h"].as_u64().and_then(|h| u32::try_from(h).ok()),
                v["id"].as_str(),
            ) else {
                return Err(Error::Parse {
                    path: path.clone(),
                    line: index + 1,
                    message: "a block line without its height or id".into(),
                });
            };
            out.push((h, id.to_owned(), part.to_vec()));
        }
    }
    Ok(out)
}

/// The lines (by index into `lines`, a node's block lines in order) that end the branches holding
/// every block of `blocks` the lines name: a block line at or below a height applied before
/// abandons the blocks above it, as the relay's switch to another branch does, so the lines up to
/// a block's line give the chain the relay held when it applied that block. The fewest such ends,
/// latest first, each covering a block no later one holds; in the order of the lines.
pub(super) fn branch_ends(lines: &[(u32, String)], blocks: &RecordedBlocks) -> Vec<usize> {
    let mut chain: BTreeMap<u32, &str> = BTreeMap::new();
    let mut candidates: Vec<(usize, Vec<(u32, String)>)> = Vec::new();
    for (index, (h, id)) in lines.iter().enumerate() {
        let _ = chain.split_off(h);
        chain.insert(*h, id);
        if blocks.contains_key(&(*h, id.clone())) {
            let held = chain
                .iter()
                .filter(|(ch, cid)| blocks.contains_key(&(**ch, (**cid).to_owned())))
                .map(|(ch, cid)| (*ch, (*cid).to_owned()))
                .collect();
            candidates.push((index, held));
        }
    }
    let mut covered: BTreeSet<(u32, String)> = BTreeSet::new();
    let mut ends = Vec::new();
    for (index, held) in candidates.into_iter().rev() {
        if held.iter().any(|b| !covered.contains(b)) {
            covered.extend(held);
            ends.push(index);
        }
    }
    ends.sort_unstable();
    ends
}

/// The name of the corpus of a minority branch: the run's final corpus's (or `live-<run>`), the
/// branch's tip height and the first eight characters of its id.
fn branch_name(manifest: &Manifest, height: u32, id: &str) -> String {
    let base = manifest
        .corpora
        .keys()
        .next()
        .cloned()
        .unwrap_or_else(|| format!("live-{}", manifest.run));
    format!("{base}-minority-{height}-{}", id.get(..8).unwrap_or(id))
}

/// Build, import and judge the branches of `node`'s block files that hold `blocks`, each with the
/// problems of its judgement (its anchors, its tip, the reference's processor results).
fn judge_branches_of(
    run: &ForgingRun<'_>,
    node: &str,
    blocks: &RecordedBlocks,
) -> Result<Vec<(corpora::Judged, Vec<String>)>> {
    let node_dir = run.run_dir.join("nodes").join(node);
    let lines = block_lines(&node_dir)?;
    let keys: Vec<(u32, String)> = lines.iter().map(|(h, id, _)| (*h, id.clone())).collect();
    let ends = branch_ends(&keys, blocks);
    let missing: Vec<String> = blocks
        .keys()
        .filter(|(h, id)| !keys.iter().any(|(kh, kid)| kh == h && kid == id))
        .map(|(h, id)| format!("{id} at height {h}"))
        .collect();
    if !missing.is_empty() {
        return Err(Error::Usage(format!(
            "{node}'s block files do not hold the minority blocks its digest lines name: {}",
            missing.join(", ")
        )));
    }
    let crypto = run::crypto_dir(run.run_dir, &node_dir);
    let mut out = Vec::new();
    for end in ends {
        let (height, id, _) = &lines[end];
        let name = branch_name(run.manifest, *height, id);
        let scratch = ScratchDir::new("minority-branch")?;
        let staged = scratch.path().join(node);
        for dir in ["difftest", "crypto"] {
            let dir = staged.join(dir);
            std::fs::create_dir_all(&dir).map_err(|e| Error::io(&dir, e))?;
        }
        let bytes: Vec<u8> = lines[..=end]
            .iter()
            .flat_map(|(_, _, b)| b.iter().copied())
            .collect();
        let file = staged.join("difftest/blocks-0001.bin");
        std::fs::write(&file, bytes).map_err(|e| Error::io(&file, e))?;
        for entry in std::fs::read_dir(&crypto).map_err(|e| Error::io(&crypto, e))? {
            let path = entry.map_err(|e| Error::io(&crypto, e))?.path();
            if path.extension().is_some_and(|e| e == "json")
                && let Some(file) = path.file_name()
            {
                let target = staged.join("crypto").join(file);
                std::fs::copy(&path, &target).map_err(|e| Error::io(&target, e))?;
            }
        }
        let source = staged
            .to_str()
            .ok_or_else(|| Error::Usage(format!("{}: not a UTF-8 path", staged.display())))?;
        let judged = corpora::prepare(
            run.run_dir,
            &name,
            source,
            &run.manifest.seed,
            run.corpus_root,
            run.oracle,
        )?;
        let problems = branch_problems(&judged, *height, id);
        out.push((judged, problems));
    }
    Ok(out)
}

/// What a judged branch must show: its tip is the minority block it was built to, the judge's
/// anchors hold, and the reference's processor accepted every block of it.
fn branch_problems(judged: &corpora::Judged, height: u32, id: &str) -> Vec<String> {
    let mut problems = Vec::new();
    if judged.tip != height || !judged.chain.holds(height, id) {
        problems.push(format!(
            "the branch ends at height {}, not with the block {id} at height {height}",
            judged.tip
        ));
    }
    if !judged.anchors_ok {
        let failed: Vec<String> = judged
            .anchors
            .get("anchors")
            .and_then(Value::as_object)
            .map(|a| {
                a.iter()
                    .filter(|(_, v)| v.get("ok") != Some(&Value::Bool(true)))
                    .map(|(k, _)| k.clone())
                    .collect()
            })
            .unwrap_or_default();
        problems.push(format!(
            "the judge's anchors do not hold ({}); see {}/anchors.json",
            failed.join(", "),
            judged.dir.display()
        ));
    }
    match run::read_lines(&judged.dir.join(TS_PROCESSOR)) {
        Ok(rows) => {
            let accepted: BTreeSet<u64> = rows
                .iter()
                .filter(|r| r["step"] == "apply" && r["result"] == "Accepted")
                .filter_map(|r| r["h"].as_u64())
                .collect();
            let refused: Vec<String> = rows
                .iter()
                .filter(|r| r.get("result").is_some_and(|x| x != "Accepted"))
                .map(|r| format!("{} at height {}", r["result"], r["h"]))
                .collect();
            let missing: Vec<u64> = (2..=u64::from(judged.tip))
                .filter(|h| !accepted.contains(h))
                .collect();
            if !refused.is_empty() || !missing.is_empty() {
                problems.push(format!(
                    "the reference's processor did not accept every block: {} refused ({}), {} without a result",
                    refused.len(),
                    refused.join(", "),
                    missing.len()
                ));
            }
        }
        Err(e) => problems.push(format!("the processor results: {e}")),
    }
    problems
}

/// During a partition a side with `reach` delegates or more forges, and a side with fewer forges
/// nothing (beyond the slots its relay needs to drop its peers).
fn partition(run: &ForgingRun<'_>, data: &Data, steps: &[Step], cases: &mut Cases) -> Vec<String> {
    let Some(step) = mixed::step(steps, "partition") else {
        return Vec::new();
    };
    let Some(sides) = sides(step) else {
        return vec!["the partition step records no groups".to_owned()];
    };
    let (Some(from), Some(heal)) = (
        partition_start(run.windows),
        mixed::step(steps, "heal").and_then(|s| s.start),
    ) else {
        return vec!["the run records no partition window or no heal".to_owned()];
    };
    let grace = grace_windows(data, steps, run.windows);
    for (node, window) in &grace {
        cases.record(
            SECTION,
            format!("{node}: minority peer loss window ends at {}", window.bound),
        );
    }
    let mut problems = Vec::new();
    for side in &sides {
        let keys: Vec<&String> = data
            .validators
            .iter()
            .filter(|v| side.contains(&v.name))
            .flat_map(|v| v.keys.keys())
            .collect();
        let forged = data
            .blocks
            .values()
            .filter(|b| keys.contains(&&b.key) && (from..heal).contains(&data.slot_start(b.slot)))
            .filter(|b| {
                !data
                    .holder(&b.key)
                    .and_then(|v| grace.get(&v.name))
                    .is_some_and(|w| w.contains(data, b))
            })
            .count();
        let label = side.join(",");
        cases.record(
            SECTION,
            format!("partition side {{{label}}}: {} delegates, {forged} blocks between the partition and the heal", keys.len()),
        );
        let allowed = keys.len() as u64 >= data.reach;
        if allowed && forged == 0 {
            problems.push(format!(
                "side {{{label}}} holds {} delegates, at least the reach {}, and forged no block",
                keys.len(),
                data.reach
            ));
        }
        if !allowed && forged > 0 {
            problems.push(format!("side {{{label}}} holds {} delegates, fewer than the reach {}, and forged {forged} blocks", keys.len(), data.reach));
        }
    }
    problems
}

/// The final tips of every node are one block.
pub(super) fn one_tip(run: &ForgingRun<'_>) -> Vec<String> {
    let Ok(v) = run::read_json(&run.run_dir.join("final.json")) else {
        return vec!["no final.json: the run recorded no final tips".to_owned()];
    };
    let tips = v["tips"].as_object();
    let ids: Vec<&str> = tips
        .into_iter()
        .flatten()
        .map(|(_, t)| t["id"].as_str().unwrap_or(""))
        .collect();
    let mut problems = Vec::new();
    if ids.len() != run.manifest.nodes.len() || ids.iter().any(|i| i.is_empty() || *i != ids[0]) {
        problems.push(format!(
            "the final tips are not one block on all {} nodes",
            run.manifest.nodes.len()
        ));
    }
    if v["ok"] != true {
        problems.push("the tips step did not find one tip".to_owned());
    }
    problems
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::corpus::Corpus;
    use crate::oracle::{Mode, Oracle};
    use crate::report::{CaseResult, Outcome};

    /// A recording of the mixed run whose minority forged a block before its relays dropped the
    /// peers they lost (`tests/fixtures/live/mixed-minority`): the run's manifest with the
    /// devnet's addresses replaced by documentation addresses, its partition, heal and
    /// convergence steps and the partition's fault window, the network's crypto files, and both
    /// Heartwood relays' block files and digest lines to the fork's height and ten blocks after
    /// it, with their network-state answers from two minutes before the partition to two after
    /// the heal. Node 4 applied the minority's block 156, restarted on it and gave it up at the
    /// heal (one apply and one restart line of it); node 5 applied it once (one line). The
    /// branch's corpus as the reference judged it, with its processor results, is beside it.
    fn minority_recording() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/live/mixed-minority")
    }

    /// The minority's block, and the name of its branch's corpus.
    const MINORITY_BLOCK: (u32, &str) = (
        156,
        "35b7213d09ff29109db9d5dfde9ffe37c6dd988ef107014ea61eb51fd59db56d",
    );
    const BRANCH_CORPUS: &str = "live-mixed-minority-156-35b7213d";

    /// The recording copied into a scratch directory, with a corpus root that holds the judged
    /// branch when `judged`.
    struct MinorityRun {
        _scratch: ScratchDir,
        run: std::path::PathBuf,
        root: std::path::PathBuf,
    }

    fn copy_tree(from: &Path, to: &Path) {
        crate::corpus_from_blocks::copy_dir(from, to).expect("the recording copies");
    }

    fn recorded(judged: bool) -> MinorityRun {
        let scratch = ScratchDir::new("minority-branch-test").expect("a scratch directory");
        let run = scratch.path().join("run");
        let root = scratch.path().join("corpus");
        copy_tree(&minority_recording(), &run);
        let _ = std::fs::remove_dir_all(run.join("corpus"));
        std::fs::create_dir_all(&root).expect("a corpus root");
        if judged {
            copy_tree(
                &minority_recording().join("corpus").join(BRANCH_CORPUS),
                &root.join(BRANCH_CORPUS),
            );
        }
        MinorityRun {
            _scratch: scratch,
            run,
            root,
        }
    }

    /// The minority branch judge's cases of the run in `f`, with `oracle`.
    fn branch_cases(f: &MinorityRun, oracle: &dyn Fn() -> Result<Oracle>) -> Vec<CaseResult> {
        let manifest = Manifest::load(&f.run).expect("the manifest loads");
        let steps = run::steps(&f.run).expect("the steps read");
        let mut windows = run::windows(&f.run, manifest.window_seconds).expect("the windows");
        windows.extend(run::minority_window(&manifest, &steps));
        windows.extend(heal_window(&manifest, &steps));
        let forging = ForgingRun {
            run_dir: &f.run,
            manifest: &manifest,
            windows: &windows,
            corpus_root: &f.root,
            oracle,
        };
        let mut cases = Cases {
            run: manifest.run.clone(),
            results: Vec::new(),
        };
        minority_branches(&forging, &steps, &mut cases);
        cases.results
    }

    fn no_oracle() -> Result<Oracle> {
        Err(Error::Usage(
            "the fixture's branch is judged: no oracle".into(),
        ))
    }

    fn failures(cases: &[CaseResult]) -> Vec<String> {
        cases
            .iter()
            .filter(|c| c.outcome.is_failure())
            .map(|c| match &c.outcome {
                Outcome::HarnessError(m) => format!("{}: {m}", c.label),
                other => format!("{}: {other:?}", c.label),
            })
            .collect()
    }

    /// Rewrite `key` of `node`'s digest line of the minority's block with step `step`.
    fn corrupt(f: &MinorityRun, node: &str, step: &str, key: &str) {
        let dir = f.run.join("nodes").join(node).join("difftest");
        let mut done = false;
        for entry in std::fs::read_dir(&dir).expect("the digest directory") {
            let path = entry.expect("an entry").path();
            if !path.to_string_lossy().contains("digest-") {
                continue;
            }
            let text = std::fs::read_to_string(&path).expect("a digest file");
            let mut out = String::new();
            for line in text.lines() {
                let mut v: Value = serde_json::from_str(line).expect("a digest line");
                if v["h"] == MINORITY_BLOCK.0 && v["id"] == MINORITY_BLOCK.1 && v["step"] == step {
                    v[key] = Value::String("0".repeat(64));
                    done = true;
                }
                out.push_str(&format!("{v}\n"));
            }
            std::fs::write(&path, out).expect("the digest file writes");
        }
        assert!(done, "{node} has no {step} line of the minority's block");
    }

    #[test]
    fn the_minority_s_block_is_judged_on_its_branch() {
        let f = recorded(true);
        let cases = branch_cases(&f, &no_oracle);
        assert!(failures(&cases).is_empty(), "{:#?}", failures(&cases));
        let labels: Vec<&str> = cases.iter().map(|c| c.label.as_str()).collect();
        // Node 4's block files make the branch; node 5's line of the same block is judged on it.
        assert!(
            labels.iter().any(|l| l.starts_with(&format!(
                "{BRANCH_CORPUS}: {BRANCH}: node4's block files to height 156 imported and judged (judged before)"
            ))),
            "{labels:#?}"
        );
        // The anchors that do not apply to a corpus of relay block files are named, as the final
        // chain's case names them: the anchors hold without them.
        assert!(
            labels
                .iter()
                .any(|l| l.starts_with(&format!("{BRANCH_CORPUS}: {BRANCH}"))
                    && l.ends_with("; A4 and A5 do not apply to a corpus of relay block files")),
            "{labels:#?}"
        );
        assert!(labels.contains(&format!(
            "node4: its lines of the minority's blocks equal their branch's (1 apply lines of {BRANCH_CORPUS}, 1 restart lines of {BRANCH_CORPUS})"
        ).as_str()), "{labels:#?}");
        assert!(labels.contains(&format!(
            "node5: its lines of the minority's blocks equal their branch's (1 apply lines of {BRANCH_CORPUS})"
        ).as_str()), "{labels:#?}");
        assert_eq!(cases.len(), 3, "{labels:#?}");
    }

    #[test]
    fn a_wrong_state_of_a_minority_block_fails_even_with_one_line() {
        // Node 5 wrote one line of the block: the node's own lines cannot hold it to anything,
        // its branch does.
        for key in ["w", "b", "rw"] {
            let f = recorded(true);
            corrupt(&f, "node5", "apply", key);
            let failed = failures(&branch_cases(&f, &no_oracle));
            assert!(
                failed.iter().any(|p| p.starts_with("node5: its lines")
                    && p.ends_with(&format!(
                        "the apply line of height 156 differs from {BRANCH_CORPUS}'s in {key}"
                    ))),
                "{key}: {failed:#?}"
            );
            assert_eq!(failed.len(), 1, "{key}: {failed:#?}");
        }
        // Node 4's restart line in a state key, and its apply line in a key its restart line does
        // not carry (which the node's own lines cannot catch).
        for (step, key) in [("restart", "s"), ("apply", "m")] {
            let f = recorded(true);
            corrupt(&f, "node4", step, key);
            let failed = failures(&branch_cases(&f, &no_oracle));
            assert!(
                failed.iter().any(|p| p.starts_with("node4: its lines")
                    && p.ends_with(&format!(
                        "the {step} line of height 156 differs from {BRANCH_CORPUS}'s in {key}"
                    ))),
                "{step} {key}: {failed:#?}"
            );
        }
    }

    #[test]
    fn without_a_minority_block_no_branch_is_judged_and_no_oracle_is_asked() {
        let asked = std::cell::Cell::new(0);
        let oracle = || -> Result<Oracle> {
            asked.set(asked.get() + 1);
            no_oracle()
        };
        // No digest line of the block: nothing to judge (the block files still hold it).
        let f = recorded(false);
        for node in ["node4", "node5"] {
            let dir = f.run.join("nodes").join(node).join("difftest");
            for entry in std::fs::read_dir(&dir).expect("the directory") {
                let path = entry.expect("an entry").path();
                if path.to_string_lossy().contains("digest-") {
                    let text = std::fs::read_to_string(&path).expect("a digest file");
                    let kept: String = text
                        .lines()
                        .filter(|l| !l.contains(MINORITY_BLOCK.1))
                        .map(|l| format!("{l}\n"))
                        .collect();
                    std::fs::write(&path, kept).expect("the digest file writes");
                }
            }
        }
        assert!(branch_cases(&f, &oracle).is_empty());
        // A declared minority's run and the other runs record no minority block.
        for (run, minority) in [
            ("mixed-split", true),
            ("mixed-short", false),
            ("mixed", true),
        ] {
            let f = recorded(false);
            let path = f.run.join("run.json");
            let mut manifest = run::read_json(&path).expect("run.json");
            manifest["run"] = Value::String(run.into());
            if minority {
                manifest["minority"] = serde_json::json!({"nodes": ["node4", "node5"], "corpus": "live-mixed-minority"});
            }
            std::fs::write(&path, manifest.to_string()).expect("run.json writes");
            assert!(branch_cases(&f, &oracle).is_empty(), "{run}");
        }
        assert_eq!(asked.get(), 0);
    }

    #[test]
    fn a_branch_that_cannot_be_judged_fails() {
        let f = recorded(false);
        let failed = failures(&branch_cases(&f, &no_oracle));
        assert!(
            failed
                .iter()
                .any(|p| p.starts_with(&format!("node4: {BRANCH}")) && p.contains("no oracle")),
            "{failed:#?}"
        );
        // Without the branch, neither node's line of the block is held by a judged branch.
        for node in ["node4", "node5"] {
            assert!(
                failed
                    .iter()
                    .any(|p| p.starts_with(&format!("{node}: its lines"))
                        && p.contains("which no judged branch holds")),
                "{node}: {failed:#?}"
            );
        }
    }

    #[test]
    fn a_branch_the_reference_did_not_accept_or_whose_anchors_fail_fails() {
        // Rewrite the judged branch's file `file` line by line with `edit` (a line it maps to
        // `None` is dropped), and its manifest again.
        let edited = |file: &str, edit: &dyn Fn(Value) -> Option<Value>| {
            let f = recorded(true);
            let dir = f.root.join(BRANCH_CORPUS);
            let path = dir.join(file);
            let text = std::fs::read_to_string(&path).expect("a judged file");
            let out: String = if file == "anchors.json" {
                let v: Value = serde_json::from_str(&text).expect("the anchors");
                format!("{}\n", edit(v).expect("the anchors"))
            } else {
                text.lines()
                    .filter_map(|l| edit(serde_json::from_str(l).expect("a line")))
                    .map(|v| format!("{v}\n"))
                    .collect()
            };
            std::fs::write(&path, out).expect("the file writes");
            crate::corpus::write_manifest(&dir).expect("the manifest");
            let cases = branch_cases(&f, &no_oracle);
            let branch = format!("{BRANCH_CORPUS}: {BRANCH}");
            assert!(
                !cases
                    .iter()
                    .any(|c| c.label.starts_with(&branch) && !c.outcome.is_failure()),
                "{file}: {cases:#?}"
            );
            failures(&cases)
                .into_iter()
                .filter(|p| p.starts_with(&branch))
                .collect::<Vec<_>>()
        };
        let at = |height: u64, step: &'static str| {
            move |v: &Value| v["h"] == height && v["step"] == step
        };
        // The reference's processor refused the minority's block.
        let refused = edited(TS_PROCESSOR, &|mut v| {
            if at(156, "apply")(&v) {
                v["result"] = Value::String("Rejected".into());
            }
            Some(v)
        });
        assert!(
            refused
                .iter()
                .any(|p| p.contains("1 refused (\"Rejected\" at height 156)")),
            "{refused:#?}"
        );
        // A block below it without a processor result.
        let missing = edited(TS_PROCESSOR, &|v| (!at(100, "apply")(&v)).then_some(v));
        assert!(
            missing
                .iter()
                .any(|p| p.contains("0 refused (), 1 without a result")),
            "{missing:#?}"
        );
        // The judge's round anchor does not hold.
        let anchors = edited("anchors.json", &|mut v| {
            v["ok"] = Value::Bool(false);
            v["anchors"]["A1"]["ok"] = Value::Bool(false);
            Some(v)
        });
        assert!(
            anchors
                .iter()
                .any(|p| p.contains("the judge's anchors do not hold (A1")),
            "{anchors:#?}"
        );
    }

    #[test]
    fn a_branch_ends_at_the_latest_line_that_holds_each_recorded_block() {
        let key = |h: u32, id: &str| (h, id.to_owned());
        let blocks = |list: &[(u32, &str)]| -> RecordedBlocks {
            list.iter()
                .map(|(h, id)| (key(*h, id), String::new()))
                .collect()
        };
        let lines: Vec<(u32, String)> = [
            (2, "a2"),
            (3, "a3"),
            (4, "m4"),
            (5, "m5"),
            (4, "a4"),
            (5, "a5"),
            (6, "a6"),
        ]
        .iter()
        .map(|(h, id)| key(*h, id))
        .collect();
        // Two minority blocks on one branch: one branch, ending at the later.
        assert_eq!(branch_ends(&lines, &blocks(&[(4, "m4"), (5, "m5")])), [3]);
        assert_eq!(branch_ends(&lines, &blocks(&[(4, "m4")])), [2]);
        assert!(branch_ends(&lines, &blocks(&[])).is_empty());
        assert!(branch_ends(&lines, &blocks(&[(4, "x4")])).is_empty());
        // Two branches the node gave up one after the other: one end each.
        let mut two = lines.clone();
        two.extend([key(6, "n6"), key(6, "b6"), key(7, "a7")]);
        assert_eq!(branch_ends(&two, &blocks(&[(4, "m4"), (6, "n6")])), [2, 7]);
    }

    /// The oracle `DIFFTEST_TEST_SOLAR_DIR` or `DIFFTEST_TEST_IMAGE` names, if either.
    fn test_oracle() -> Option<Oracle> {
        if let Ok(dir) = std::env::var("DIFFTEST_TEST_SOLAR_DIR") {
            return Some(Oracle::new(Mode::Local {
                solar_dir: dir.into(),
            }));
        }
        std::env::var("DIFFTEST_TEST_IMAGE")
            .ok()
            .map(|image| Oracle::new(Mode::Container { image }))
    }

    #[test]
    #[ignore = "integration"]
    fn the_reference_judges_the_minority_s_branch_with_its_processor_results() {
        let Some(oracle) = test_oracle() else {
            assert!(
                std::env::var_os("DIFFTEST_TEST_NO_ORACLE").is_some(),
                "no test oracle configured: set DIFFTEST_TEST_IMAGE or DIFFTEST_TEST_SOLAR_DIR, or \
                 DIFFTEST_TEST_NO_ORACLE to skip the test"
            );
            return;
        };
        let f = recorded(false);
        let asked = || Ok(oracle.clone());
        let cases = branch_cases(&f, &asked);
        assert!(failures(&cases).is_empty(), "{:#?}", failures(&cases));
        assert!(
            cases.iter().any(|c| c.label.starts_with(&format!(
                "{BRANCH_CORPUS}: {BRANCH}: node4's block files to height 156 imported and judged (imported and judged now)"
            ))),
            "{cases:#?}"
        );
        // The committed branch is what the reference judges: the same blocks and lines.
        let judged = Corpus::load(&f.root.join(BRANCH_CORPUS)).expect("the judged branch");
        let committed = Corpus::load(&minority_recording().join("corpus").join(BRANCH_CORPUS))
            .expect("the committed branch");
        assert_eq!(
            crate::store::forward_lines(&judged).expect("lines"),
            crate::store::forward_lines(&committed).expect("lines")
        );
        for file in ["blocks.jsonl", TS_PROCESSOR] {
            assert_eq!(
                std::fs::read_to_string(judged.dir.join(file)).expect("a file"),
                std::fs::read_to_string(committed.dir.join(file)).expect("a file"),
                "{file}"
            );
        }
        // A wrong state of node 5's one line fails against the branch the reference judged.
        corrupt(&f, "node5", "apply", "w");
        let failed = failures(&branch_cases(&f, &asked));
        assert!(
            failed.iter().any(|p| p.starts_with("node5: its lines")
                && p.ends_with(&format!("differs from {BRANCH_CORPUS}'s in w"))),
            "{failed:#?}"
        );
    }

    #[test]
    fn an_undeclared_minority_s_convergence_after_the_heal_is_its_window() {
        let at = |text: &str| run::parse_time(text).expect("a time");
        let step = |name: &str, record: Value| Step {
            name: name.to_owned(),
            node: "all".to_owned(),
            ok: true,
            start: record["start"].as_str().and_then(run::parse_time),
            record,
        };
        let steps = vec![
            step(
                "partition",
                serde_json::json!({"start": "2026-10-02T01:05:16.556Z",
                    "groups": [["node1", "node2", "node3"], ["node4", "node5"]]}),
            ),
            step(
                "heal",
                serde_json::json!({"start": "2026-10-02T01:06:52.990Z"}),
            ),
            step(
                "converged",
                serde_json::json!({"start": "2026-10-02T01:06:53.298Z",
                    "end": "2026-10-02T01:08:08.142Z"}),
            ),
        ];
        let mut manifest = Manifest {
            run: "mixed".into(),
            network: "mixed-net".into(),
            dry_run: false,
            seed: "4".into(),
            round_length: 53,
            block_time: 8,
            stop_height: 265,
            control: "node1".into(),
            nodes: Vec::new(),
            corpora: BTreeMap::new(),
            pair: None,
            minority: None,
            api: None,
            api_extra: Vec::new(),
            window_seconds: 30,
            status: "complete".into(),
            relay_image: None,
            relay_binary_sha256: None,
            worktree: None,
        };
        let window = heal_window(&manifest, &steps).expect("a window");
        assert_eq!(window.fault, "minority");
        assert_eq!(window.nodes, ["node4", "node5"]);
        assert_eq!(window.start, at("2026-10-02T01:06:52.990Z"));
        assert_eq!(window.end, at("2026-10-02T01:08:38.142Z"));
        // Node 5's block 165, 37 s after its first arrival elsewhere, falls inside it.
        assert!(window.contains(at("2026-10-02T01:07:45.000Z")) && window.concerns("node5"));
        assert!(!window.concerns("node1"));
        // A declared minority has its own window; a run without a converged step has none.
        manifest.minority = Some((vec!["node4".to_owned()], "live-x".to_owned()));
        assert_eq!(heal_window(&manifest, &steps), None);
        manifest.minority = None;
        assert_eq!(heal_window(&manifest, &steps[..2]), None);
    }

    fn data(reach: u64, has_quorum: u64, no_quorum: u64, status: u64) -> Data {
        let validator = Validator {
            name: "node2".into(),
            forger: "192.0.2.2".into(),
            keys: BTreeMap::from([("k1".to_owned(), "d1".to_owned())]),
        };
        let block = Recorded {
            id: "b".into(),
            height: 10,
            slot: 5,
            key: "k1".into(),
        };
        Data {
            validators: vec![validator],
            hosted: vec!["node2".into()],
            blocks: BTreeMap::from([("b".to_owned(), block)]),
            answers: vec![Answer {
                relay: "node2".into(),
                at: 5_500,
                caller: "192.0.2.2".into(),
                node_height: Some(9),
                has_quorum,
                no_quorum,
                status: Some(status),
                no_response: None,
            }],
            slots: Slots {
                epoch_ms: 0,
                slot_ms: 1_000,
            },
            reach,
            problems: Vec::new(),
        }
    }

    fn partition_steps(from: i64, heal: i64) -> (Vec<Step>, Vec<Window>) {
        let step = |name: &str, at| Step {
            name: name.into(),
            node: "all".into(),
            ok: true,
            start: Some(at),
            record: serde_json::json!({"groups": [["node1"], ["node2", "node3"]]}),
        };
        (
            vec![step("partition", from), step("heal", heal)],
            vec![Window {
                fault: "partition".into(),
                nodes: Vec::new(),
                start: from,
                end: heal,
            }],
        )
    }

    #[test]
    fn the_partition_case_uses_the_forgers_observed_window() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/live/mixed-short");
        let manifest = Manifest::load(&dir).unwrap();
        let (steps, windows) = partition_steps(4_000, 20_000);
        let oracle = || Err(Error::Usage("no oracle needed".into()));
        let run = ForgingRun {
            run_dir: &dir,
            manifest: &manifest,
            windows: &windows,
            corpus_root: &dir,
            oracle: &oracle,
        };
        let mut data = data(2, 0, 0, 1);
        data.validators.push(Validator {
            name: "node1".into(),
            forger: "198.51.100.1".into(),
            keys: BTreeMap::from([
                ("majority".into(), "first".into()),
                ("other".into(), "second".into()),
            ]),
        });
        data.blocks.insert(
            "majority-block".into(),
            Recorded {
                id: "majority-block".into(),
                height: 11,
                slot: 6,
                key: "majority".into(),
            },
        );
        let mut cases = Cases {
            run: "mixed".into(),
            results: Vec::new(),
        };
        assert!(partition(&run, &data, &steps, &mut cases).is_empty());
        data.answers[0].at = 4_999;
        assert!(
            partition(&run, &data, &steps, &mut cases)
                .iter()
                .any(|p| p.contains("forged 1 blocks"))
        );
        data.answers.clear();
        data.blocks.get_mut("b").unwrap().slot = 8;
        assert!(
            partition(&run, &data, &steps, &mut cases)
                .iter()
                .any(|p| p.contains("forged 1 blocks"))
        );
    }

    #[test]
    fn declared_minority_and_other_runs_never_get_unjudged_records() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/live/mixed-short");
        let mut manifest = Manifest::load(&dir).unwrap();
        let (steps, windows) = partition_steps(4_000, 20_000);
        for name in ["mixed-short", "mixed-split", "churn", "core"] {
            manifest.run = name.into();
            assert!(
                minority_records(Path::new("absent-records"), &manifest, &windows, &steps)
                    .unwrap()
                    .is_empty()
            );
        }
        manifest.run = "mixed".into();
        manifest.minority = Some((vec!["node2".into()], "minority".into()));
        assert!(
            minority_records(Path::new("absent-records"), &manifest, &windows, &steps)
                .unwrap()
                .is_empty()
        );
        manifest.minority = None;
        assert!(
            minority_records(Path::new("absent-records"), &manifest, &windows, &steps).is_err()
        );
    }

    #[test]
    fn peer_loss_windows_follow_each_relay_and_stop_at_the_cap_or_heal() {
        let mut data = data(27, 0, 0, 1);
        let (steps, faults) = partition_steps(4_000, 20_000);
        let mut other = data.answers[0].clone();
        other.relay = "node3".into();
        other.at = 7_000;
        data.answers.push(other);
        let windows = grace_windows(&data, &steps, &faults);
        assert_eq!(windows["node2"].until, 5_500);
        assert_eq!(windows["node3"].until, 7_000);
        assert!(windows["node2"].bound.contains("first network-state"));
        assert_eq!(records_in_windows(&data, &windows)["node2"].len(), 1);
        // An answer before the partition does not close the window.
        data.answers[0].at = 3_999;
        let windows = grace_windows(&data, &steps, &faults);
        assert_eq!(windows["node2"].until, 8_000);
        assert!(windows["node2"].bound.contains("four-slot cap"));
        // Quorum loss alone is sufficient, including an answer exactly at the partition.
        data.answers[0].at = 4_000;
        data.answers[0].status = Some(0);
        let windows = grace_windows(&data, &steps, &faults);
        assert!(records_in_windows(&data, &windows)["node2"].is_empty());
        // A nonzero status alone is sufficient, even if the count still reaches the threshold.
        data.answers[0].has_quorum = 53;
        data.answers[0].status = Some(1);
        assert_eq!(grace_windows(&data, &steps, &faults)["node2"].until, 4_000);
        data.answers.clear();
        let (steps, faults) = partition_steps(4_000, 4_500);
        let windows = grace_windows(&data, &steps, &faults);
        assert_eq!(windows["node2"].until, 4_500);
        assert!(windows["node2"].bound.contains("heal"));
        assert!(records_in_windows(&data, &windows)["node2"].is_empty());
    }

    #[test]
    fn the_recorded_partition_timings_allow_only_the_attempt_with_a_minority_block() {
        let time = |s| run::parse_time(s).unwrap();
        // The two recordings put the same minority delegate in the first slot after partition.
        for (from, slot, lost, expected) in [
            (
                "2026-09-30T18:03:26.381Z",
                "2026-09-30T18:03:33.000Z",
                "2026-09-30T18:03:36.534Z",
                1,
            ),
            (
                "2026-09-30T17:03:23.759Z",
                "2026-09-30T17:03:31.000Z",
                "2026-09-30T17:03:26.555Z",
                0,
            ),
        ] {
            let mut data = data(27, 0, 0, 1);
            data.slots = Slots {
                epoch_ms: time(slot),
                slot_ms: 8_000,
            };
            data.answers[0].at = time(lost);
            let block = data.blocks.get_mut("b").unwrap();
            block.height = 156;
            block.slot = 0;
            let (steps, faults) = partition_steps(time(from), time(from) + 96_000);
            let windows = grace_windows(&data, &steps, &faults);
            assert_eq!(records_in_windows(&data, &windows)["node2"].len(), expected);
        }
    }

    #[test]
    fn only_minority_keys_and_slots_strictly_inside_the_window_can_be_recorded() {
        let mut data = data(27, 0, 0, 1);
        let (steps, faults) = partition_steps(4_000, 20_000);
        let windows = grace_windows(&data, &steps, &faults);
        assert_eq!(records_in_windows(&data, &windows)["node2"].len(), 1);
        for slot in [3, 6, 8] {
            data.blocks.get_mut("b").unwrap().slot = slot;
            assert!(records_in_windows(&data, &windows)["node2"].is_empty());
        }
        data.blocks.get_mut("b").unwrap().slot = 5;
        data.blocks.get_mut("b").unwrap().key = "another-side".into();
        assert!(records_in_windows(&data, &windows)["node2"].is_empty());
        data.reach = 1;
        assert!(!grace_windows(&data, &steps, &faults).contains_key("node2"));
    }

    #[test]
    fn a_block_needs_an_answer_on_which_the_reference_would_forge() {
        // A side of 21 delegates that all agree forges; the reference's quorum is then all of them.
        assert!(quorum_before(&data(15, 21, 0, 0)).is_empty());
        assert_eq!(quorum_before(&data(15, 20, 1, 0)).len(), 1);
        // With 27 or more counted, 27 must agree, whatever the run's delegate reach.
        assert!(quorum_before(&data(15, 27, 5, 0)).is_empty());
        assert_eq!(quorum_before(&data(15, 20, 12, 0)).len(), 1);
        assert_eq!(quorum_before(&data(27, 26, 27, 0)).len(), 1);
        // An answer below the minimum delegate reach (status 1) allows no block.
        assert_eq!(quorum_before(&data(15, 14, 0, 1)).len(), 1);
        assert_eq!(quorum_before(&data(15, 0, 0, 0)).len(), 1);
    }
}
