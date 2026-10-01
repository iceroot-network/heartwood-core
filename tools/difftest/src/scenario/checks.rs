//! What a step checks: each named transaction, digest, projection, count and invariant of a
//! [`Check`] evaluated on the backend as a case of the report. A check that the runner cannot
//! decide by the API (`digest`, `revertSymmetry`, `restartEqual`, which need the nodes' digest
//! lines and the reference's judgement of them) is skipped on a live network, where `live check`
//! judges the run's chain and lines, unless the backend vouches for it by how it works (the
//! lockstep producer compares every line with the reference). `withinBlocks` gives a live check
//! that many blocks to come true.

use std::collections::hash_map::Entry;

use serde_json::Value;

use super::backend::{Backend, BackendError};
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

    fn invariant(&mut self, which: Invariant) -> Probe {
        if which == Invariant::SingleTip {
            return self.single_tip();
        }
        let name = match which {
            Invariant::SingleTip => "singleTip",
            Invariant::RevertSymmetry => "revertSymmetry",
            Invariant::RestartEqual => "restartEqual",
            Invariant::Supply => "supply",
            Invariant::Schedule => "schedule",
        };
        if let Some(said) = self.backend().verified(name) {
            return Ok(said.map_or_else(|why| expected("it holds", why), |()| Outcome::Pass));
        }
        match which {
            Invariant::SingleTip => self.single_tip(),
            Invariant::Supply => self.supply(),
            Invariant::Schedule => self.schedule(),
            Invariant::RevertSymmetry | Invariant::RestartEqual => Ok(Outcome::Skipped(
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

    /// Every block was forged by an active delegate of its round. A delegate may forge twice in
    /// a round: after a missed slot the reference gives the round's later slots to delegates that
    /// have forged in it already, at the milestone's `secondaryReward`.
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
                "a live network's digests are judged against the reference by live check".into(),
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
