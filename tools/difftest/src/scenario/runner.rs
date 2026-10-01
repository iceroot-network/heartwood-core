//! The runner: a compiled scenario driven on a [`Backend`]. Each step fires when the observed tip
//! (the highest of the nodes that answer) first satisfies its trigger; its action is done, then
//! its checks are evaluated, each one a case of the report ([`super::checks`]). What a step did is
//! kept as the run's [`Record`], which `scenario compare` reads.

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};
use std::time::{Duration, Instant};

use super::backend::{Backend, BackendError, Submitted, Tip};
use super::chain::Chain;
use super::checks::{self, Ctx};
use super::format::{Action, Compiled, Level, Part, RoundPosition, Step, Trigger, Via};
use super::record::{Record, StepRecord};
use crate::errors::Result;
use crate::report::{CaseResult, Outcome, Report};

/// The longest a live network may take to bring a step's trigger.
pub const STEP_LIMIT: Duration = Duration::from_secs(40 * 60);

/// The rounds a scenario may reach: a test network runs at most five.
pub const MAX_ROUNDS: u32 = 5;

/// The last height `scenario` looks at as planned: each step's fire height with its wait and its
/// check's window.
pub fn last_height(scenario: &super::format::Scenario, round_length: u32) -> Result<u32> {
    let fires = super::compile::fire_heights(scenario, round_length)?;
    let reach = |(step, fire): (&Step, &u32)| {
        let window = step
            .check
            .as_ref()
            .and_then(|c| c.within_blocks)
            .unwrap_or(0);
        let wait = match step.action {
            Some(Action::Wait) => step.blocks.unwrap_or(0),
            Some(Action::Submit) => 1,
            _ => 0,
        };
        fire.saturating_add(wait).saturating_add(window)
    };
    Ok(scenario
        .steps
        .iter()
        .zip(&fires)
        .map(reach)
        .max()
        .unwrap_or(0))
}

/// Where the network was when a step fired or is now: the tip's height and slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Seen {
    /// The tip's height.
    pub height: u32,
    /// The tip's slot.
    pub slot: u64,
}

/// Whether `trigger` is satisfied at `now`, the previous step having fired at `previous`, on a
/// chain of rounds of `round_length` blocks.
pub fn due(trigger: &Trigger, round_length: u32, previous: Seen, now: Seen) -> bool {
    let round_end = |round: u64| u64::from(round_length).saturating_mul(round);
    let height = u64::from(now.height);
    if let Some(h) = trigger.height {
        return now.height >= h;
    }
    if let Some(n) = trigger.after_blocks {
        return now.height >= previous.height.saturating_add(n);
    }
    if let Some(slot) = trigger.slot {
        return now.slot >= slot;
    }
    if let Some(n) = trigger.slots_after {
        return now.slot >= previous.slot.saturating_add(u64::from(n));
    }
    if let (Some(round), Some(position)) = (trigger.round, trigger.position) {
        return match position {
            RoundPosition::Start => height > round_end(round.saturating_sub(1)),
            RoundPosition::End => height >= round_end(round),
        };
    }
    trigger.round_end == Some(true)
        && height
            >= u64::from(previous.height.div_ceil(round_length.max(1)).max(1))
                * u64::from(round_length)
}

fn harness(action: &str, e: impl ToString) -> Outcome {
    Outcome::HarnessError(format!("{action}: {}", e.to_string()))
}

/// The state of one run.
pub struct Runner<'a> {
    pub(super) backend: &'a mut dyn Backend,
    pub(super) compiled: &'a Compiled,
    pub(super) chain: Chain,
    pub(super) answers: HashMap<String, Submitted>,
    results: Vec<CaseResult>,
    records: Vec<StepRecord>,
    needs_slot: bool,
    answered_nodes: RefCell<BTreeSet<u32>>,
    expected_nodes: BTreeSet<u32>,
    stopped_relays: BTreeSet<u32>,
    /// The compared, exempt and missing nodes at the latest single-tip probe.
    pub(super) single_tip_label: String,
    settle_height: u32,
}

impl<'a> Runner<'a> {
    /// The runner of `compiled` on `backend`.
    pub fn new(compiled: &'a Compiled, backend: &'a mut dyn Backend) -> Result<Self> {
        let scenario = &compiled.scenario;
        let needs_slot = scenario
            .steps
            .iter()
            .any(|s| s.at.slot.is_some() || s.at.slots_after.is_some());
        if needs_slot && backend.name() == "lockstep" {
            return Err(crate::Error::Usage(
                "the lockstep backend has no slot clock; slot triggers are not supported".into(),
            ));
        }
        let chain = Chain::new(scenario)?;
        let last = last_height(scenario, chain.round_length)?;
        let most = MAX_ROUNDS.saturating_mul(chain.round_length);
        if last > most {
            return Err(crate::Error::Usage(format!(
                "{} looks at height {last}, past the {MAX_ROUNDS} rounds ({most} blocks) a test \
                 network runs",
                scenario.id
            )));
        }
        backend
            .rules(&chain.crypto)
            .map_err(|e| crate::Error::Fixture(e.to_string()))?;
        Ok(Runner {
            backend,
            compiled,
            chain,
            answers: HashMap::new(),
            results: Vec::new(),
            records: Vec::new(),
            needs_slot,
            answered_nodes: RefCell::default(),
            expected_nodes: BTreeSet::new(),
            stopped_relays: BTreeSet::new(),
            single_tip_label: String::new(),
            settle_height: 1_u32.saturating_add(scenario.settle_blocks),
        })
    }

    /// The node with the highest tip among those that answer, and its tip.
    pub(super) fn observe(&self) -> std::result::Result<(u32, Tip), BackendError> {
        let mut best: Option<(u32, Tip)> = None;
        let mut failure = None;
        for node in 1..=self.backend.nodes() {
            match self.read_tip(node) {
                Ok(tip) => {
                    if best.as_ref().is_none_or(|(_, b)| tip.height > b.height) {
                        best = Some((node, tip));
                    }
                }
                Err(e) => failure = Some(e),
            }
        }
        best.ok_or_else(|| {
            failure.unwrap_or(BackendError::Failed {
                action: "observe the tip",
                message: "the network has no node".into(),
            })
        })
    }

    /// Read a tip and remember that the node has answered during this run.
    pub(super) fn read_tip(&self, node: u32) -> std::result::Result<Tip, BackendError> {
        let tip = self.backend.tip(node)?;
        self.answered_nodes.borrow_mut().insert(node);
        Ok(tip)
    }

    /// Why a relay is exempt from answering the single-tip check, if it is.
    pub(super) fn tip_exemption(&self, node: u32) -> Option<&'static str> {
        if self.stopped_relays.contains(&node) {
            Some("deliberately stopped")
        } else if !self.expected_nodes.contains(&node)
            && !self.answered_nodes.borrow().contains(&node)
        {
            // The backend interface exposes no held-back list.
            Some("never answered; held-back list unavailable")
        } else {
            None
        }
    }

    fn seen(&self, node: u32, tip: &Tip) -> std::result::Result<Seen, BackendError> {
        let slot = if self.needs_slot {
            let answer = self
                .backend
                .get(node, &format!("/api/blocks/{}", tip.height))?;
            answer.body["data"]["timestamp"]["epoch"]
                .as_u64()
                .filter(|_| answer.status == 200)
                .ok_or_else(|| BackendError::Failed {
                    action: "read the tip's slot",
                    message: format!("node {node} answered without a block timestamp"),
                })?
                / self.chain.block_time.max(1)
        } else {
            u64::from(tip.height)
        };
        Ok(Seen {
            height: tip.height,
            slot,
        })
    }

    /// Wait until `trigger` is satisfied; the network then.
    fn until_due(
        &mut self,
        trigger: &Trigger,
        previous: Seen,
    ) -> std::result::Result<(u32, Tip, Seen), BackendError> {
        let started = Instant::now();
        loop {
            let (node, tip) = self.observe()?;
            let now = self.seen(node, &tip)?;
            if now.height >= self.settle_height
                && due(trigger, self.chain.round_length, previous, now)
            {
                return Ok((node, tip, now));
            }
            if started.elapsed() > STEP_LIMIT {
                return Err(BackendError::Failed {
                    action: "wait for the trigger",
                    message: format!(
                        "the tip stays at {} for {} minutes",
                        tip.height,
                        STEP_LIMIT.as_secs() / 60
                    ),
                });
            }
            self.backend.wait()?;
        }
    }

    fn case(&mut self, step: usize, tip: u32, label: String, outcome: Outcome) {
        self.results.push(CaseResult {
            file: self.compiled.scenario.id.clone(),
            line: step + 1,
            op: "scenario.step".into(),
            label,
            height: u64::from(tip),
            outcome,
        });
    }

    /// Do the action of `step`; the submitted transactions' answers are kept.
    fn act(
        &mut self,
        index: usize,
        step: &Step,
        tip: &Tip,
    ) -> std::result::Result<Vec<Submitted>, Outcome> {
        let Some(action) = step.action else {
            return Ok(Vec::new());
        };
        let node = || {
            step.node
                .ok_or_else(|| harness("the step", "it names no node"))
        };
        let part = step.role.unwrap_or(Part::Node);
        let fail = |e: BackendError| harness(&format!("{action:?}").to_lowercase(), e);
        match action {
            Action::Submit => {
                let txs: Vec<_> = self
                    .compiled
                    .transactions
                    .iter()
                    .filter(|t| t.step == index)
                    .cloned()
                    .collect();
                let answered = self
                    .backend
                    .submit(node()?, step.via.unwrap_or(Via::Api), &txs)
                    .map_err(fail)?;
                for a in &answered {
                    self.answers.insert(a.reference.clone(), a.clone());
                }
                return Ok(answered);
            }
            Action::Stop => {
                let node = node()?;
                self.backend.stop(node, part, false).map(|()| {
                    if part != Part::Forger {
                        self.stopped_relays.insert(node);
                    }
                })
            }
            Action::Start | Action::Restart => {
                let node = node()?;
                if part != Part::Forger {
                    self.stopped_relays.remove(&node);
                    self.expected_nodes.insert(node);
                }
                if action == Action::Start {
                    self.backend.start(node, part)
                } else {
                    self.backend
                        .restart(node, part, step.mode.unwrap_or_default())
                }
            }
            Action::Partition => self.backend.partition(&step.groups),
            Action::Heal => self.backend.heal(),
            Action::Revert => {
                let blocks = step
                    .blocks
                    .ok_or_else(|| harness("revert", "it names no blocks"))?;
                self.backend.revert(blocks)
            }
            Action::Fork => {
                let branch = step
                    .branch
                    .as_deref()
                    .ok_or_else(|| harness("fork", "it names no branch"))?;
                self.backend.fork(branch)
            }
            Action::Wait => {
                let blocks = step
                    .blocks
                    .ok_or_else(|| harness("wait", "it names no blocks"))?;
                self.wait_blocks(tip.height + blocks)
            }
        }
        .map(|()| Vec::new())
        .map_err(fail)
    }

    /// Wait until the network's tip is at `height`.
    pub(super) fn wait_blocks(&mut self, height: u32) -> std::result::Result<(), BackendError> {
        let started = Instant::now();
        while self.observe()?.1.height < height {
            if started.elapsed() > STEP_LIMIT {
                return Err(BackendError::Failed {
                    action: "wait for blocks",
                    message: format!(
                        "no block {height} within {} minutes",
                        STEP_LIMIT.as_secs() / 60
                    ),
                });
            }
            self.backend.wait()?;
        }
        Ok(())
    }

    /// Run every step.
    pub fn run(mut self) -> (Vec<CaseResult>, Record) {
        let compiled = self.compiled;
        let scenario = &compiled.scenario;
        if scenario.settle_blocks > 0
            && let Ok((_, tip)) = self.observe()
        {
            self.settle_height = tip.height.saturating_add(scenario.settle_blocks);
        }
        let mut previous = Seen {
            height: self.settle_height,
            slot: u64::from(self.settle_height),
        };
        for (index, step) in scenario.steps.iter().enumerate() {
            let name = step
                .name
                .clone()
                .unwrap_or_else(|| format!("step {}", index + 1));
            let (node, tip, seen) = match self.until_due(&step.at, previous) {
                Ok(fired) => fired,
                Err(e) => {
                    self.case(index, 0, name, harness("the trigger", e));
                    break;
                }
            };
            previous = seen;
            let mut record = StepRecord::new(index, &name, &tip);
            match self.act(index, step, &tip) {
                Ok(answered) => {
                    record.answered(&answered);
                    if let Some(action) = step.action {
                        self.case(
                            index,
                            tip.height,
                            format!("{name}: {action:?}").to_lowercase(),
                            Outcome::Pass,
                        );
                    }
                }
                Err(outcome) => self.case(index, tip.height, format!("{name}: action"), outcome),
            }
            if let Some(check) = &step.check {
                let mut ctx = Ctx {
                    runner: &mut self,
                    step: index,
                    name: &name,
                    node,
                    values: Vec::new(),
                };
                let cases = checks::evaluate(&mut ctx, check);
                record.projections = ctx.values;
                for case in cases {
                    record.checked(&case.label, &case.outcome);
                    self.results.push(case);
                }
            }
            self.records.push(record);
        }
        let last = self.observe().map(|(_, tip)| tip).ok();
        let record = Record::new(
            &self.compiled.scenario.id,
            self.backend.name(),
            self.records,
            last,
        );
        (self.results, record)
    }
}

/// Run `compiled` on `backend` at `level`: the report of every step's checks, and the run's
/// record.
pub fn run(
    compiled: &Compiled,
    backend: &mut dyn Backend,
    level: Level,
) -> Result<(Report, Record)> {
    let (results, record) = Runner::new(compiled, backend)?.run();
    let report = Report {
        title: format!("scenario {}", compiled.scenario.id),
        source: format!("the {} backend at {level:?}", record.backend),
        files: 1,
        results,
    };
    Ok((report, record))
}
