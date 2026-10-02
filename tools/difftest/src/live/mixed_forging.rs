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

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde_json::Value;

use super::forging::{self, Answer, QUORUM, Recorded, Slots, Validator};
use super::run::{self, Implementation, Manifest, Step, Window};
use super::{Cases, ForgingRun, mixed, schedule};
use crate::errors::{Error, Result};

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
                        "minority delegate forged during peer loss detection; {node} window ends at {}",
                        window.bound
                    );
                    ((b.height, b.id.clone()), reason)
                })
                .collect();
            (node.clone(), blocks)
        })
        .collect()
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
            network: "b8-mixed".into(),
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
