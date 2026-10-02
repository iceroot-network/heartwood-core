//! What a step checks: each named transaction, digest, projection, count and invariant of a
//! [`Check`] evaluated on the backend as a case of the report. A check that the runner cannot
//! decide by the API (`digest`, `revertSymmetry`, `restartEqual`, which need the nodes' digest
//! lines and the reference's judgement of them) is skipped on a live network, where `live check`
//! judges the run's chain and lines, unless the backend vouches for it by how it works (the
//! lockstep producer compares every line with the reference). `withinBlocks` gives a live check
//! that many blocks to come true.

use std::collections::hash_map::Entry;

use serde_json::Value;

use super::backend::{Backend, BackendError, Tip};
use super::format::{Action, ApiCheck, Check, Invariant};
use super::projection::{self, Names};
use super::runner::Runner;
use crate::compare;
use crate::report::{CaseResult, Outcome};

/// Reads of a projection that a new block may interrupt.
const ATTEMPTS: u32 = 20;

/// What a check is evaluated against.
pub struct Ctx<'r, 'a> {
    /// The run.
    pub runner: &'r mut Runner<'a>,
    /// The step's index.
    pub step: usize,
    /// The step's name.
    pub name: &'r str,
    /// The node the step fired on, the highest tip's.
    pub node: u32,
    /// The projections read, by check label, for the run's record.
    pub values: Vec<(String, Value)>,
}

/// The result of one probe: settled, or a block should come first.
type Probe = Result<Outcome, BackendError>;

/// A backend that cannot do what a check needs skips it; any other failure is the harness's.
fn unfit(e: BackendError) -> Outcome {
    match e {
        BackendError::Unsupported { .. } => Outcome::Skipped(e.to_string()),
        other => Outcome::HarnessError(other.to_string()),
    }
}

fn expected(want: &str, got: impl std::fmt::Display) -> Outcome {
    Outcome::VerdictMismatch {
        oracle: want.to_owned(),
        heartwood: got.to_string(),
    }
}

fn refused(answer: &super::backend::Submitted) -> String {
    match &answer.code {
        Some(code) => format!("refused: {code}"),
        None => "refused".to_owned(),
    }
}

impl Ctx<'_, '_> {
    fn backend(&self) -> &dyn Backend {
        &*self.runner.backend
    }

    fn case(&self, label: String, height: u32, outcome: Outcome) -> CaseResult {
        CaseResult {
            file: self.runner.compiled.scenario.id.clone(),
            line: self.step + 1,
            op: "scenario.check".into(),
            label: format!("{}: {label}", self.name),
            height: u64::from(height),
            outcome,
        }
    }

    /// Probe until it holds or `within` blocks have come.
    fn within(&mut self, within: u32, probe: impl Fn(&mut Self) -> Probe) -> (u32, Outcome) {
        let start = self.runner.observe().map_or(0, |(_, tip)| tip.height);
        loop {
            let outcome = probe(self).unwrap_or_else(unfit);
            let now = self.runner.observe().map_or(start, |(_, tip)| tip.height);
            if !outcome.is_failure() || now >= start + within {
                return (now, outcome);
            }
            if let Err(e) = self.runner.backend.wait() {
                return (now, Outcome::HarnessError(e.to_string()));
            }
        }
    }

    fn submitted(&self, reference: &str) -> Result<super::backend::Submitted, Outcome> {
        self.runner.answers.get(reference).cloned().ok_or_else(|| {
            Outcome::HarnessError(format!("{reference} was not submitted before this check"))
        })
    }

    fn id_of(&self, reference: &str) -> Result<String, Outcome> {
        self.runner
            .compiled
            .transactions
            .iter()
            .find(|t| t.reference == reference)
            .map(|t| t.id.clone())
            .ok_or_else(|| {
                Outcome::HarnessError(format!("the scenario has no transaction {reference}"))
            })
    }

    /// Whether the chain holds `reference`, on the node with the highest tip.
    fn forged(&self, reference: &str) -> Probe {
        let id = match self.id_of(reference) {
            Ok(id) => id,
            Err(outcome) => return Ok(outcome),
        };
        let (node, _) = self.runner.observe()?;
        Ok(if self.backend().forged(node, &id)? {
            Outcome::Pass
        } else {
            expected("in the chain", "in no block")
        })
    }

    /// A projection of the API, equal on the nodes given, read while every one of them stood at
    /// the same height; a read that a new block interrupted, or that found the nodes at different
    /// heights, is read again after the network had time to move on. The value read is kept for
    /// the run's record.
    fn projection(&mut self, api: &ApiCheck) -> Probe {
        let nodes: Vec<u32> = if api.nodes.is_empty() {
            (1..=self.backend().nodes()).collect()
        } else {
            api.nodes.clone()
        };
        for attempt in 0..ATTEMPTS {
            if attempt > 0 {
                self.runner.backend.wait()?;
            }
            if let Some(outcome) = self.projection_once(api, &nodes)? {
                return Ok(outcome);
            }
        }
        Ok(Outcome::Skipped(
            "the nodes never stood at one height while the answers were read".into(),
        ))
    }

    /// One read of `api` on `nodes`: `None` when the tips moved or differed.
    fn projection_once(
        &mut self,
        api: &ApiCheck,
        nodes: &[u32],
    ) -> Result<Option<Outcome>, BackendError> {
        let names = Names {
            chain: &self.runner.chain,
            actors: self
                .runner
                .compiled
                .scenario
                .actors
                .keys()
                .map(String::as_str)
                .collect(),
            transactions: &self.runner.compiled.transactions,
        };
        let batch = self
            .runner
            .compiled
            .scenario
            .steps
            .iter()
            .enumerate()
            .take(self.step.saturating_add(1))
            .rfind(|(_, step)| step.action == Some(Action::Submit))
            .map(|(index, _)| index);
        let required = projection::pool_transactions(&names, &api.projection, batch, self.step)?;
        let mut height = None;
        let mut first: Option<Value> = None;
        let mut outcome = Outcome::Pass;
        for &node in nodes {
            let before = self.runner.read_tip(node)?;
            let value = projection::read(self.backend(), &names, node, &api.projection)?;
            let after = self.runner.read_tip(node)?;
            if after != before || height.is_some_and(|h| h != before.height) {
                return Ok(None);
            }
            height = Some(before.height);
            if let Some(required) = &required {
                let missing: Vec<_> = required
                    .iter()
                    .filter(|tx| {
                        !value.as_array().is_some_and(|ids| {
                            ids.iter().any(|id| id.as_str() == Some(tx.id.as_str()))
                        })
                    })
                    .map(|tx| tx.reference.as_str())
                    .collect();
                if !missing.is_empty() && outcome == Outcome::Pass {
                    outcome = expected(
                        "the submitted batch in every pool",
                        format!("node {node} is missing {}", missing.join(", ")),
                    );
                }
            }
            match &first {
                None => first = Some(value),
                Some(expected) => {
                    let differences = compare::diff(expected, &value);
                    if !differences.is_empty() && outcome == Outcome::Pass {
                        outcome = Outcome::Mismatch(differences);
                    }
                }
            }
        }
        if let Some(value) = first {
            let label = format!("{}: api {}", self.name, api.projection);
            match self.values.iter_mut().find(|(l, _)| *l == label) {
                Some(kept) => kept.1 = value,
                None => self.values.push((label, value)),
            }
        }
        Ok(Some(outcome))
    }

    /// The missed slots recorded for the delegates of `node`'s forger (all of them when the
    /// backend does not know the delegates).
    fn missed(&self, node: u32) -> Result<u64, BackendError> {
        let total = |path: String| -> Result<u64, BackendError> {
            let answer = self.backend().get(node, &path)?;
            match answer.body["meta"]["totalCount"].as_u64() {
                Some(count) if answer.status == 200 => Ok(count),
                _ => Err(BackendError::Failed {
                    action: "count the missed slots",
                    message: format!("{path} answers {} without a count", answer.status),
                }),
            }
        };
        let delegates = self.backend().forger_delegates(node)?;
        if delegates.is_empty() {
            return total("/api/blocks/missed?limit=1".to_owned());
        }
        delegates.iter().try_fold(0, |sum, d| {
            Ok(sum + total(format!("/api/delegates/{d}/blocks/missed?limit=1"))?)
        })
    }

    /// Compare every expected relay, recording both the answering and exempt nodes.
    fn single_tip(&mut self) -> Probe {
        let mut tips = Vec::new();
        let mut missing = Vec::new();
        let mut exempt = Vec::new();
        for node in 1..=self.backend().nodes() {
            let tip = self.runner.read_tip(node);
            if let Some(reason) = self.runner.tip_exemption(node) {
                exempt.push(format!("node {node} ({reason})"));
                continue;
            }
            match tip {
                Ok(tip) => tips.push((node, tip)),
                Err(error) => missing.push(format!("node {node}: {error}")),
            }
        }
        let compared: Vec<_> = tips
            .iter()
            .map(|(node, _)| format!("node {node}"))
            .collect();
        self.runner.single_tip_label = format!(
            "compared [{}]; exempt [{}]; missing [{}]",
            compared.join(", "),
            exempt.join(", "),
            missing.join(", ")
        );
        Ok(match tips.split_first() {
            Some(((_, first), rest))
                if missing.is_empty() && rest.iter().all(|(_, tip)| tip == first) =>
            {
                Outcome::Pass
            }
            _ => expected("one tip", format!("{tips:?}; missing {missing:?}")),
        })
    }

    /// Node `node`'s block at `height`: its id, read from a block object the API answered with a
    /// 200 at that height, and its generator's username; why not, when the answer is not one.
    fn block_at(
        &self,
        node: u32,
        height: u32,
    ) -> Result<Result<(String, Value), String>, BackendError> {
        let answer = self.backend().get(node, &format!("/api/blocks/{height}"))?;
        let block = &answer.body["data"];
        Ok(
            match block["id"].as_str().filter(|_| {
                answer.status == 200 && block["height"].as_u64() == Some(u64::from(height))
            }) {
                Some(id) => Ok((id.to_owned(), block["generator"]["username"].clone())),
                None => Err(format!(
                    "node {node} answered {} without its block at height {height}",
                    answer.status
                )),
            },
        )
    }

    /// Each group of the partition forged a branch of its own: at one height above the
    /// partition's, the height of the lowest group's highest tip, every group's highest node holds
    /// a block of its own (no two groups the same block), forged by a delegate of a node of its
    /// group. Those blocks are kept for the fork-recovery check.
    fn both_sides_forge(&mut self) -> Probe {
        let Some((groups, height)) = self.runner.partition.clone() else {
            return Ok(Outcome::HarnessError(
                "no partition was made before this check".into(),
            ));
        };
        let mut best = Vec::new();
        for group in &groups {
            let found = group
                .iter()
                .filter_map(|node| self.runner.read_tip(*node).ok().map(|tip| (*node, tip)))
                .max_by_key(|(_, tip)| tip.height);
            let Some(found) = found else {
                return Ok(expected(
                    "a tip of every group",
                    format!("no node of {group:?} answers"),
                ));
            };
            best.push(found);
        }
        let shared = best.iter().map(|(_, tip)| tip.height).min().unwrap_or(0);
        if shared <= height {
            return Ok(expected(
                &format!("every group's highest tip above the partition's height {height}"),
                format!("{best:?}"),
            ));
        }
        let delegates = self.backend().delegates_by_node();
        if delegates.is_empty() {
            return Ok(Outcome::Skipped(
                "the backend does not name its nodes' delegates, so no branch's generator can be \
                 placed in its group"
                    .into(),
            ));
        }
        let mut blocks: Vec<Tip> = Vec::new();
        for ((node, _), group) in best.iter().zip(&groups) {
            let (id, generator) = match self.block_at(*node, shared)? {
                Ok(block) => block,
                Err(why) => return Ok(expected(&format!("a block at height {shared}"), why)),
            };
            let own = group
                .iter()
                .flat_map(|n| delegates.get(n).into_iter().flatten())
                .any(|d| generator.as_str() == Some(d.as_str()));
            if !own {
                return Ok(expected(
                    &format!(
                        "node {node}'s block at height {shared} forged by a delegate of {group:?}"
                    ),
                    format!("{id} forged by {generator}"),
                ));
            }
            blocks.push(Tip { height: shared, id });
        }
        let ids: std::collections::BTreeSet<&str> = blocks.iter().map(|b| b.id.as_str()).collect();
        if ids.len() == blocks.len() {
            self.runner.branches = blocks;
            Ok(Outcome::Pass)
        } else {
            Ok(expected(
                &format!("each group on a branch of its own at height {shared}"),
                format!("{blocks:?}"),
            ))
        }
    }

    /// After the heal, every node is on one tip, at or above the branches' height, and the joined
    /// chain's block there (a 200 with a block object of that height) is not every branch's: a
    /// branch was given up.
    fn fork_recovered(&mut self) -> Probe {
        if self.runner.branches.is_empty() {
            return Ok(Outcome::HarnessError(
                "no branch was seen: a both-sides check during the partition comes first".into(),
            ));
        }
        let one = self.single_tip()?;
        if one != Outcome::Pass {
            return Ok(one);
        }
        let (node, tip) = self.runner.observe()?;
        let mut replaced = Vec::new();
        for branch in self.runner.branches.clone() {
            if tip.height < branch.height {
                return Ok(expected(
                    &format!(
                        "the joined chain at or above the branches' height {}",
                        branch.height
                    ),
                    format!("its tip at {}", tip.height),
                ));
            }
            let id = match self.block_at(node, branch.height)? {
                Ok((id, _)) => id,
                Err(why) => {
                    return Ok(expected(
                        &format!("the joined chain's block at height {}", branch.height),
                        why,
                    ));
                }
            };
            if id != branch.id {
                replaced.push(branch.id);
            }
        }
        Ok(if replaced.is_empty() {
            expected(
                "a branch's block replaced in the joined chain",
                "every branch's block is still in it",
            )
        } else {
            Outcome::Pass
        })
    }

    fn invariant(&mut self, which: Invariant) -> Probe {
        let name = match which {
            Invariant::SingleTip => return self.single_tip(),
            Invariant::BothSidesForge => return self.both_sides_forge(),
            Invariant::ForkRecovered => return self.fork_recovered(),
            Invariant::RevertSymmetry => "revertSymmetry",
            Invariant::RestartEqual => "restartEqual",
            Invariant::Supply => "supply",
            Invariant::Schedule => "schedule",
            Invariant::SecondaryReward => "secondaryReward",
        };
        if let Some(said) = self.backend().verified(name) {
            return Ok(said.map_or_else(|why| expected("it holds", why), |()| Outcome::Pass));
        }
        match which {
            Invariant::Supply => self.supply(),
            Invariant::Schedule => self.schedule(),
            Invariant::SecondaryReward => self.secondary_reward(),
            _ => Ok(Outcome::Skipped(
                "a live network's revert and restart lines are judged by live check".into(),
            )),
        }
    }

    /// The blocks of the chain, in height order, from the API.
    fn blocks(&self, node: u32, to: u32) -> Result<Vec<Value>, BackendError> {
        let mut out = Vec::new();
        for page in 1.. {
            let body = self
                .backend()
                .get(
                    node,
                    &format!("/api/blocks?limit=100&page={page}&orderBy=height:asc"),
                )?
                .body;
            let list = body["data"].as_array().cloned().unwrap_or_default();
            if list.is_empty() {
                break;
            }
            out.extend(list);
            if out
                .last()
                .and_then(|b| b["height"].as_u64())
                .is_some_and(|h| h >= u64::from(to))
            {
                break;
            }
        }
        out.retain(|b| b["height"].as_u64().is_some_and(|h| h <= u64::from(to)));
        Ok(out)
    }

    /// The supply is the genesis supply plus the rewards less the burns.
    fn supply(&mut self) -> Probe {
        let (node, tip) = self.runner.observe()?;
        let blockchain = self.backend().get(node, "/api/blockchain")?.body;
        let blocks = self.blocks(node, tip.height)?;
        let amount = |v: &Value| {
            v.as_str()
                .map(str::to_owned)
                .or_else(|| v.as_u64().map(|n| n.to_string()))
                .and_then(|t| t.parse::<i128>().ok())
        };
        let (Some(supply), Some(burned)) = (
            amount(&blockchain["data"]["supply"]),
            amount(&blockchain["data"]["burned"]["total"]),
        ) else {
            return Ok(Outcome::HarnessError(
                "/api/blockchain names no supply and no burned total".into(),
            ));
        };
        let genesis: i128 = self.runner.chain.supply.parse().unwrap_or(0);
        let mut rewards = 0;
        for block in &blocks {
            let Some(reward) = amount(&block["forged"]["reward"]) else {
                return Ok(Outcome::HarnessError(
                    "a block of /api/blocks names no forged.reward".into(),
                ));
            };
            rewards += reward;
        }
        let now = self.backend().tip(node)?;
        if now != tip
            || blockchain["data"]["block"]["height"].as_u64() != Some(u64::from(tip.height))
        {
            return Ok(Outcome::Skipped(
                "the tip moved on while the supply was read".into(),
            ));
        }
        let want = genesis + rewards - burned;
        Ok(if want == supply {
            Outcome::Pass
        } else {
            expected(&want.to_string(), supply)
        })
    }

    /// Round membership: every block was forged by an active delegate of its round. A delegate
    /// may forge twice in a round: after a missed slot the round's later slots fall to delegates
    /// that have forged in it already, at the milestone's `secondaryReward`. That each block's
    /// forger is the one its slot names is the reference's processing's to judge: a suite run's
    /// chain is judged by it when the suite's runs are compared ([`super::judge`]).
    fn schedule(&mut self) -> Probe {
        let (node, tip) = self.runner.observe()?;
        let length = self.runner.chain.round_length.max(1);
        let mut rounds: std::collections::HashMap<u32, Vec<String>> =
            std::collections::HashMap::new();
        for block in self.blocks(node, tip.height)? {
            let height = block["height"]
                .as_u64()
                .and_then(|h| u32::try_from(h).ok())
                .unwrap_or(0);
            if height < 2 {
                continue;
            }
            let round = height.div_ceil(length);
            let generator = block["generator"]["publicKey"]
                .as_str()
                .or_else(|| block["generatorPublicKey"].as_str())
                .unwrap_or_default()
                .to_owned();
            let keys = match rounds.entry(round) {
                Entry::Occupied(known) => known.into_mut(),
                Entry::Vacant(free) => {
                    let body = self
                        .backend()
                        .get(node, &format!("/api/rounds/{round}/delegates"))?
                        .body;
                    let keys = body["data"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|d| d["publicKey"].as_str())
                        .map(str::to_owned)
                        .collect();
                    free.insert(keys)
                }
            };
            if !keys.contains(&generator) {
                return Ok(expected(
                    &format!("a delegate of round {round}"),
                    format!("{generator} at {height}"),
                ));
            }
        }
        Ok(Outcome::Pass)
    }

    /// A delegate's second block of a round carries the milestone's `secondaryReward`, as the
    /// reference's round state gives it (`getRewardForBlockInRound`): where the milestone of the
    /// block's height enables dynamic rewards, a block whose generator forged an earlier block of
    /// the same round is rewarded with `secondaryReward`. Its first block's reward follows its
    /// rank, which the reference's processing judges. A chain with no second block of a delegate
    /// in a round compares nothing, and says so.
    fn secondary_reward(&mut self) -> Probe {
        let (node, tip) = self.runner.observe()?;
        let length = self.runner.chain.round_length.max(1);
        let milestones = self.runner.chain.crypto["milestones"].clone();
        let mut forged: std::collections::HashMap<u32, Vec<String>> =
            std::collections::HashMap::new();
        let (mut compared, mut wrong) = (0_usize, Vec::new());
        for block in self.blocks(node, tip.height)? {
            let height = block["height"]
                .as_u64()
                .and_then(|h| u32::try_from(h).ok())
                .unwrap_or(0);
            if height < 2 {
                continue;
            }
            let Some(generator) = block["generator"]["publicKey"]
                .as_str()
                .or_else(|| block["generatorPublicKey"].as_str())
                .or_else(|| block["generator"]["username"].as_str())
                .filter(|g| !g.is_empty())
            else {
                return Ok(Outcome::HarnessError(format!(
                    "the block at height {height} of /api/blocks names no generator"
                )));
            };
            let round = height.div_ceil(length);
            let earlier = forged.entry(round).or_default();
            let second = earlier.iter().any(|g| g == generator);
            earlier.push(generator.to_owned());
            let Some(want) = secondary_reward_at(&milestones, height).filter(|_| second) else {
                continue;
            };
            compared += 1;
            if amount(&block["forged"]["reward"]) != Some(want) {
                wrong.push(format!(
                    "{generator}'s second block of round {round}, at {height}: reward {}, not {want}",
                    block["forged"]["reward"]
                ));
            }
        }
        Ok(if !wrong.is_empty() {
            expected(
                "the secondaryReward for a delegate's second block of a round",
                wrong.join("; "),
            )
        } else if compared == 0 {
            Outcome::Skipped(format!(
                "no delegate forged a second block of a round up to height {}, so no \
                 secondaryReward was compared",
                tip.height
            ))
        } else {
            Outcome::Pass
        })
    }
}

/// An amount the API gives as a decimal string or a number.
fn amount(value: &Value) -> Option<u128> {
    value
        .as_str()
        .map(str::to_owned)
        .or_else(|| value.as_u64().map(|n| n.to_string()))
        .and_then(|t| t.parse().ok())
}

/// The `secondaryReward` of the milestone at `height` when it enables dynamic rewards: the
/// milestones up to that height merged in order, as the reference's configuration manager merges
/// them, each later key over an earlier one.
fn secondary_reward_at(milestones: &Value, height: u32) -> Option<u128> {
    let mut merged = Value::Null;
    for milestone in milestones.as_array().into_iter().flatten() {
        if milestone["height"]
            .as_u64()
            .is_some_and(|h| h <= u64::from(height))
        {
            merge(&mut merged, &milestone["dynamicReward"]);
        }
    }
    (merged["enabled"] == Value::Bool(true))
        .then(|| amount(&merged["secondaryReward"]))
        .flatten()
}

/// `over` merged into `into`: objects key by key, anything else replacing what was there.
fn merge(into: &mut Value, over: &Value) {
    match (into, over) {
        (_, Value::Null) => {}
        (Value::Object(into), Value::Object(over)) => {
            for (key, value) in over {
                merge(into.entry(key.clone()).or_insert(Value::Null), value);
            }
        }
        (into, over) => *into = over.clone(),
    }
}

/// Every part of `check`, evaluated, as cases.
pub fn evaluate(ctx: &mut Ctx<'_, '_>, check: &Check) -> Vec<CaseResult> {
    let within = check.within_blocks.unwrap_or(0);
    let mut cases = Vec::new();
    let height = |ctx: &Ctx<'_, '_>| ctx.runner.observe().map_or(0, |(_, tip)| tip.height);
    for reference in &check.accept {
        let outcome = match ctx.submitted(reference) {
            Ok(a) if a.accepted => Outcome::Pass,
            Ok(a) => expected("accepted", refused(&a)),
            Err(outcome) => outcome,
        };
        cases.push(ctx.case(format!("accept {reference}"), height(ctx), outcome));
    }
    for reference in &check.invalid {
        let outcome = match ctx.submitted(reference) {
            Ok(a) if !a.accepted => Outcome::Pass,
            Ok(_) => expected("refused", "accepted"),
            Err(outcome) => outcome,
        };
        cases.push(ctx.case(format!("invalid {reference}"), height(ctx), outcome));
    }
    for (reference, code) in &check.codes {
        let outcome = match ctx.submitted(reference) {
            Ok(a) if a.code.as_deref() == Some(code) => Outcome::Pass,
            Ok(a) => expected(
                code,
                if a.accepted {
                    "accepted".to_owned()
                } else {
                    refused(&a)
                },
            ),
            Err(outcome) => outcome,
        };
        cases.push(ctx.case(format!("code {reference}"), height(ctx), outcome));
    }
    for reference in &check.forged {
        let (h, outcome) = ctx.within(within, |c| c.forged(reference));
        cases.push(ctx.case(format!("forged {reference}"), h, outcome));
    }
    for reference in &check.absent {
        // A transaction stays absent through the whole window: it is looked for at its end.
        let target = height(ctx) + within;
        let outcome = match ctx.runner.wait_blocks(target) {
            Ok(()) => ctx.forged(reference).map(|o| match o {
                Outcome::Pass => expected("in no block", "in the chain"),
                Outcome::VerdictMismatch { .. } => Outcome::Pass,
                other => other,
            }),
            Err(e) => Err(e),
        }
        .unwrap_or_else(unfit);
        cases.push(ctx.case(format!("absent {reference}"), height(ctx), outcome));
    }
    if check.digest {
        let outcome = match ctx.backend().verified("digest") {
            Some(said) => {
                said.map_or_else(|why| expected("digests agree", why), |()| Outcome::Pass)
            }
            None => Outcome::Skipped(
                "a live network's digests are judged against the reference when the suite's runs are compared".into(),
            ),
        };
        cases.push(ctx.case("digest".into(), height(ctx), outcome));
    }
    if let Some(api) = &check.api {
        let (h, outcome) = ctx.within(within, |c| c.projection(api));
        cases.push(ctx.case(format!("api {}", api.projection), h, outcome));
    }
    if let Some(missed) = &check.missed {
        let (h, outcome) = ctx.within(within, |c| {
            let total = c.missed(missed.node)?;
            Ok(if total >= u64::from(missed.at_least) {
                Outcome::Pass
            } else {
                expected(&format!("at least {} missed", missed.at_least), total)
            })
        });
        cases.push(ctx.case(format!("missed of node {}", missed.node), h, outcome));
    }
    for which in &check.invariant {
        let (h, outcome) = ctx.within(within, |c| c.invariant(*which));
        let mut label = format!("invariant {which:?}").to_lowercase();
        if *which == Invariant::SingleTip {
            label.push_str(&format!("; {}", ctx.runner.single_tip_label));
        }
        cases.push(ctx.case(label, h, outcome));
    }
    cases
}
