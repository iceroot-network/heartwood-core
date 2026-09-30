//! What a step checks: each named transaction, digest, projection, count and invariant of a
//! [`Check`] evaluated on the backend as a case of the report. A check that a network of one
//! implementation cannot decide (`digest`, `revertSymmetry`, `restartEqual`, which compare two
//! runs) is skipped there, unless the backend vouches for it by how it works; the suites' two
//! runs are compared by `scenario compare`. `withinBlocks` gives a live check that many blocks to
//! come true.

use std::collections::hash_map::Entry;

use serde_json::{Value, json};

use super::backend::{Backend, BackendError};
use super::format::{ApiCheck, Check, Invariant};
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

    /// A projection of the API, equal on the nodes given, read at tips that stood still; a read
    /// that a new block interrupted is read again.
    fn projection(&mut self, api: &ApiCheck) -> Probe {
        let nodes: Vec<u32> = if api.nodes.is_empty() {
            (1..=self.backend().nodes()).collect()
        } else {
            api.nodes.clone()
        };
        'attempt: for _ in 0..ATTEMPTS {
            let mut first: Option<Value> = None;
            for &node in &nodes {
                let before = self.backend().tip(node)?;
                let value = self.project(node, &api.projection)?;
                if self.backend().tip(node)? != before {
                    continue 'attempt;
                }
                match &first {
                    None => first = Some(value),
                    Some(expected) => {
                        let differences = compare::diff(expected, &value);
                        if !differences.is_empty() {
                            return Ok(Outcome::Mismatch(differences));
                        }
                    }
                }
            }
            return Ok(Outcome::Pass);
        }
        Ok(Outcome::Skipped(
            "the tips kept moving while the answers were read".into(),
        ))
    }

    fn project(&self, node: u32, name: &str) -> Result<Value, BackendError> {
        let get = |path: String| -> Result<Value, BackendError> {
            Ok(self.backend().get(node, &path)?.body)
        };
        let failed = |message: String| BackendError::Failed {
            action: "project the API",
            message,
        };
        match name {
            "wallets" => {
                let mut out = serde_json::Map::new();
                let mut names: Vec<&String> = self.runner.compiled.scenario.actors.keys().collect();
                names.sort();
                let genesis = "genesis".to_owned();
                for actor in std::iter::once(&genesis).chain(names) {
                    let address = &self
                        .runner
                        .chain
                        .person(actor)
                        .map_err(|e| failed(e.to_string()))?
                        .address;
                    let body = get(format!("/api/wallets/{address}"))?;
                    let data = &body["data"];
                    out.insert(
                        actor.clone(),
                        json!({"balance": data["balance"], "nonce": data["nonce"]}),
                    );
                }
                Ok(Value::Object(out))
            }
            "delegates" => {
                let body = get("/api/delegates?limit=100".to_owned())?;
                let list = body["data"].as_array().cloned().unwrap_or_default();
                Ok(Value::Object(
                    list.iter()
                        .filter_map(|d| {
                            Some((
                                d["username"].as_str()?.to_owned(),
                                json!({"rank": d["rank"], "votes": d["votes"]}),
                            ))
                        })
                        .collect(),
                ))
            }
            "supply" => {
                let body = get("/api/blockchain".to_owned())?;
                Ok(json!({"supply": body["data"]["supply"], "burned": body["data"]["burned"]}))
            }
            other => Err(failed(format!(
                "no projection is named {other}: wallets, delegates or supply"
            ))),
        }
    }

    /// The missed slots recorded for the delegates of `node`'s forger (all of them when the
    /// backend does not know the delegates).
    fn missed(&self, node: u32) -> Result<u64, BackendError> {
        let total = |path: String| -> Result<u64, BackendError> {
            Ok(self.backend().get(node, &path)?.body["meta"]["totalCount"]
                .as_u64()
                .unwrap_or(0))
        };
        let delegates = self.backend().forger_delegates(node)?;
        if delegates.is_empty() {
            return total("/api/blocks/missed?limit=1".to_owned());
        }
        delegates.iter().try_fold(0, |sum, d| {
            Ok(sum + total(format!("/api/delegates/{d}/blocks/missed?limit=1"))?)
        })
    }

    fn invariant(&mut self, which: Invariant) -> Probe {
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
            Invariant::SingleTip => {
                let tips: Vec<_> = (1..=self.backend().nodes())
                    .filter_map(|n| self.backend().tip(n).ok())
                    .collect();
                Ok(match tips.split_first() {
                    Some((first, rest)) if rest.iter().all(|t| t == first) => Outcome::Pass,
                    _ => expected("one tip", format!("{tips:?}")),
                })
            }
            Invariant::Supply => self.supply(),
            Invariant::Schedule => self.schedule(),
            Invariant::RevertSymmetry | Invariant::RestartEqual => Ok(Outcome::Skipped(
                "compared from both runs by scenario compare".into(),
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
            let Some(reward) = amount(&block["reward"]) else {
                return Ok(Outcome::HarnessError(
                    "a block of /api/blocks names no reward".into(),
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

    /// Every block was forged by a delegate of its round, none twice in a round.
    fn schedule(&mut self) -> Probe {
        let (node, tip) = self.runner.observe()?;
        let length = self.runner.chain.round_length.max(1);
        let mut rounds: std::collections::HashMap<u32, Vec<String>> =
            std::collections::HashMap::new();
        let mut seen: std::collections::HashSet<(u32, String)> = std::collections::HashSet::new();
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
            if !seen.insert((round, generator.clone())) {
                return Ok(expected(
                    "one block a round",
                    format!("{generator} twice in round {round}"),
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
                "the digests of both implementations are compared by scenario compare".into(),
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
        cases.push(ctx.case(format!("invariant {which:?}").to_lowercase(), h, outcome));
    }
    cases
}
