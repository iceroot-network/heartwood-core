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

use std::collections::BTreeMap;

use serde_json::Value;

use super::forging::{self, Answer, QUORUM, Recorded, Slots, Validator};
use super::run::{self, Implementation, Manifest, Step, Window};
use super::{Cases, ForgingRun, mixed, schedule};
use crate::errors::{Error, Result};

const SECTION: &str = "mixed-forging";

/// The slots after a partition starts that a side without the delegates' reach may still forge in:
/// its relay drops the peers it can no longer reach one by one.
const GRACE_SLOTS: i64 = 2;

/// What the checks read of the run: its validators, the blocks the Heartwood relays applied and
/// the chain's slots.
struct Data {
    validators: Vec<Validator>,
    /// The validators whose relay is a Heartwood node.
    hosted: Vec<String>,
    blocks: BTreeMap<String, Recorded>,
    answers: Vec<Answer>,
    slots: Slots,
    reach: u64,
    problems: Vec<String>,
}

impl Data {
    /// The validator that holds the delegate `key`.
    fn holder(&self, key: &str) -> Option<&Validator> {
        self.validators.iter().find(|v| v.keys.contains_key(key))
    }

    /// The start of `slot`, in milliseconds since the Unix epoch.
    fn slot_start(&self, slot: i64) -> i64 {
        self.slots.epoch_ms + slot * self.slots.slot_ms
    }
}

fn load(run: &ForgingRun<'_>) -> Result<Data> {
    let m = run.manifest;
    let mut heartwood = m.clone();
    heartwood
        .nodes
        .retain(|n| n.implementation == Implementation::Heartwood);
    let milestones = forging::milestones(run.run_dir, m)?;
    let epoch_ms = heartwood_crypto::crypto::slots::epoch_unix_ms(milestones.at(1).epoch())
        .ok_or_else(|| Error::Usage("the chain's epoch is not a time".into()))?;
    let (blocks, mut problems) = forging::recorded_blocks(run.run_dir, &heartwood, &milestones)?;
    let (answers, more) = forging::answers(run.run_dir, &heartwood)?;
    problems.extend(more);
    let validators = forging::validators(run.run_dir)?;
    let hosted = validators
        .iter()
        .filter(|v| heartwood.node(&v.name).is_some())
        .map(|v| v.name.clone())
        .collect();
    let json = run::read_json(&run.run_dir.join("run.json"))?;
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
        cases.record(
            SECTION,
            "no Heartwood node in this run (a dry run of the recipe): no block file or record to read",
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
    let sides = partition(run, &data, &steps, cases);
    verdict(
        cases,
        "the partition's sides forged as their delegates' reach allows",
        sides,
    );
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
fn production(run: &ForgingRun<'_>, data: &Data) -> Vec<String> {
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

/// Every block a Heartwood-hosted delegate forged followed, in its slot, an answer of its own
/// relay at the height below it that counted the quorum.
fn quorum_before(data: &Data) -> Vec<String> {
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
                && a.has_quorum >= QUORUM
        });
        if !evidence {
            problems.push(format!(
                "block {} at height {} (slot {}): no answer of {}'s relay in its slot at height {} with hasQuorum at least {QUORUM}",
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
fn counts(run: &ForgingRun<'_>, data: &Data, _steps: &[Step]) -> Vec<String> {
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
    let from = from + GRACE_SLOTS * data.slots.slot_ms;
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
fn one_tip(run: &ForgingRun<'_>) -> Vec<String> {
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
