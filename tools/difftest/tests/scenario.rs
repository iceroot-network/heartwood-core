//! The scenario compiler and runner: the format's schema, the triggers, the planning of nonces and
//! heights, every action and check against a fake backend, the run records and their comparison,
//! the external backend's calls against a stub executable and a stub API, and the lockstep
//! backend on the fixtures under `tests/fixtures/scenario` (integration tests, ignored for the
//! reason `integration`: they run the real reference, `DIFFTEST_TEST_SOLAR_DIR` or
//! `DIFFTEST_TEST_IMAGE`, and fail without one unless `DIFFTEST_TEST_NO_ORACLE` says to skip).

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};

use heartwood_crypto::transactions::TransactionBuilder;
use heartwood_crypto::utils::hex;
use heartwood_crypto::{Aux, KeyPair, Milestones, Network, Transaction};
use heartwood_difftest::report::{Outcome, Report};
use heartwood_difftest::scenario::backend::Answer;
use heartwood_difftest::scenario::backends::{external, heartwood};
use heartwood_difftest::scenario::chain::Chain;
use heartwood_difftest::scenario::compile::{fire_heights, specs};
use heartwood_difftest::scenario::format::{
    Compiled, CompiledTx, Level, Part, RestartModes, Scenario, Via,
};
use heartwood_difftest::scenario::projection::{self, Names};
use heartwood_difftest::scenario::record::{Record, StepRecord, compare};
use heartwood_difftest::scenario::runner::{Seen, due, run};
use heartwood_difftest::scenario::{self, Backend, BackendError, Submitted, Tip};
use heartwood_difftest::store::ScratchDir;
use serde_json::{Value, json};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/scenario")
}

// The schema.

fn scenario_json(steps: Value) -> Value {
    json!({
        "id": "fake", "title": "t", "levels": ["L2", "L3"], "network": "devnet",
        "genesisSeed": "seed", "actors": {"alice": {"seed": 1}, "bob": {"seed": 2}},
        "steps": steps,
    })
}

#[test]
fn every_object_of_the_format_refuses_a_key_it_does_not_know() {
    let full = scenario_json(json!([{
        "at": {"height": 2}, "do": "submit", "node": 1, "via": "api",
        "txs": [{"ref": "t", "type": "transfer", "from": "alice", "to": [["bob", "1"]]}],
        "check": {"accept": ["t"], "api": {"projection": "wallets"}, "missed": {"node": 1, "atLeast": 1}},
        "mode": {"heartwood": "replay"},
    }]));
    serde_json::from_value::<Scenario>(full.clone()).expect("the base file reads");
    let paths: [&[&str]; 9] = [
        &[],
        &["actors", "alice"],
        &["steps", "0"],
        &["steps", "0", "at"],
        &["steps", "0", "txs", "0"],
        &["steps", "0", "check"],
        &["steps", "0", "check", "api"],
        &["steps", "0", "check", "missed"],
        &["steps", "0", "mode"],
    ];
    for path in paths {
        let mut value = full.clone();
        let mut at = &mut value;
        for key in path {
            at = match key.parse::<usize>() {
                Ok(index) => &mut at[index],
                Err(_) => &mut at[*key],
            };
        }
        at["surprise"] = json!(1);
        let error = serde_json::from_value::<Scenario>(value)
            .expect_err(&format!("a stray key at {path:?} is refused"))
            .to_string();
        assert!(error.contains("unknown field `surprise`"), "{error}");
    }
    for (bad, why) in [
        (json!({"nonce": "last"}), "a nonce is next or a number"),
        (json!({"via": "http"}), "via is api or p2p"),
        (json!({"do": "explode"}), "an action of the list"),
    ] {
        let mut value = full.clone();
        let step = &mut value["steps"][0];
        for (key, item) in bad.as_object().expect("object") {
            if key == "nonce" {
                step["txs"][0][key] = item.clone();
            } else {
                step[key] = item.clone();
            }
        }
        assert!(serde_json::from_value::<Scenario>(value).is_err(), "{why}");
    }
}

#[test]
fn the_fixtures_are_scenarios_with_content_names_that_fit_a_short_chain() {
    let mut found = 0;
    for entry in std::fs::read_dir(fixtures()).expect("fixtures") {
        let path = entry.expect("entry").path();
        let scenario = scenario::load(&path).expect("the fixture reads");
        let stem = path.file_stem().and_then(|s| s.to_str()).expect("stem");
        assert_eq!(scenario.id, stem);
        assert!(
            scenario
                .id
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b == b'-'),
            "{} is a content name",
            scenario.id
        );
        assert_eq!(scenario.levels, [Level::L2]);
        let chain = Chain::new(&scenario).expect("chain");
        let fires = fire_heights(&scenario, chain.round_length).expect("fires");
        assert!(
            fires.iter().all(|h| *h <= 5 * chain.round_length),
            "{} stays within five rounds",
            scenario.id
        );
        found += 1;
    }
    assert_eq!(found, 3);
}

// The triggers.

fn seen(height: u32) -> Seen {
    Seen {
        height,
        slot: u64::from(height) * 2,
    }
}

fn trigger(value: Value) -> heartwood_difftest::scenario::format::Trigger {
    serde_json::from_value(value).expect("trigger")
}

#[test]
fn a_trigger_is_due_when_the_recorded_tips_first_satisfy_it() {
    let at = |t: Value, previous: u32, now: u32| due(&trigger(t), 53, seen(previous), seen(now));
    // A height is a lower bound.
    assert!(!at(json!({"height": 5}), 1, 4));
    assert!(at(json!({"height": 5}), 1, 5));
    assert!(at(json!({"height": 5}), 1, 9));
    // Blocks after the previous step.
    assert!(!at(json!({"afterBlocks": 3}), 10, 12));
    assert!(at(json!({"afterBlocks": 3}), 10, 13));
    // Slots: on the slot of the tips, not their height.
    assert!(!at(json!({"slot": 20}), 1, 9));
    assert!(at(json!({"slot": 20}), 1, 10));
    assert!(!at(json!({"slotsAfter": 4}), 10, 11));
    assert!(at(json!({"slotsAfter": 4}), 10, 12));
    // The start and the end of a round.
    assert!(!at(json!({"round": 2, "position": "start"}), 1, 53));
    assert!(at(json!({"round": 2, "position": "start"}), 1, 54));
    assert!(at(json!({"round": 1, "position": "start"}), 1, 2));
    assert!(!at(json!({"round": 2, "position": "end"}), 1, 105));
    assert!(at(json!({"round": 2, "position": "end"}), 1, 106));
    // The end of the round the previous step fired in.
    assert!(!at(json!({"roundEnd": true}), 20, 52));
    assert!(at(json!({"roundEnd": true}), 20, 53));
    assert!(at(json!({"roundEnd": true}), 53, 53));
    assert!(!at(json!({"roundEnd": true}), 54, 106 - 1));
    assert!(at(json!({"roundEnd": true}), 54, 106));
    // A trigger of no form is never due.
    assert!(!at(json!({}), 1, 1000));
}

#[test]
fn a_trigger_gives_the_tip_it_fires_at() {
    let scenario: Scenario = serde_json::from_value(scenario_json(json!([
        {"at": {"height": 5}},
        {"at": {"afterBlocks": 2}},
        {"at": {"round": 2, "position": "start"}},
        {"at": {"round": 2, "position": "end"}},
        {"at": {"roundEnd": true}},
        {"at": {"afterBlocks": 1}},
        {"at": {"height": 3}},
        {"at": {"slotsAfter": 4}},
    ])))
    .expect("scenario");
    // A height below the previous step's does not send the plan back.
    assert_eq!(
        fire_heights(&scenario, 53).expect("fires"),
        [5, 7, 54, 106, 106, 107, 107, 111]
    );
    for bad in [
        json!({"height": 5, "afterBlocks": 1}),
        json!({"round": 2}),
        json!({"position": "start"}),
        json!({}),
    ] {
        let scenario: Scenario =
            serde_json::from_value(scenario_json(json!([{"at": bad}]))).expect("scenario");
        assert!(fire_heights(&scenario, 53).is_err(), "{bad}");
    }
}

// The planning of nonces, heights and fees.

#[test]
fn a_transaction_is_planned_after_its_step_with_the_nonces_of_its_sender() {
    let scenario: Scenario = serde_json::from_value(scenario_json(json!([
        {"at": {"height": 2}, "do": "submit", "node": 1, "txs": [
            {"ref": "a1", "type": "transfer", "from": "alice", "to": [["bob", "10"]]},
            {"ref": "a2", "type": "transfer", "from": "alice", "nonce": 7, "to": [["bob", "20"]],
             "fee": "5", "memo": "m"},
            {"ref": "b1", "type": "vote", "from": "bob", "asset": {"votes": [["genesis_2", 100]]}},
            {"ref": "grant", "type": "transfer", "from": "genesis", "to": [["alice", "30"]]},
            {"ref": "d1", "type": "vote", "from": "genesis_3", "asset": {"votes": [["genesis_3", 100]]}}]},
        {"at": {"afterBlocks": 3}, "do": "submit", "node": 1, "txs": [
            {"ref": "a3", "type": "transfer", "from": "alice", "to": [["bob", "40"]]}]},
    ])))
    .expect("scenario");
    let chain = Chain::new(&scenario).expect("chain");
    let planned = specs(&scenario, &chain).expect("specs");
    let heights: Vec<(usize, u32, &str)> = planned
        .iter()
        .map(|(step, height, name, _)| (*step, *height, name.as_str()))
        .collect();
    assert_eq!(
        heights,
        [
            (0, 3, "a1"),
            (0, 3, "a2"),
            (0, 3, "b1"),
            (0, 3, "grant"),
            (0, 3, "d1"),
            (1, 6, "a3")
        ]
    );
    let nonces: Vec<&str> = planned
        .iter()
        .map(|(.., spec)| spec["nonce"].as_str().expect("nonce"))
        .collect();
    // A genesis delegate signed its registration (1) and its vote (2) in the genesis block: its
    // next nonce is 3.
    assert_eq!(nonces, ["1", "7", "1", "1", "3", "8"]);
    assert_eq!(chain.genesis_nonce("genesis_3").expect("delegate"), 2);
    assert_eq!(chain.genesis_nonce("alice").expect("actor"), 0);
    let spec = |name: &str| {
        &planned
            .iter()
            .find(|(_, _, n, _)| n == name)
            .expect("planned")
            .3
    };
    // The fee is the oracle's floor unless the file gives one.
    assert!(spec("a1").get("fee").is_none());
    assert_eq!(spec("a2")["fee"], "5");
    assert_eq!(spec("a2")["memo"], "m");
    let alice = chain.person("alice").expect("alice");
    assert_eq!(spec("a1")["passphrase"], alice.passphrase.as_str());
    assert_eq!(
        spec("a1")["asset"]["recipients"],
        json!([[chain.person("bob").expect("bob").address, "10"]])
    );
    assert_eq!(spec("b1")["asset"]["votes"], json!([["genesis_2", 100]]));
    assert_ne!(
        spec("grant")["passphrase"],
        alice.passphrase.as_str(),
        "the genesis wallet is its own"
    );
    // The same file plans the same specs.
    assert_eq!(planned, specs(&scenario, &chain).expect("again"));
}

#[test]
fn a_scenario_that_cannot_be_planned_is_refused() {
    let plan = |steps: Value| {
        let scenario: Scenario =
            serde_json::from_value(scenario_json(steps)).expect("the file reads");
        let chain = Chain::new(&scenario).expect("chain");
        specs(&scenario, &chain).expect_err("refused").to_string()
    };
    let tx = |name: &str, kind: &str| json!({"ref": name, "type": kind, "from": "alice", "to": [["bob", "1"]]});
    let twice = plan(json!([
        {"at": {"height": 2}, "do": "submit", "node": 1, "txs": [tx("t", "transfer"), tx("t", "transfer")]},
    ]));
    assert!(twice.contains("named twice"), "{twice}");
    let no_submit = plan(json!([{"at": {"height": 2}, "txs": [tx("t", "transfer")]}]));
    assert!(no_submit.contains("without a submit"), "{no_submit}");
    let unknown = plan(json!([
        {"at": {"height": 2}, "do": "submit", "node": 1, "txs": [tx("t", "frobnicate")]},
    ]));
    assert!(unknown.contains("no such type"), "{unknown}");
    let stranger = plan(json!([
        {"at": {"height": 2}, "do": "submit", "node": 1,
         "txs": [{"ref": "t", "type": "transfer", "from": "mallory", "to": [["bob", "1"]]}]},
    ]));
    assert!(stranger.contains("mallory"), "{stranger}");
}

// A fake backend.

/// A network of `nodes` nodes that grows a block whenever the runner waits, keeps a log of what it
/// is asked and answers the API from pages the test sets.
struct Fake {
    nodes: u32,
    backend_name: &'static str,
    unavailable: HashMap<u32, (u32, u32)>,
    height: Cell<u32>,
    /// The height the network stops growing at: a wait beyond it fails.
    limit: u32,
    log: RefCell<Vec<String>>,
    /// The transactions the pools refuse, by reference, with the code.
    refuse: HashMap<String, String>,
    /// The ids accepted and not yet in a block.
    pool: RefCell<Vec<String>>,
    chain: RefCell<HashSet<String>>,
    pages: HashMap<(u32, String), Value>,
    verdicts: HashMap<&'static str, Result<(), String>>,
    /// Blocks a node's tip is behind the network's.
    lag: HashMap<u32, u32>,
    delegates: Vec<String>,
    unsupported: HashSet<&'static str>,
    /// The height at which the lagging nodes catch up.
    catch_up: Option<u32>,
}

impl Fake {
    fn new(nodes: u32) -> Fake {
        Fake {
            nodes,
            backend_name: "fake",
            unavailable: HashMap::new(),
            height: Cell::new(1),
            limit: 200,
            log: RefCell::default(),
            refuse: HashMap::new(),
            pool: RefCell::default(),
            chain: RefCell::default(),
            pages: HashMap::new(),
            verdicts: HashMap::new(),
            lag: HashMap::new(),
            delegates: Vec::new(),
            unsupported: HashSet::new(),
            catch_up: None,
        }
    }

    fn page(&mut self, node: u32, path: &str, body: Value) {
        self.pages.insert((node, path.to_owned()), body);
    }

    fn page_everywhere(&mut self, path: &str, body: Value) {
        for node in 1..=self.nodes {
            self.page(node, path, body.clone());
        }
    }

    fn note(&self, line: String) {
        self.log.borrow_mut().push(line);
    }

    fn refused(&self, action: &'static str) -> Result<(), BackendError> {
        if self.unsupported.contains(action) {
            return Err(BackendError::Unsupported {
                backend: "fake",
                action,
            });
        }
        Ok(())
    }
}

impl Backend for Fake {
    fn name(&self) -> &'static str {
        self.backend_name
    }

    fn nodes(&self) -> u32 {
        self.nodes
    }

    fn start(&mut self, node: u32, part: Part) -> Result<(), BackendError> {
        self.note(format!("start {node} {part:?}"));
        self.refused("start")
    }

    fn stop(&mut self, node: u32, part: Part, kill: bool) -> Result<(), BackendError> {
        self.note(format!("stop {node} {part:?} {kill}"));
        self.refused("stop")
    }

    fn restart(&mut self, node: u32, part: Part, modes: RestartModes) -> Result<(), BackendError> {
        self.note(format!("restart {node} {part:?} {modes:?}"));
        self.refused("restart")
    }

    fn partition(&mut self, groups: &[Vec<u32>]) -> Result<(), BackendError> {
        self.note(format!("partition {groups:?}"));
        self.refused("partition")
    }

    fn heal(&mut self) -> Result<(), BackendError> {
        self.note("heal".into());
        self.refused("heal")
    }

    fn api_url(&self, node: u32) -> Result<String, BackendError> {
        Ok(format!("http://fake-{node}"))
    }

    fn archive(&mut self, _: &Path) -> Result<(), BackendError> {
        Ok(())
    }

    fn submit(
        &mut self,
        node: u32,
        via: Via,
        transactions: &[CompiledTx],
    ) -> Result<Vec<Submitted>, BackendError> {
        let names: Vec<&str> = transactions.iter().map(|t| t.reference.as_str()).collect();
        self.note(format!("submit {node} {via:?} {}", names.join(",")));
        Ok(transactions
            .iter()
            .map(|t| {
                let code = self.refuse.get(&t.reference).cloned();
                if code.is_none() {
                    self.pool.borrow_mut().push(t.id.clone());
                }
                Submitted {
                    reference: t.reference.clone(),
                    accepted: code.is_none(),
                    code,
                    message: None,
                }
            })
            .collect())
    }

    fn tip(&self, node: u32) -> Result<Tip, BackendError> {
        if self
            .unavailable
            .get(&node)
            .is_some_and(|(from, to)| (*from..=*to).contains(&self.height.get()))
        {
            return Err(BackendError::Failed {
                action: "read the tip",
                message: format!("node {node} does not answer"),
            });
        }
        let height = self.height.get() - self.lag.get(&node).copied().unwrap_or(0);
        Ok(Tip {
            height,
            id: format!("block-{height}"),
        })
    }

    fn revert(&mut self, blocks: u32) -> Result<(), BackendError> {
        self.note(format!("revert {blocks}"));
        self.refused("revert")
    }

    fn fork(&mut self, branch: &str) -> Result<(), BackendError> {
        self.note(format!("fork {branch}"));
        self.refused("fork")
    }

    fn get(&self, node: u32, path: &str) -> Result<Answer, BackendError> {
        Ok(match self.pages.get(&(node, path.to_owned())) {
            Some(body) => Answer {
                status: 200,
                body: body.clone(),
            },
            None => Answer {
                status: 404,
                body: json!({}),
            },
        })
    }

    fn forged(&self, _: u32, id: &str) -> Result<bool, BackendError> {
        Ok(self.chain.borrow().contains(id))
    }

    fn wait(&mut self) -> Result<(), BackendError> {
        if self.height.get() >= self.limit {
            return Err(BackendError::Failed {
                action: "wait",
                message: "the chain ends".into(),
            });
        }
        self.height.set(self.height.get() + 1);
        if self.catch_up.is_some_and(|at| self.height.get() >= at) {
            self.lag.clear();
        }
        let taken: Vec<String> = self.pool.borrow_mut().drain(..).collect();
        self.chain.borrow_mut().extend(taken);
        Ok(())
    }

    fn verified(&self, name: &str) -> Option<Result<(), String>> {
        self.verdicts.get(name).cloned()
    }

    fn forger_delegates(&self, _: u32) -> Result<Vec<String>, BackendError> {
        Ok(self.delegates.clone())
    }
}

/// A compiled scenario of `steps`, its transactions `(reference, step)` with the ids `id-<ref>`.
fn compiled(steps: Value, transactions: &[(&str, usize)]) -> Compiled {
    Compiled {
        scenario: serde_json::from_value(scenario_json(steps)).expect("scenario"),
        transactions: transactions
            .iter()
            .map(|(reference, step)| CompiledTx {
                reference: (*reference).to_owned(),
                step: *step,
                height: 3,
                id: format!("id-{reference}"),
                bytes: format!("00{reference}"),
            })
            .collect(),
    }
}

fn run_on(fake: &mut Fake, compiled: &Compiled) -> (Report, Record) {
    run(compiled, fake, Level::L3).expect("the run")
}

/// The outcome of the case whose label holds `part`.
fn outcome<'a>(report: &'a Report, part: &str) -> &'a Outcome {
    let mut found = report.results.iter().filter(|c| c.label.contains(part));
    let case = found
        .next()
        .unwrap_or_else(|| panic!("no case {part}: {:#?}", report.results));
    assert!(found.next().is_none(), "two cases {part}");
    &case.outcome
}

fn mismatch(want: &str, got: &str) -> Outcome {
    Outcome::VerdictMismatch {
        oracle: want.into(),
        heartwood: got.into(),
    }
}

// Every action.

#[test]
fn every_action_reaches_the_backend_in_step_order() {
    let compiled = compiled(
        json!([
            {"at": {"height": 2}, "do": "submit", "node": 1, "via": "api"},
            {"at": {"afterBlocks": 1}, "do": "wait", "blocks": 2},
            {"at": {"afterBlocks": 1}, "do": "stop", "node": 2, "role": "forger"},
            {"at": {"afterBlocks": 1}, "do": "start", "node": 2, "role": "forger"},
            {"at": {"afterBlocks": 1}, "do": "restart", "node": 1, "role": "relay",
             "mode": {"reference": "rebuild", "heartwood": "replay"}},
            {"at": {"afterBlocks": 1}, "do": "partition", "groups": [[1], [2]]},
            {"at": {"afterBlocks": 1}, "do": "heal"},
            {"at": {"afterBlocks": 1}, "do": "revert", "blocks": 2},
            {"at": {"afterBlocks": 1}, "do": "fork", "branch": "later"},
            {"at": {"afterBlocks": 1}, "do": "stop", "node": 1},
        ]),
        &[("t1", 0), ("t2", 0)],
    );
    let mut fake = Fake::new(2);
    let (report, record) = run_on(&mut fake, &compiled);
    assert_eq!(report.failed(), 0, "{:#?}", report.results);
    assert_eq!(
        *fake.log.borrow(),
        [
            "submit 1 Api t1,t2",
            "stop 2 Forger false",
            "start 2 Forger",
            "restart 1 Relay RestartModes { reference: Rebuild, heartwood: Replay }",
            "partition [[1], [2]]",
            "heal",
            "revert 2",
            "fork later",
            "stop 1 Node false",
        ]
    );
    assert_eq!(record.steps.len(), 10);
    // The wait took two blocks: the step after it fired at the tip they brought.
    assert_eq!(record.steps[2].tip.0, record.steps[1].tip.0 + 2);
    assert_eq!(record.steps[0].submitted.len(), 2);
    assert_eq!(record.tip.as_ref().map(|t| t.0), Some(fake.height.get()));
}

#[test]
fn an_action_a_backend_does_not_offer_fails_its_step_and_the_run_goes_on() {
    let compiled = compiled(
        json!([
            {"at": {"height": 2}, "do": "partition", "groups": [[1], [2]]},
            {"at": {"afterBlocks": 1}, "do": "heal"},
        ]),
        &[],
    );
    let mut fake = Fake::new(2);
    fake.unsupported.insert("partition");
    let (report, record) = run_on(&mut fake, &compiled);
    assert!(matches!(
        outcome(&report, "action"),
        Outcome::HarnessError(m) if m.contains("the fake backend cannot partition")
    ));
    assert!(matches!(outcome(&report, "heal"), Outcome::Pass));
    assert_eq!(record.steps.len(), 2);
    // A step that names what it needs but lacks it is the harness's error, not a panic.
    let incomplete = self::compiled(
        json!([
            {"at": {"height": 2}, "do": "fork"},
            {"at": {"afterBlocks": 1}, "do": "revert"},
            {"at": {"afterBlocks": 1}, "do": "wait"},
            {"at": {"afterBlocks": 1}, "do": "stop"},
        ]),
        &[],
    );
    let (report, _) = run_on(&mut Fake::new(1), &incomplete);
    assert_eq!(report.failed(), 4, "{:#?}", report.results);
}

#[test]
fn a_trigger_that_never_comes_ends_the_run_with_the_harness_error() {
    let compiled = compiled(
        json!([
            {"at": {"height": 2}, "do": "heal"},
            {"at": {"height": 99}, "do": "heal"},
            {"at": {"afterBlocks": 1}, "do": "heal"},
        ]),
        &[],
    );
    let mut fake = Fake::new(1);
    fake.limit = 6;
    let (report, record) = run_on(&mut fake, &compiled);
    assert_eq!(report.failed(), 1, "{:#?}", report.results);
    assert!(matches!(
        outcome(&report, "step 2"),
        Outcome::HarnessError(m) if m.contains("the trigger") && m.contains("the chain ends")
    ));
    assert_eq!(record.steps.len(), 1, "later steps do not fire");
}

// Every check.

#[test]
fn a_submit_is_checked_against_the_answers_of_the_pool() {
    let compiled = compiled(
        json!([{
            "at": {"height": 2}, "do": "submit", "node": 1, "via": "api",
            "check": {
                "accept": ["ok", "no", "ghost"], "invalid": ["ok", "no"],
                "codes": {"no": "ERR_APPLY", "ok": "ERR_APPLY", "other": "ERR_APPLY"},
            },
        }]),
        &[("ok", 0), ("no", 0), ("other", 0)],
    );
    let mut fake = Fake::new(1);
    fake.refuse.insert("no".into(), "ERR_OFFLINE".into());
    fake.refuse.insert("other".into(), "ERR_APPLY".into());
    let (report, record) = run_on(&mut fake, &compiled);
    assert_eq!(*outcome(&report, "accept ok"), Outcome::Pass);
    assert_eq!(
        *outcome(&report, "accept no"),
        mismatch("accepted", "refused: ERR_OFFLINE")
    );
    assert!(matches!(
        outcome(&report, "accept ghost"),
        Outcome::HarnessError(m) if m.contains("ghost was not submitted")
    ));
    assert_eq!(*outcome(&report, "invalid no"), Outcome::Pass);
    assert_eq!(
        *outcome(&report, "invalid ok"),
        mismatch("refused", "accepted")
    );
    assert_eq!(
        *outcome(&report, "code no"),
        mismatch("ERR_APPLY", "refused: ERR_OFFLINE")
    );
    assert_eq!(
        *outcome(&report, "code ok"),
        mismatch("ERR_APPLY", "accepted")
    );
    assert_eq!(*outcome(&report, "code other"), Outcome::Pass);
    let answers = &record.steps[0].submitted;
    assert_eq!(answers[0], ("ok".to_owned(), true, None));
    assert_eq!(
        answers[1],
        ("no".to_owned(), false, Some("ERR_OFFLINE".to_owned()))
    );
    assert!(
        record.steps[0]
            .outcomes
            .iter()
            .any(|(label, how)| label.ends_with("accept no") && how.starts_with("failed"))
    );
}

#[test]
fn forged_and_absent_look_in_the_chain_within_the_blocks_given() {
    let compiled = compiled(
        json!([
            {"at": {"height": 2}, "do": "submit", "node": 1,
             "check": {"forged": ["in"], "withinBlocks": 2}},
            {"at": {"afterBlocks": 0}, "check": {"forged": ["out"], "withinBlocks": 3, "absent": ["out"]}},
            {"at": {"afterBlocks": 0}, "check": {"absent": ["in", "unknown"], "forged": ["unknown"]}},
        ]),
        &[("in", 0), ("out", 0)],
    );
    let mut fake = Fake::new(1);
    fake.refuse.insert("out".into(), "ERR_APPLY".into());
    let (report, _) = run_on(&mut fake, &compiled);
    assert_eq!(*outcome(&report, "forged in"), Outcome::Pass);
    let forged_out = report
        .results
        .iter()
        .find(|c| c.label.contains("forged out"))
        .expect("case");
    assert_eq!(forged_out.outcome, mismatch("in the chain", "in no block"));
    // It looked through the whole window before it gave up.
    assert!(forged_out.height >= 5, "{}", forged_out.height);
    let absent: Vec<_> = report
        .results
        .iter()
        .filter(|c| c.label.contains("absent"))
        .collect();
    assert_eq!(absent[0].outcome, Outcome::Pass, "out is in no block");
    assert_eq!(absent[1].outcome, mismatch("in no block", "in the chain"));
    assert!(matches!(&absent[2].outcome, Outcome::HarnessError(m) if m.contains("unknown")));
}

#[test]
fn digest_and_invariants_are_the_backends_to_vouch_for_or_a_comparison_of_two_runs() {
    let compiled = compiled(
        json!([{
            "at": {"height": 3},
            "check": {"digest": true, "invariant": ["revertSymmetry", "restartEqual", "singleTip"]},
        }]),
        &[],
    );
    // A backend that says nothing: the checks of two runs are skipped, one tip is judged.
    let (report, _) = run_on(&mut Fake::new(2), &compiled);
    assert!(matches!(outcome(&report, "digest"), Outcome::Skipped(_)));
    assert!(matches!(
        outcome(&report, "revertsymmetry"),
        Outcome::Skipped(_)
    ));
    assert!(matches!(
        outcome(&report, "restartequal"),
        Outcome::Skipped(_)
    ));
    assert_eq!(*outcome(&report, "singletip"), Outcome::Pass);
    // Nodes on different tips.
    let mut behind = Fake::new(2);
    behind.lag.insert(2, 1);
    let (report, _) = run_on(&mut behind, &compiled);
    assert!(matches!(
        outcome(&report, "singletip"),
        Outcome::VerdictMismatch { oracle, .. } if oracle == "one tip"
    ));
    // A backend that vouches.
    let mut vouching = Fake::new(1);
    vouching.verdicts.insert("digest", Ok(()));
    vouching
        .verdicts
        .insert("revertSymmetry", Err("a line differs".into()));
    vouching.verdicts.insert("restartEqual", Ok(()));
    let (report, _) = run_on(&mut vouching, &compiled);
    assert_eq!(*outcome(&report, "digest"), Outcome::Pass);
    assert_eq!(
        *outcome(&report, "revertsymmetry"),
        mismatch("it holds", "a line differs")
    );
    assert_eq!(*outcome(&report, "restartequal"), Outcome::Pass);
}

fn wallet_pages(fake: &mut Fake, chain: &Chain, node: u32, alice: &str) {
    for (actor, balance) in [("genesis", "1000"), ("alice", alice), ("bob", "7")] {
        let address = &chain.person(actor).expect("actor").address;
        fake.page(
            node,
            &format!("/api/wallets/{address}"),
            json!({"data": {"balance": balance, "nonce": "2", "unrelated": node}}),
        );
    }
}

/// A list route's one page, as the API pages it.
fn one_page(rows: Value) -> Value {
    json!({"meta": {"count": 1, "next": null}, "data": rows})
}

#[test]
fn an_api_projection_is_equal_on_every_node_or_a_mismatch() {
    let compiled = compiled(
        json!([{
            "at": {"height": 2},
            "check": {"api": {"projection": "wallets"}},
        }, {
            "at": {"afterBlocks": 0},
            "check": {"api": {"projection": "wallets", "nodes": [1]}},
        }, {
            "at": {"afterBlocks": 0},
            "check": {"api": {"projection": "delegates"}},
        }, {
            "at": {"afterBlocks": 0},
            "check": {"api": {"projection": "supply"}},
        }, {
            "at": {"afterBlocks": 0},
            "check": {"api": {"projection": "weather"}},
        }]),
        &[],
    );
    let chain = Chain::new(&compiled.scenario).expect("chain");
    let mut fake = Fake::new(2);
    wallet_pages(&mut fake, &chain, 1, "5");
    wallet_pages(&mut fake, &chain, 2, "6");
    // The delegates differ only in what the projection removes: the version and productivity.
    for (node, version, productivity) in [(1, "1.0.0", 99.1), (2, "1.0.1", 98.0)] {
        fake.page(
            node,
            "/api/delegates?page=1&limit=100",
            one_page(json!([
                {"username": "genesis_1", "rank": 1, "version": version,
                 "votesReceived": {"votes": "5", "percent": 1, "voters": 1},
                 "blocks": {"produced": 3, "missed": node, "productivity": productivity}},
            ])),
        );
    }
    fake.page(
        1,
        "/api/blockchain",
        json!({"data": {"supply": "10", "burned": "0"}}),
    );
    fake.page(
        2,
        "/api/blockchain",
        json!({"data": {"supply": "11", "burned": "0"}}),
    );
    let (report, record) = run_on(&mut fake, &compiled);
    let cases: Vec<&Outcome> = report.results.iter().map(|c| &c.outcome).collect();
    // The wallets differ between the nodes (alice's balance); on one node they cannot.
    assert!(
        matches!(cases[0], Outcome::Mismatch(d) if !d.is_empty()),
        "{cases:?}"
    );
    assert_eq!(*cases[1], Outcome::Pass);
    assert_eq!(*cases[2], Outcome::Pass, "{cases:?}");
    assert!(matches!(cases[3], Outcome::Mismatch(_)), "supply differs");
    assert!(
        matches!(cases[4], Outcome::HarnessError(m) if m.contains("no projection is named weather"))
    );
    // The value read is the run's record: node 1's, without what the projection removes.
    let delegates = &record.steps[2].projections;
    assert_eq!(delegates[0].0, "step 3: api delegates");
    assert_eq!(
        delegates[0].1,
        json!([{"username": "genesis_1", "rank": 1,
                "votesReceived": {"votes": "5", "percent": 1, "voters": 1},
                "blocks": {"produced": 3}}])
    );
    // What is kept of a wallet is its balance and nonce: a difference elsewhere is not one.
    let mut equal = Fake::new(2);
    wallet_pages(&mut equal, &chain, 1, "5");
    wallet_pages(&mut equal, &chain, 2, "5");
    let (report, _) = run_on(&mut equal, &compiled);
    assert_eq!(report.results[0].outcome, Outcome::Pass);
}

#[test]
fn the_projections_read_the_routes_they_name_without_what_may_differ() {
    let compiled = compiled(json!([{"at": {"height": 2}}]), &[("fund", 0)]);
    let chain = Chain::new(&compiled.scenario).expect("chain");
    let alice = chain.person("alice").expect("alice").address.clone();
    let mut fake = Fake::new(1);
    fake.page(
        1,
        &format!("/api/wallets/{alice}"),
        json!({"data": {"balance": "5",
        "attributes": {"delegate": {"username": "a", "version": "1.0.0", "productivity": 1}}}}),
    );
    fake.page(
        1,
        "/api/transactions/id-fund",
        json!({"data": {"id": "id-fund", "confirmations": 3}}),
    );
    fake.page(
        1,
        "/api/blocks/4",
        json!({"data": {"height": 4, "confirmations": 1}}),
    );
    fake.page(
        1,
        "/api/rounds/1/delegates",
        json!({"data": [
        {"publicKey": "bb", "votes": "5"}, {"publicKey": "aa", "votes": "5"},
        {"publicKey": "cc", "votes": "10"}]}),
    );
    fake.page(
        1,
        "/api/transactions/unconfirmed?page=1&limit=100",
        one_page(json!([{"id": "b"}, {"id": "a"}])),
    );
    fake.page(
        1,
        "/api/blocks/missed?height.from=2&height.to=9&page=1&limit=100",
        one_page(json!([{"height": 3}])),
    );
    fake.page(
        1,
        "/api/node/syncing",
        json!({"data": {"syncing": false, "blocks": 0, "height": 4}}),
    );
    fake.page(
        1,
        "/api/node/configuration",
        json!({"data": {"core": {"version": "x"}, "nethash": "n"}}),
    );
    // Two pages, the first pointing at the second.
    fake.page(1, "/api/delegates/genesis_1/voters?page=1&limit=100",
        json!({"meta": {"next": "/delegates/genesis_1/voters?page=2&limit=100"}, "data": [{"address": "1"}]}));
    fake.page(
        1,
        "/api/delegates/genesis_1/voters?page=2&limit=100",
        one_page(json!([{"address": "2",
            "attributes": {"delegate": {"username": "d", "version": "1.0.0"}}}])),
    );
    let runner_chain = Chain::new(&compiled.scenario).expect("chain");
    let names = Names {
        chain: &runner_chain,
        actors: vec!["alice", "bob"],
        transactions: &compiled.transactions,
    };
    let read = |text: &str| projection::read(&fake, &names, 1, text).expect(text);
    assert_eq!(
        read("wallet(alice)"),
        json!({"data": {"balance": "5", "attributes": {"delegate": {"username": "a"}}}})
    );
    assert_eq!(read("tx(fund)"), json!({"data": {"id": "id-fund"}}));
    assert_eq!(read("block(4)"), json!({"data": {"height": 4}}));
    assert_eq!(
        read("round(1)")["data"],
        json!([{"publicKey": "cc", "votes": "10"}, {"publicKey": "aa", "votes": "5"},
               {"publicKey": "bb", "votes": "5"}])
    );
    assert_eq!(read("pool"), json!(["a", "b"]));
    assert_eq!(read("missed(2, 9)"), json!([{"height": 3}]));
    assert_eq!(
        read("status"),
        json!({"data": {"syncing": false, "height": 4}})
    );
    assert_eq!(
        read("config"),
        json!({"data": {"core": {}, "nethash": "n"}})
    );
    // A voter that is a delegate shows the version its node attested to this relay: node-local.
    assert_eq!(
        read("voters(genesis_1)"),
        json!([{"address": "1"}, {"address": "2", "attributes": {"delegate": {"username": "d"}}}])
    );
    // A route that answers another status is compared with its status.
    assert_eq!(read("tx(elsewhere)"), json!({"status": 404, "body": {}}));
    for bad in ["wallet", "delegates(1)", "missed(1)", "tx(a"] {
        assert!(projection::read(&fake, &names, 1, bad).is_err(), "{bad}");
    }
}

#[test]
fn a_projection_waits_for_the_nodes_to_stand_at_one_height() {
    let compiled = compiled(
        json!([{"at": {"height": 3}, "check": {"api": {"projection": "supply"}}}]),
        &[],
    );
    // Node 2 is a block behind until the network reaches height 4.
    let mut fake = Fake::new(2);
    fake.lag.insert(2, 1);
    fake.catch_up = Some(4);
    fake.page_everywhere("/api/blockchain", json!({"data": {"supply": "10"}}));
    let (report, _) = run_on(&mut fake, &compiled);
    assert_eq!(report.results[0].outcome, Outcome::Pass);
    assert_eq!(report.results[0].height, 4, "read once both stood at 4");
}

#[test]
fn a_projection_is_read_again_when_a_block_came_in_between() {
    // A node whose tip moves at every look never gives a settled read: the check is skipped.
    struct Restless(Fake);
    impl Backend for Restless {
        fn name(&self) -> &'static str {
            "restless"
        }
        fn nodes(&self) -> u32 {
            1
        }
        fn start(&mut self, n: u32, p: Part) -> Result<(), BackendError> {
            self.0.start(n, p)
        }
        fn stop(&mut self, n: u32, p: Part, k: bool) -> Result<(), BackendError> {
            self.0.stop(n, p, k)
        }
        fn restart(&mut self, n: u32, p: Part, m: RestartModes) -> Result<(), BackendError> {
            self.0.restart(n, p, m)
        }
        fn partition(&mut self, g: &[Vec<u32>]) -> Result<(), BackendError> {
            self.0.partition(g)
        }
        fn heal(&mut self) -> Result<(), BackendError> {
            self.0.heal()
        }
        fn api_url(&self, n: u32) -> Result<String, BackendError> {
            self.0.api_url(n)
        }
        fn archive(&mut self, d: &Path) -> Result<(), BackendError> {
            self.0.archive(d)
        }
        fn submit(
            &mut self,
            n: u32,
            v: Via,
            t: &[CompiledTx],
        ) -> Result<Vec<Submitted>, BackendError> {
            self.0.submit(n, v, t)
        }
        fn tip(&self, node: u32) -> Result<Tip, BackendError> {
            self.0.height.set(self.0.height.get() + 1);
            self.0.tip(node)
        }
        fn get(&self, node: u32, path: &str) -> Result<Answer, BackendError> {
            self.0.get(node, path)
        }
        fn wait(&mut self) -> Result<(), BackendError> {
            self.0.wait()
        }
    }
    let compiled = compiled(
        json!([{"at": {"height": 2}, "check": {"api": {"projection": "supply"}}}]),
        &[],
    );
    let mut fake = Fake::new(1);
    fake.page(1, "/api/blockchain", json!({"data": {"supply": "10"}}));
    let (report, _) = run(&compiled, &mut Restless(fake), Level::L3).expect("run");
    assert!(
        matches!(outcome(&report, "api supply"), Outcome::Skipped(m) if m.contains("never stood at one height")),
        "{:#?}",
        report.results
    );
}

#[test]
fn missed_slots_are_counted_for_the_delegates_of_the_forger() {
    let compiled = compiled(
        json!([
            {"at": {"height": 2}, "check": {"missed": {"node": 1, "atLeast": 2}}},
            {"at": {"afterBlocks": 0}, "check": {"missed": {"node": 1, "atLeast": 4}}},
        ]),
        &[],
    );
    // Without the forger's delegates every missed slot counts.
    let mut all = Fake::new(1);
    all.page(
        1,
        "/api/blocks/missed?limit=1",
        json!({"meta": {"totalCount": 3}}),
    );
    let (report, _) = run_on(&mut all, &compiled);
    assert_eq!(report.results[0].outcome, Outcome::Pass);
    assert_eq!(
        report.results[1].outcome,
        mismatch("at least 4 missed", "3")
    );
    // With them, the missed slots of each delegate are added.
    let mut own = Fake::new(1);
    own.delegates = vec!["genesis_1".into(), "genesis_2".into()];
    for name in ["genesis_1", "genesis_2"] {
        own.page(
            1,
            &format!("/api/delegates/{name}/blocks/missed?limit=1"),
            json!({"meta": {"totalCount": 2}}),
        );
    }
    let (report, _) = run_on(&mut own, &compiled);
    assert_eq!(report.results[0].outcome, Outcome::Pass);
    assert_eq!(report.results[1].outcome, Outcome::Pass);
    // A route that gives no count is the harness's error, not a count of none.
    let (report, _) = run_on(&mut Fake::new(1), &compiled);
    assert!(
        matches!(&report.results[0].outcome, Outcome::HarnessError(m) if m.contains("without a count")),
        "{:?}",
        report.results[0]
    );
}

#[test]
fn a_scenario_that_looks_past_five_rounds_is_refused_before_it_starts() {
    let far = |steps: Value| {
        let compiled = compiled(steps, &[]);
        let mut fake = Fake::new(1);
        let refused = run(&compiled, &mut fake, Level::L3).map(|_| ());
        (refused, fake.log.borrow().len(), fake.height.get())
    };
    // The devnet's round is 53 blocks: five rounds end at 265.
    let (ok, ..) = far(json!([{"at": {"round": 5, "position": "end"}, "check": {"digest": true}}]));
    assert!(ok.is_ok());
    for steps in [
        json!([{"at": {"height": 266}}]),
        json!([{"at": {"height": 260}, "do": "wait", "blocks": 6}]),
        json!([{"at": {"height": 260}, "check": {"forged": ["t"], "withinBlocks": 6}}]),
        json!([{"at": {"round": 6, "position": "start"}}]),
    ] {
        let (refused, actions, height) = far(steps.clone());
        let error = refused.expect_err("refused").to_string();
        assert!(error.contains("past the 5 rounds"), "{steps}: {error}");
        assert_eq!((actions, height), (0, 1), "nothing ran");
    }
}

const BLOCKS: &str = "/api/blocks?limit=100&page=1&orderBy=height:asc";

#[test]
fn the_supply_is_the_genesis_supply_plus_the_rewards_less_the_burns() {
    let compiled = compiled(
        json!([{"at": {"height": 3}, "check": {"invariant": ["supply"]}}]),
        &[],
    );
    let genesis: u64 = Chain::new(&compiled.scenario)
        .expect("chain")
        .supply
        .parse()
        .expect("supply");
    let blocks = json!({"data": [
        {"height": 1, "forged": {"reward": "0"}}, {"height": 2, "forged": {"reward": "10"}},
        {"height": 3, "forged": {"reward": 10}}, {"height": 4, "forged": {"reward": "10"}},
    ]});
    let run_with = |supply: u64, burned: u64| {
        let mut fake = Fake::new(1);
        fake.page(
            1,
            "/api/blockchain",
            json!({"data": {
                "supply": supply.to_string(), "burned": {"total": burned.to_string()},
                "block": {"height": 3},
            }}),
        );
        fake.page(1, BLOCKS, blocks.clone());
        run_on(&mut fake, &compiled).0
    };
    // Blocks above the tip are not counted; the burns are taken off.
    assert_eq!(
        run_with(genesis + 20 - 4, 4).results[0].outcome,
        Outcome::Pass
    );
    assert_eq!(
        run_with(genesis + 20, 4).results[0].outcome,
        mismatch(&(genesis + 16).to_string(), &(genesis + 20).to_string())
    );
    let mut bare = Fake::new(1);
    bare.page(1, "/api/blockchain", json!({"data": {"supply": "1"}}));
    let (report, _) = run_on(&mut bare, &compiled);
    assert!(
        matches!(&report.results[0].outcome, Outcome::HarnessError(m) if m.contains("no supply"))
    );
}

#[test]
fn every_block_is_forged_by_a_delegate_of_its_round() {
    let compiled = compiled(
        json!([{"at": {"height": 3}, "check": {"invariant": ["schedule"]}}]),
        &[],
    );
    let run_with = |generators: [&str; 2]| {
        let mut fake = Fake::new(1);
        fake.page(
            1,
            BLOCKS,
            json!({"data": [
                {"height": 1, "generator": {"publicKey": "genesis"}},
                {"height": 2, "generator": {"publicKey": generators[0]}},
                {"height": 3, "generatorPublicKey": generators[1]},
            ]}),
        );
        fake.page(
            1,
            "/api/rounds/1/delegates",
            json!({"data": [{"publicKey": "aa"}, {"publicKey": "bb"}]}),
        );
        run_on(&mut fake, &compiled).0.results.remove(0).outcome
    };
    assert_eq!(run_with(["aa", "bb"]), Outcome::Pass);
    assert_eq!(
        run_with(["aa", "cc"]),
        mismatch("a delegate of round 1", "cc at 3")
    );
    // After a missed slot a delegate forges a second slot of its round (at the secondary reward).
    assert_eq!(run_with(["aa", "aa"]), Outcome::Pass);
}

// The records and their comparison.

fn record(id: &str, backend: &str, tip: (u32, &str), answer: bool, digest: Outcome) -> Record {
    let mut step = StepRecord::new(
        0,
        "first",
        &Tip {
            height: tip.0,
            id: tip.1.into(),
        },
    );
    step.answered(&[Submitted {
        reference: "t".into(),
        accepted: answer,
        code: None,
        message: None,
    }]);
    step.checked("first: digest", &digest);
    Record::new(
        id,
        backend,
        vec![step],
        Some(Tip {
            height: tip.0,
            id: tip.1.into(),
        }),
    )
}

fn suite(dir: &Path, name: &str, record: &Record) {
    record.write(&dir.join(name)).expect("write");
    chain(dir, name, "g", "b2");
    steps(dir, name, true);
}

/// A run's scripted steps: one, that met its expectation or not.
fn steps(dir: &Path, name: &str, ok: bool) {
    let line = json!({"step": "scenario-s", "node": "all", "ok": ok});
    std::fs::write(dir.join(name).join("steps.jsonl"), format!("{line}\n")).expect("steps");
}

/// A run's chain as the devnet tooling leaves it: two nodes' archives (a reference run) or digest
/// files (a Heartwood run), each holding the genesis `genesis` and a block 2 (`second` on node 2).
fn chain(dir: &Path, name: &str, genesis: &str, second: &str) {
    let run = dir.join(name);
    for (node, id) in [("node1", "b2"), ("node2", second)] {
        if name.ends_with("-reference") {
            let blocks = run
                .join("archives")
                .join(node)
                .join("20261001T000000Z/blocks");
            std::fs::create_dir_all(&blocks).expect("archive");
            std::fs::write(
                blocks.join("000000001-000000002.jsonl"),
                format!(
                    "{{\"height\":1,\"id\":\"{genesis}\"}}\n{{\"height\":2,\"id\":\"{id}\"}}\n"
                ),
            )
            .expect("blocks");
        } else {
            let digests = run.join("nodes").join(node).join("difftest");
            std::fs::create_dir_all(&digests).expect("digests");
            let line = |h: u32, step: &str, id: &str| {
                json!({"h": h, "step": step, "id": id, "rw": "x", "b": id, "s": "s", "r": "r",
                       "m": "m", "w": "w", "sd": "sd", "cd": node})
                .to_string()
                    + "\n"
            };
            std::fs::write(
                digests.join("digest-0001.jsonl"),
                line(1, "genesis", genesis) + &line(2, "apply", id),
            )
            .expect("digest");
        }
    }
}

#[test]
fn a_record_reads_back_as_written() {
    let scratch = ScratchDir::new("scenario-record").expect("scratch");
    let written = record("one", "fake", (5, "b5"), true, Outcome::Pass);
    let path = written.write(scratch.path()).expect("write");
    assert_eq!(path, Record::path(scratch.path(), "one"));
    assert_eq!(Record::read(&path).expect("read"), written);
    let mut other = written.clone();
    other.steps[0].checked("first: skip", &Outcome::Skipped("no".into()));
    assert_eq!(other.steps[0].outcomes[1].1, "skipped: no");
    other.steps[0].checked("first: bad", &mismatch("a", "b"));
    assert!(other.steps[0].outcomes[2].1.starts_with("failed"));
}

/// `record` with the scenario's wallets read at its step.
fn with_wallets(mut record: Record, alice: &str) -> Record {
    record.steps[0].projections.push((
        "first: api wallets".into(),
        json!({"alice": {"balance": alice, "nonce": "1"}}),
    ));
    record
}

#[test]
fn a_suite_compares_the_pool_answers_the_checks_and_the_scenario_wallets() {
    let scratch = ScratchDir::new("scenario-compare").expect("scratch");
    let dir = scratch.path();
    let same = |backend, tip: (u32, &str), answer| {
        with_wallets(record("s", backend, tip, answer, Outcome::Pass), "5")
    };
    // Equal runs pass: two networks forge their own blocks, so their tips are not compared.
    suite(dir, "a-reference", &same("reference", (5, "x"), true));
    suite(dir, "a-heartwood", &same("heartwood", (6, "y"), true));
    // Another answer of the pool.
    suite(dir, "b-reference", &same("reference", (5, "x"), true));
    suite(dir, "b-heartwood", &same("heartwood", (5, "x"), false));
    // A check that came out otherwise.
    suite(dir, "c-reference", &same("reference", (5, "x"), true));
    suite(
        dir,
        "c-heartwood",
        &with_wallets(
            record("s", "heartwood", (5, "x"), true, mismatch("a", "b")),
            "5",
        ),
    );
    // Other balances of the scenario's wallets, and wallets one run did not read.
    suite(dir, "d-reference", &same("reference", (5, "x"), true));
    suite(
        dir,
        "d-heartwood",
        &with_wallets(record("s", "heartwood", (5, "x"), true, Outcome::Pass), "6"),
    );
    suite(dir, "e-reference", &same("reference", (5, "x"), true));
    suite(
        dir,
        "e-heartwood",
        &record("s", "heartwood", (5, "x"), true, Outcome::Pass),
    );
    let report = compare(dir, None, None).expect("compare");
    let of = |suite: &str| -> Vec<&Outcome> {
        report
            .results
            .iter()
            .filter(|c| c.file.starts_with(&format!("{suite}/")))
            .map(|c| &c.outcome)
            .collect()
    };
    assert!(
        of("a").iter().all(|o| **o == Outcome::Pass),
        "{:?}",
        of("a")
    );
    assert_eq!(
        of("a").len(),
        8,
        "answers, the check, the wallets, each run's steps, each run's chain and their genesis"
    );
    for (name, failures) in [("b", 1), ("c", 1), ("d", 1), ("e", 1)] {
        assert_eq!(
            of(name).iter().filter(|o| o.is_failure()).count(),
            failures,
            "{name}: {:?}",
            of(name)
        );
    }
    assert!(of("d").iter().any(|o| matches!(o, Outcome::Mismatch(_))));
    // One suite alone.
    let only = compare(dir, Some("a"), None).expect("compare");
    assert_eq!(only.files, 1);
    assert!(only.results.iter().all(|c| c.outcome == Outcome::Pass));
}

#[test]
fn a_suite_run_whose_nodes_hold_two_chains_fails_and_so_does_another_genesis() {
    let scratch = ScratchDir::new("scenario-compare-chains").expect("scratch");
    let dir = scratch.path();
    let run = |backend| record("s", backend, (5, "x"), true, Outcome::Pass);
    // Each run's own chain: two networks, so block 2 differs between the runs, not within one.
    suite(dir, "a-reference", &run("reference"));
    suite(dir, "a-heartwood", &run("heartwood"));
    chain(dir, "a-heartwood", "g", "b2");
    suite(dir, "b-reference", &run("reference"));
    suite(dir, "b-heartwood", &run("heartwood"));
    chain(dir, "b-heartwood", "g", "fork");
    suite(dir, "c-reference", &run("reference"));
    suite(dir, "c-heartwood", &run("heartwood"));
    chain(dir, "c-reference", "g", "fork");
    suite(dir, "d-reference", &run("reference"));
    suite(dir, "d-heartwood", &run("heartwood"));
    chain(dir, "d-heartwood", "another", "b2");
    let report = compare(dir, None, None).expect("compare");
    let failed = |suite: &str| -> Vec<String> {
        report
            .results
            .iter()
            .filter(|c| c.file.starts_with(&format!("{suite}/")) && c.outcome.is_failure())
            .map(|c| format!("{}: {:?}", c.label, c.outcome))
            .collect()
    };
    assert!(failed("a").is_empty(), "{:?}", failed("a"));
    for (suite, needle) in [
        ("b", "the heartwood run: one chain"),
        ("c", "the reference run: one chain"),
        ("d", "the genesis of both runs"),
    ] {
        let found = failed(suite);
        assert_eq!(found.len(), 1, "{suite}: {found:?}");
        assert!(found[0].contains(needle), "{suite}: {found:?}");
    }
    assert!(failed("b")[0].contains("height 2: node2 differs from node1"));
    // A run without a chain to read fails its case.
    std::fs::remove_dir_all(dir.join("a-heartwood/nodes")).expect("remove");
    let bare = compare(dir, Some("a"), None).expect("compare");
    assert!(
        bare.results
            .iter()
            .any(|c| c.label.starts_with("the heartwood run") && c.outcome.is_failure()),
        "{:#?}",
        bare.results
    );
}

#[test]
fn a_suite_of_one_run_or_none_is_the_harness_error() {
    let scratch = ScratchDir::new("scenario-compare-missing").expect("scratch");
    let dir = scratch.path();
    let empty = compare(dir, None, None).expect("compare");
    assert!(matches!(
        &empty.results[0].outcome,
        Outcome::HarnessError(m) if m.contains("holds no suite")
    ));
    suite(
        dir,
        "a-reference",
        &record("s", "reference", (5, "x"), true, Outcome::Pass),
    );
    let lonely = compare(dir, None, None).expect("compare");
    assert!(matches!(
        &lonely.results[0].outcome,
        Outcome::HarnessError(m) if m.contains("only one of its two runs")
    ));
    // A scenario one run lacks.
    suite(
        dir,
        "a-heartwood",
        &record("t", "heartwood", (5, "x"), true, Outcome::Pass),
    );
    let unpaired = compare(dir, None, None).expect("compare");
    assert_eq!(unpaired.failed(), 2, "{:#?}", unpaired.results);
    let elsewhere = compare(dir, Some("z"), None).expect("compare");
    assert!(elsewhere.results[0].outcome.is_failure());
}

#[test]
fn a_check_alike_on_both_runs_passes_only_when_it_passed() {
    let scratch = ScratchDir::new("scenario-compare-alike").expect("scratch");
    let dir = scratch.path();
    let both = |name: &str, outcome: Outcome| {
        suite(
            dir,
            &format!("{name}-reference"),
            &record("s", "reference", (5, "x"), true, outcome.clone()),
        );
        suite(
            dir,
            &format!("{name}-heartwood"),
            &record("s", "heartwood", (5, "x"), true, outcome),
        );
    };
    both("passed", Outcome::Pass);
    both("failed", mismatch("a", "b"));
    both("skipped", Outcome::Skipped("not modelled".into()));
    let live = "a live network's digests are judged against the reference when the suite's runs are compared";
    both("digest", Outcome::Skipped(live.into()));
    let report = compare(dir, None, None).expect("compare");
    let digest_case = |suite: &str| -> Outcome {
        report
            .results
            .iter()
            .find(|c| c.file == format!("{suite}/s") && c.label == "first: digest")
            .map(|c| c.outcome.clone())
            .expect("the digest check's case")
    };
    assert_eq!(digest_case("passed"), Outcome::Pass);
    assert!(
        digest_case("failed").is_failure(),
        "{:?}",
        digest_case("failed")
    );
    assert_eq!(
        digest_case("skipped"),
        Outcome::Skipped("not modelled".into())
    );
    // Without the chains' judgement a live digest check stays skipped, so a strict run fails it.
    assert!(matches!(digest_case("digest"), Outcome::Skipped(_)));
    assert!(!report.passes(true));
}

#[test]
fn a_scenario_without_a_record_on_either_side_and_a_failed_step_fail() {
    let scratch = ScratchDir::new("scenario-compare-planned").expect("scratch");
    let dir = scratch.path();
    for side in ["reference", "heartwood"] {
        let name = format!("a-{side}");
        suite(
            dir,
            &name,
            &record("s", side, (5, "x"), true, Outcome::Pass),
        );
        let scenarios = dir.join(&name).join("scenarios");
        std::fs::create_dir_all(&scenarios).expect("scenarios");
        for (file, id, levels) in [
            ("s.json", "s", "L3"),
            ("gone.json", "gone", "L3"),
            ("l2.json", "l2-only", "L2"),
        ] {
            let scenario = json!({
                "id": id, "title": "t", "levels": [levels], "network": "devnet",
                "genesisSeed": "seed", "steps": []
            });
            std::fs::write(scenarios.join(file), scenario.to_string()).expect("scenario");
        }
    }
    steps(dir, "a-heartwood", false);
    let report = compare(dir, None, None).expect("compare");
    let failed: Vec<String> = report
        .results
        .iter()
        .filter(|c| c.outcome.is_failure())
        .map(|c| format!("{}: {}: {:?}", c.file, c.label, c.outcome))
        .collect();
    assert_eq!(failed.len(), 2, "{failed:#?}");
    assert!(
        failed
            .iter()
            .any(|f| f.starts_with("a/gone: both runs") && f.contains("neither run")),
        "{failed:#?}"
    );
    assert!(
        failed
            .iter()
            .any(|f| f.contains("the heartwood run's scripted steps")
                && f.contains("scenario-s (all)")),
        "{failed:#?}"
    );
}

#[test]
fn the_pools_refusals_are_compared_up_to_the_numbers_of_each_network() {
    let scratch = ScratchDir::new("scenario-compare-refusals").expect("scratch");
    let dir = scratch.path();
    let refused = |backend: &str, message: &str| {
        let mut r = record("s", backend, (5, "x"), false, Outcome::Pass);
        r.steps[0].answered(&[Submitted {
            reference: "t".into(),
            accepted: false,
            code: Some("ERR_APPLY".into()),
            message: Some(message.into()),
        }]);
        r
    };
    suite(
        dir,
        "same-reference",
        &refused("reference", "5 more blocks must be produced"),
    );
    suite(
        dir,
        "same-heartwood",
        &refused("heartwood", "7 more blocks must be produced"),
    );
    suite(
        dir,
        "other-reference",
        &refused("reference", "Insufficient balance in the wallet"),
    );
    suite(
        dir,
        "other-heartwood",
        &refused("heartwood", "The nonce is not the next one"),
    );
    let report = compare(dir, None, None).expect("compare");
    let answers = |suite: &str| -> Outcome {
        report
            .results
            .iter()
            .find(|c| c.file == format!("{suite}/s") && c.label == "first: pool answers")
            .map(|c| c.outcome.clone())
            .expect("the answers' case")
    };
    assert_eq!(answers("same"), Outcome::Pass);
    assert!(answers("other").is_failure(), "{:?}", answers("other"));
}

#[test]
fn a_refusal_without_its_reason_on_either_run_is_not_judged_and_a_name_s_digits_count() {
    let scratch = ScratchDir::new("scenario-compare-reasons").expect("scratch");
    let dir = scratch.path();
    let refused = |backend: &str, message: Option<&str>| {
        let mut r = record("s", backend, (5, "x"), false, Outcome::Pass);
        r.steps[0].answered(&[Submitted {
            reference: "t".into(),
            accepted: false,
            code: Some("ERR_APPLY".into()),
            message: message.map(str::to_owned),
        }]);
        r
    };
    let offline = "genesis_44 is not operating a node on the network";
    // One side kept no reason (an answer without a message, or a record from before reasons were
    // kept): the two refusals cannot be held to each other.
    suite(
        dir,
        "silent-reference",
        &refused("reference", Some(offline)),
    );
    suite(dir, "silent-heartwood", &refused("heartwood", None));
    // Another delegate's name differs only in its digits, which are part of the name.
    suite(dir, "named-reference", &refused("reference", Some(offline)));
    suite(
        dir,
        "named-heartwood",
        &refused(
            "heartwood",
            Some("genesis_45 is not operating a node on the network"),
        ),
    );
    let report = compare(dir, None, None).expect("compare");
    let answers = |suite: &str| -> Outcome {
        report
            .results
            .iter()
            .find(|c| c.file == format!("{suite}/s") && c.label == "first: pool answers")
            .map(|c| c.outcome.clone())
            .expect("the answers' case")
    };
    assert!(
        matches!(answers("silent"), Outcome::Skipped(_)),
        "{:?}",
        answers("silent")
    );
    assert!(answers("named").is_failure(), "{:?}", answers("named"));
}

#[test]
fn a_live_answer_names_the_offline_refusal_by_its_handler_s_code_and_keeps_the_reason() {
    use heartwood_difftest::scenario::backends::http::verdict;
    let body = json!({
        "data": {"accept": ["a"], "broadcast": [], "excess": [], "invalid": ["v", "n"]},
        "errors": {
            "v": [{"type": "ERR_APPLY", "message": "Vote v cannot be applied: genesis_44 is not operating a node on the network"}],
            "n": [{"type": "ERR_APPLY", "message": "Transfer n cannot be applied: Cannot apply a transaction with nonce 3"}]
        }
    });
    assert_eq!(verdict(&body, "a"), Some((true, None, None)));
    assert_eq!(
        verdict(&body, "v"),
        Some((
            false,
            Some("ERR_OFFLINE".into()),
            Some("genesis_44 is not operating a node on the network".into())
        ))
    );
    assert_eq!(
        verdict(&body, "n"),
        Some((
            false,
            Some("ERR_APPLY".into()),
            Some("Cannot apply a transaction with nonce 3".into())
        ))
    );
    // An id the answer does not name is no refusal: the submit fails on it.
    assert_eq!(verdict(&body, "unnamed"), None);
}

/// Copy the directory `from` to `to`, recursively.
fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("dir");
    for entry in std::fs::read_dir(from).expect("read dir") {
        let path = entry.expect("entry").path();
        let target = to.join(path.file_name().expect("name"));
        if path.is_dir() {
            copy_tree(&path, &target);
        } else {
            std::fs::copy(&path, &target).expect("copy");
        }
    }
}

#[test]
fn a_heartwood_suite_run_s_chain_is_judged_against_the_reference_s_lines() {
    use heartwood_difftest::scenario::judge::{Side, judge};
    let recorded = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/live/core");
    let scratch = ScratchDir::new("scenario-judge").expect("scratch");
    let run = scratch.path().join("transactions-heartwood");
    let root = scratch.path().join("corpus");
    copy_tree(&recorded, &run);
    // A suite run's control node keeps its block files in its own directory, and the network's
    // crypto files beside the run; the chain was judged before, so no oracle is asked for.
    copy_tree(&run.join("nodes/node1"), &run.join("node1"));
    copy_tree(&recorded.join("corpus/crypto"), &run.join("network/crypto"));
    copy_tree(
        &recorded.join("corpus"),
        &root.join("suite-transactions-heartwood-1"),
    );
    let no_oracle = || -> heartwood_difftest::Result<_> {
        Err(heartwood_difftest::Error::Usage("judged already".into()))
    };
    let held = judge(&run, Side::Heartwood, "transactions", &root, &no_oracle);
    assert!(held.holds, "{:#?}", held.cases);
    assert!(
        held.cases
            .iter()
            .any(|(label, _)| label.contains("node2's final chain equals the judge's")),
        "{:#?}",
        held.cases
    );
    // One node's state at one height differs from the judge's: the chain is not held.
    let digest = run.join("nodes/node2/difftest/digest-0001.jsonl");
    let text = std::fs::read_to_string(&digest).expect("digest");
    let mut lines: Vec<Value> = text
        .lines()
        .map(|l| serde_json::from_str(l).expect("line"))
        .collect();
    let at = lines
        .iter()
        .position(|l| l["step"] == "apply")
        .expect("an apply line");
    lines[at]["s"] = json!("not the judge's");
    let edited: String = lines.iter().map(|l| format!("{l}\n")).collect();
    std::fs::write(&digest, edited).expect("edit");
    let broken = judge(&run, Side::Heartwood, "transactions", &root, &no_oracle);
    assert!(!broken.holds);
    assert!(
        broken
            .cases
            .iter()
            .any(|(label, outcome)| label.contains("node2") && outcome.is_failure()),
        "{:#?}",
        broken.cases
    );
}

#[test]
fn a_heartwood_suite_run_counts_only_for_the_relay_built_from_this_tree_s_product_code() {
    use heartwood_difftest::scenario::judge::built_from;
    let scratch = ScratchDir::new("scenario-built-from").expect("scratch");
    let tree = scratch.path().join("tree");
    let git = |args: &[&str]| -> String {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(&tree)
            .args([
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
            ])
            .args(args)
            .output()
            .expect("git");
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    };
    let write = |path: &str, text: &str| {
        let full = tree.join(path);
        std::fs::create_dir_all(full.parent().expect("parent")).expect("dirs");
        std::fs::write(full, text).expect("write");
    };
    std::fs::create_dir_all(&tree).expect("tree");
    git(&["init", "-q", "-b", "main"]);
    write("packages/node/src/lib.rs", "pub fn f() {}\n");
    write("tools/harness.rs", "fn h() {}\n");
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "first"]);
    let built = git(&["rev-parse", "HEAD"]);
    let run = scratch.path().join("transactions-heartwood");
    std::fs::create_dir_all(&run).expect("run");
    let manifest = |commit: &str, clean: bool| {
        let run_json = json!({"format": "heartwood-live-run/1", "run": "suite-transactions",
            "network": "n", "seed": "1", "nodes": [], "worktreeCommit": commit,
            "worktreeClean": clean});
        std::fs::write(run.join("run.json"), run_json.to_string()).expect("run.json");
    };
    manifest(&built, true);
    assert_eq!(built_from(&run, &tree).1, Outcome::Pass);
    // Only the harness and the documents changed since: the same relay.
    write("tools/harness.rs", "fn h() { let _ = 1; }\n");
    write("packages/node/README.md", "notes\n");
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "harness"]);
    assert_eq!(built_from(&run, &tree).1, Outcome::Pass);
    // The product changed: the run tested another relay.
    write("packages/node/src/lib.rs", "pub fn f() { let _ = 2; }\n");
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "product"]);
    assert!(built_from(&run, &tree).1.is_failure());
    manifest(&git(&["rev-parse", "HEAD"]), false);
    assert!(built_from(&run, &tree).1.is_failure());
    manifest(&"0".repeat(40), true);
    assert!(built_from(&run, &tree).1.is_failure());
}

// The backends.

fn difftest(args: &[&str], oracle: &[String]) -> (bool, String) {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_difftest"))
        .args(args)
        .args(oracle)
        .output()
        .expect("difftest runs");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out.status.success(), text)
}

#[test]
fn the_heartwood_backend_calls_the_devnet_commands_and_needs_a_manifest() {
    let root = Path::new("/net");
    assert_eq!(
        heartwood::devnet_args(root, "start-node", 3, Part::Node, false),
        ["devnet", "--dir", "/net", "start-node", "3"]
    );
    assert_eq!(
        heartwood::devnet_args(root, "stop-node", 2, Part::Forger, true),
        [
            "devnet",
            "--dir",
            "/net",
            "stop-node",
            "2",
            "--role",
            "forger",
            "--kill"
        ]
    );
    assert_eq!(
        heartwood::devnet_args(root, "restart-node", 1, Part::Relay, false)[5..],
        ["--role", "relay"]
    );
    let scratch = ScratchDir::new("scenario-heartwood").expect("scratch");
    let error = heartwood::Heartwood::open(scratch.path())
        .err()
        .expect("no manifest");
    assert!(
        matches!(&error, BackendError::Failed { action, .. } if action.contains("manifest")),
        "{error}"
    );
    // A restart by a replay places the reference's marker where the relay reads it at its start.
    let node = heartwood_cli::devnet::ManifestNode {
        index: 2,
        address: "127.24.0.2".parse().expect("address"),
        forger_address: None,
        forger_hosts: Vec::new(),
        delegates: Vec::new(),
        dir: scratch.path().join("node2"),
        core: false,
    };
    let marker = heartwood::request_replay(&node).expect("the marker");
    assert_eq!(
        marker,
        scratch.path().join("node2/temp/clear-saved-states.lock")
    );
    assert!(marker.is_file());
}

#[test]
fn a_run_without_a_devnet_says_so_before_it_starts_anything() {
    // Files that cannot run are cases of the run, and the oracle is not needed for them.
    let scratch = ScratchDir::new("scenario-files").expect("scratch");
    let dir = scratch.path();
    std::fs::write(dir.join("broken.json"), r#"{"id": "x", "bogus": 1}"#).expect("write");
    let l3 = std::fs::read_to_string(fixtures().join("zero-vote-ties.json"))
        .expect("fixture")
        .replace("\"L2\"", "\"L3\"");
    std::fs::write(dir.join("later.json"), l3).expect("write");
    let d = dir.to_str().expect("utf-8");
    let (ok, text) = difftest(
        &[
            "scenario",
            "run",
            "--backend",
            "lockstep",
            "--level",
            "l2",
            d,
        ],
        &["--solar-dir".into(), "/nonexistent".into()],
    );
    assert!(!ok, "{text}");
    assert!(text.contains("unknown field `bogus`"), "{text}");
    // A scenario of another level is named, not counted: its level is another record's.
    assert!(
        text.contains("left to the records of their other levels: zero-vote-ties"),
        "{text}"
    );
    assert!(text.contains("1 records in 2 files"), "{text}");
    // A run with no scenario at its level at all is the harness's error, not an empty pass.
    std::fs::remove_file(dir.join("broken.json")).expect("remove");
    let (ok, text) = difftest(
        &[
            "scenario",
            "run",
            "--backend",
            "lockstep",
            "--level",
            "l2",
            d,
            "--strict",
        ],
        &["--solar-dir".into(), "/nonexistent".into()],
    );
    assert!(!ok, "{text}");
    assert!(text.contains("no scenario of"), "{text}");
}

#[test]
fn a_request_the_rate_limit_refused_is_sent_again_after_a_pause() {
    // The API refuses the first two requests with 429, then answers.
    let listener = TcpListener::bind("127.24.0.201:0").expect("bind");
    let address = listener.local_addr().expect("address");
    std::thread::spawn(move || {
        for (n, stream) in listener.incoming().enumerate() {
            let Ok(mut stream) = stream else { return };
            let mut buffer = [0_u8; 4096];
            let _ = stream.read(&mut buffer).expect("read");
            let (status, body) = if n < 2 {
                (
                    429,
                    json!({"statusCode": 429, "error": "Too Many Requests"}),
                )
            } else {
                (200, json!({"data": {"block": {"height": 9, "id": "b9"}}}))
            };
            let body = body.to_string();
            let reply = format!(
                "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(reply.as_bytes()).expect("write");
        }
    });
    let started = std::time::Instant::now();
    let tip = scenario::backends::http::tip(&format!("http://{address}")).expect("the tip");
    assert_eq!(
        tip,
        Tip {
            height: 9,
            id: "b9".into()
        }
    );
    assert!(
        started.elapsed() >= std::time::Duration::from_millis(750),
        "two pauses"
    );
}

/// A stub API of a node: a tip, and a verdict for every transaction of a `POST`.
fn stub_api() -> String {
    let listener = TcpListener::bind("127.24.0.200:0").expect("bind");
    let address = listener.local_addr().expect("address");
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            let mut request = Vec::new();
            let mut buffer = [0_u8; 1024];
            let body_at = loop {
                let n = stream.read(&mut buffer).expect("read");
                request.extend_from_slice(&buffer[..n]);
                if let Some(at) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                    break at + 4;
                }
                assert!(n > 0, "the request ends before its head does");
            };
            let head = String::from_utf8_lossy(&request[..body_at]).to_lowercase();
            let wanted = head
                .split("content-length: ")
                .nth(1)
                .and_then(|r| r.split("\r\n").next())
                .and_then(|n| n.parse::<usize>().ok())
                .unwrap_or(0);
            while request.len() < body_at + wanted {
                let n = stream.read(&mut buffer).expect("read");
                request.extend_from_slice(&buffer[..n]);
            }
            let line = head.lines().next().unwrap_or_default().to_owned();
            let (status, body) = if line.starts_with("get /api/blockchain") {
                (200, json!({"data": {"block": {"height": 7, "id": "abc"}}}))
            } else if line.starts_with("post /api/transactions") {
                let sent = String::from_utf8_lossy(&request[body_at..]).into_owned();
                let sent: Value = serde_json::from_str(&sent).expect("a JSON body");
                let nonces: Vec<&Value> = sent["transactions"]
                    .as_array()
                    .expect("a list")
                    .iter()
                    .map(|t| &t["nonce"])
                    .collect();
                assert_eq!(nonces, [&json!("1"), &json!("2")], "{sent}");
                (
                    422,
                    json!({
                        "data": {"accept": ["id-t1"], "broadcast": ["id-t1"], "invalid": ["id-t2"]},
                        "errors": {"id-t2": [{"type": "ERR_APPLY", "message": "no"}]},
                    }),
                )
            } else {
                (404, json!({}))
            };
            let body = body.to_string();
            let reply = format!(
                "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(reply.as_bytes()).expect("write");
        }
    });
    format!("http://{address}")
}

/// A crypto file of the devnet profile's chain for the scenario `scenario_json` gives.
fn chain_crypto(name: &str) -> Value {
    let scenario: Scenario = serde_json::from_value(scenario_json(json!([]))).expect("scenario");
    Chain::new(&scenario).expect("chain").crypto[name].clone()
}

/// The hex of a signed burn with `nonce`, under the rules `crypto` holds.
fn burn(crypto: &Value, nonce: u64) -> String {
    let network = Network::load(&crypto["network"].to_string()).expect("network");
    let milestones = Milestones::load(&crypto["milestones"].to_string(), &network).expect("rules");
    let keys = KeyPair::from_passphrase("scenario").expect("keys");
    let tx = TransactionBuilder::burn(1)
        .nonce(nonce)
        .build(&keys, Aux::fixed([7; 32]), milestones.at(1))
        .expect("built");
    hex::encode(tx.bytes())
}

#[test]
fn the_external_backend_calls_its_executable_once_per_action() {
    use std::os::unix::fs::PermissionsExt;
    let scratch = ScratchDir::new("scenario-external").expect("scratch");
    let dir = scratch.path();
    let log = dir.join("calls.log");
    let api = stub_api();
    let script = dir.join("backend.sh");
    std::fs::write(
        &script,
        format!(
            "#!/bin/bash\necho \"$*\" >> {log}\ncase \"$1\" in\n\
             info) echo '{{\"nodes\": 2}}' ;;\n\
             api-url) echo '{api}' ;;\n\
             heal) exit 3 ;;\n\
             archive) echo 'disk full' >&2; exit 1 ;;\n\
             post-transactions) cat \"$4\" >> {log}; echo '{{\"id\":\"id-t1\",\"accepted\":true}}'; echo '{{\"id\":\"id-t2\",\"accepted\":false}}' ;;\n\
             esac\n",
            log = log.display(),
        ),
    )
    .expect("script");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).expect("mode");
    let root = dir.join("net");
    let mut backend = external::External::open(&script, &root).expect("open");
    assert_eq!(backend.nodes(), 2);
    assert_eq!(backend.name(), "external");
    backend.start(1, Part::Relay).expect("start");
    backend.stop(2, Part::Forger, true).expect("stop");
    backend.stop(2, Part::Node, false).expect("stop");
    let modes: RestartModes =
        serde_json::from_value(json!({"reference": "rebuild", "heartwood": "replay"}))
            .expect("modes");
    backend.restart(1, Part::Node, modes).expect("restart");
    backend
        .partition(&[vec![1, 2], vec![3]])
        .expect("partition");
    // A verb the network does not offer, and one that fails with its reason.
    assert!(matches!(
        backend.heal(),
        Err(BackendError::Unsupported {
            backend: "external",
            ..
        })
    ));
    assert!(matches!(
        backend.archive(dir),
        Err(BackendError::Failed { message, .. }) if message == "disk full"
    ));
    // The tip and the API submit go to the address the executable gave.
    assert_eq!(backend.api_url(1).expect("url"), api);
    assert_eq!(
        backend.tip(1).expect("tip"),
        Tip {
            height: 7,
            id: "abc".into()
        }
    );
    let rules =
        json!({"network": chain_crypto("network"), "milestones": chain_crypto("milestones")});
    backend.rules(&rules).expect("rules");
    let txs: Vec<CompiledTx> = ["t1", "t2"]
        .iter()
        .zip(1..)
        .map(|(r, nonce)| CompiledTx {
            reference: (*r).into(),
            step: 0,
            height: 3,
            id: format!("id-{r}"),
            bytes: burn(&rules, nonce),
        })
        .collect();
    let answers = backend.submit(1, Via::Api, &txs).expect("submit");
    assert_eq!(
        answers,
        [
            Submitted {
                reference: "t1".into(),
                accepted: true,
                code: None,
                message: None,
            },
            Submitted {
                reference: "t2".into(),
                accepted: false,
                code: Some("ERR_APPLY".into()),
                message: Some("no".into()),
            },
        ]
    );
    // The peer-to-peer way in hands the executable a file of the transactions.
    let answers = backend.submit(2, Via::P2p, &txs).expect("submit");
    assert!(answers[0].accepted && !answers[1].accepted);
    let calls = std::fs::read_to_string(&log).expect("log");
    let r = root.display();
    for want in [
        format!("info {r}"),
        format!("start {r} 1 relay"),
        format!("stop {r} 2 forger --kill"),
        format!("stop {r} 2 node\n"),
        format!("restart {r} 1 node --reference rebuild --heartwood replay"),
        format!("partition {r} 1,2 3"),
        format!("archive {r} {}", dir.display()),
        format!("post-transactions {r} 2 "),
        format!("{}\n{}\n", txs[0].bytes, txs[1].bytes),
    ] {
        assert!(calls.contains(&want), "{want:?} in\n{calls}");
    }
    // An executable that is not there is the harness's failure.
    let missing = external::External::open(&dir.join("absent.sh"), &root);
    assert!(matches!(missing, Err(BackendError::Failed { .. })));
}

#[test]
fn the_restart_verb_names_the_modes_only_when_they_are_not_the_default() {
    let modes = |v: Value| -> RestartModes { serde_json::from_value(v).expect("modes") };
    assert_eq!(
        external::restart_args(3, Part::Relay, RestartModes::default()),
        ["3", "relay"]
    );
    assert_eq!(
        external::restart_args(3, Part::Forger, modes(json!({"heartwood": "replay"}))),
        ["3", "forger", "--heartwood", "replay"]
    );
    assert_eq!(
        external::restart_args(1, Part::Node, modes(json!({"reference": "rebuild"}))),
        ["1", "node", "--reference", "rebuild"]
    );
}

// Integration: the real reference.

fn oracle_flags() -> Option<Vec<String>> {
    if let Ok(dir) = std::env::var("DIFFTEST_TEST_SOLAR_DIR") {
        return Some(vec!["--solar-dir".into(), dir]);
    }
    std::env::var("DIFFTEST_TEST_IMAGE")
        .ok()
        .map(|image| vec!["--image".into(), image])
}

/// The oracle of the integration test `test`; `None` when the run skips it on purpose
/// (`DIFFTEST_TEST_NO_ORACLE`). Without either, the test fails: it must not pass unrun.
fn oracle_or_skip(test: &str) -> Option<Vec<String>> {
    if let Some(flags) = oracle_flags() {
        return Some(flags);
    }
    assert!(
        std::env::var_os("DIFFTEST_TEST_NO_ORACLE").is_some(),
        "{test}: no test oracle configured: set DIFFTEST_TEST_IMAGE or DIFFTEST_TEST_SOLAR_DIR \
         (make test-integration sets the image when it is built), or DIFFTEST_TEST_NO_ORACLE to \
         skip the test"
    );
    eprintln!("{test}: skipped (DIFFTEST_TEST_NO_ORACLE)");
    None
}

fn lockstep(file: &Path, oracle: &[String]) -> (bool, String) {
    let f = file.to_str().expect("utf-8");
    difftest(
        &[
            "scenario",
            "run",
            "--backend",
            "lockstep",
            "--level",
            "l2",
            f,
            "--strict",
        ],
        oracle,
    )
}

#[test]
#[ignore = "integration"]
fn a_scenario_compiles_to_the_same_bytes_twice_with_the_fee_floor_and_the_nonces_asked() {
    let Some(oracle) = oracle_or_skip("the compiler") else {
        return;
    };
    let scratch = ScratchDir::new("scenario-compile").expect("scratch");
    let file = fixtures().join("zero-vote-ties.json");
    let compile = |name: &str| -> Compiled {
        let out = scratch.path().join(name);
        let (ok, text) = difftest(
            &[
                "scenario",
                "compile",
                file.to_str().expect("utf-8"),
                "--out",
                out.to_str().expect("utf-8"),
            ],
            &oracle,
        );
        assert!(ok, "{text}");
        serde_json::from_str(&std::fs::read_to_string(out).expect("out")).expect("compiled")
    };
    let first = compile("one.json");
    assert_eq!(first, compile("two.json"), "the same bytes and ids");
    let refs: Vec<&str> = first
        .transactions
        .iter()
        .map(|t| t.reference.as_str())
        .collect();
    assert_eq!(
        refs,
        [
            "fund",
            "a-tenth",
            "b-ninth",
            "c-split",
            "d-moved",
            "nobody",
            "a9",
            "a09",
            "both-names"
        ]
    );
    let ids: HashSet<&str> = first.transactions.iter().map(|t| t.id.as_str()).collect();
    assert_eq!(ids.len(), 9, "nine ids");
    // Planned after their steps: the fund at height 2 goes into block 3.
    assert_eq!(first.transactions[0].height, 3);
    // Each one decodes by the rules of its planned height, with the nonce asked and the dynamic
    // fee floor for its type and length.
    let scenario = scenario::load(&file).expect("scenario");
    let chain = Chain::new(&scenario).expect("chain");
    let network = Network::load(&chain.crypto["network"].to_string()).expect("network");
    let milestones =
        Milestones::load(&chain.crypto["milestones"].to_string(), &network).expect("milestones");
    let decoded = |reference: &str| {
        let tx = first
            .transactions
            .iter()
            .find(|t| t.reference == reference)
            .expect("compiled");
        let params = milestones.at(tx.height);
        let bytes = hex::decode(&tx.bytes).expect("hex");
        let decoded = Transaction::decode_trusted(&bytes, params).expect("decodes");
        let floor = heartwood_transactions::minimum_fee(&decoded, params);
        (decoded.data().nonce, i128::from(decoded.data().fee), floor)
    };
    for tx in &first.transactions {
        let (_, fee, floor) = decoded(&tx.reference);
        assert!(floor > 0, "{}: a floor above nothing", tx.reference);
        if tx.reference == "b-ninth" {
            assert_eq!(fee, 30_000_000, "the fee the file gives");
            assert_ne!(fee, floor);
        } else {
            assert_eq!(fee, floor, "{} is built at the floor", tx.reference);
        }
    }
    // alice's first vote is her nonce 1 and her refused one 2; a genesis delegate signed nonces
    // 1 and 2 in the genesis block, so its next are 3 and 4.
    let nonces: Vec<u64> = ["a-tenth", "nobody", "d-moved", "both-names"]
        .into_iter()
        .map(|r| decoded(r).0)
        .collect();
    assert_eq!(nonces, [1, 2, 3, 4]);
}

#[test]
#[ignore = "integration"]
fn zero_vote_ties_and_numeric_names_run_on_the_lockstep_chain() {
    let Some(oracle) = oracle_or_skip("zero-vote-ties") else {
        return;
    };
    let (ok, text) = lockstep(&fixtures().join("zero-vote-ties.json"), &oracle);
    assert!(ok, "{text}");
    assert!(text.contains("0 failed"), "{text}");
}

#[test]
#[ignore = "integration"]
fn a_milestone_activation_runs_on_the_lockstep_chain() {
    let Some(oracle) = oracle_or_skip("milestone-activation") else {
        return;
    };
    let (ok, text) = lockstep(&fixtures().join("milestone-activation.json"), &oracle);
    assert!(ok, "{text}");
    assert!(text.contains("0 failed"), "{text}");
}

#[test]
#[ignore = "integration"]
fn a_reorg_across_a_round_end_with_reverted_votes_runs_on_the_lockstep_chain() {
    let Some(oracle) = oracle_or_skip("reorg-reverts-votes") else {
        return;
    };
    let (ok, text) = lockstep(&fixtures().join("reorg-reverts-votes.json"), &oracle);
    assert!(ok, "{text}");
    assert!(text.contains("0 failed"), "{text}");
}

#[test]
#[ignore = "integration"]
fn the_lockstep_backend_fails_what_the_chain_does_not_do_and_refuses_what_it_cannot() {
    let Some(oracle) = oracle_or_skip("lockstep refusals") else {
        return;
    };
    let scratch = ScratchDir::new("scenario-lockstep").expect("scratch");
    let text = std::fs::read_to_string(fixtures().join("zero-vote-ties.json")).expect("fixture");
    let write = |name: &str, text: String| {
        let path = scratch.path().join(name);
        std::fs::write(&path, text).expect("write");
        path
    };
    // The refusal code the pools give is what the file must say.
    let (ok, out) = lockstep(
        &write(
            "code.json",
            text.replace("\"ERR_APPLY\"", "\"ERR_OFFLINE\""),
        ),
        &oracle,
    );
    assert!(!ok, "{out}");
    assert!(out.contains("the recipe expects ERR_OFFLINE"), "{out}");
    // A refusal without its code cannot be told to the reference.
    let (ok, out) = lockstep(
        &write(
            "nocode.json",
            text.replace(", \"codes\": {\"nobody\": \"ERR_APPLY\"}", ""),
        ),
        &oracle,
    );
    assert!(!ok, "{out}");
    assert!(out.contains("name its refusal code"), "{out}");
    // A node cannot be stopped in a chain the producer made.
    let stop = text.replace(
        "\"do\": \"wait\", \"blocks\": 5",
        "\"do\": \"stop\", \"node\": 1",
    );
    let (ok, out) = lockstep(&write("stop.json", stop), &oracle);
    assert!(!ok, "{out}");
    assert!(out.contains("which a lockstep chain cannot"), "{out}");
    // A revert that returns to the last block of a round is the producer's refusal.
    let round_end = std::fs::read_to_string(fixtures().join("reorg-reverts-votes.json"))
        .expect("fixture")
        .replace(
            "\"do\": \"fork\", \"blocks\": 6",
            "\"do\": \"revert\", \"blocks\": 3",
        );
    let (ok, out) = lockstep(&write("round-end.json", round_end), &oracle);
    assert!(!ok, "{out}");
    assert!(out.contains("last block of a round"), "{out}");
}

#[test]
fn a_previously_answering_node_must_answer_the_single_tip_check() {
    let compiled = compiled(
        json!([{
            "at": {"height": 2}, "check": {"invariant": ["singleTip"], "withinBlocks": 2}
        }]),
        &[],
    );
    let mut fake = Fake::new(2);
    fake.unavailable.insert(2, (2, 20));
    let (report, _) = run_on(&mut fake, &compiled);
    assert!(outcome(&report, "singletip").is_failure(), "{report:#?}");
    assert_eq!(fake.height.get(), 4);
}

#[test]
fn every_trigger_waits_for_the_settling_floor() {
    for at in [
        json!({"height": 1}),
        json!({"afterBlocks": 0}),
        json!({"slot": 0}),
        json!({"slotsAfter": 0}),
        json!({"round": 1, "position": "start"}),
    ] {
        let mut compiled = compiled(json!([{ "at": at }]), &[]);
        compiled.scenario.settle_blocks = 3;
        let mut fake = Fake::new(1);
        for height in 1..=4 {
            fake.page(
                1,
                &format!("/api/blocks/{height}"),
                json!({"data": {"timestamp": {"epoch": 800}}}),
            );
        }
        let (_, record) = run_on(&mut fake, &compiled);
        assert!(record.steps[0].tip.0 >= 4, "{at}: {record:#?}");
    }
}

#[test]
fn lockstep_refuses_slot_triggers_before_running() {
    for at in [json!({"slot": 2}), json!({"slotsAfter": 2})] {
        let compiled = compiled(json!([{ "at": at }]), &[]);
        let mut fake = Fake::new(1);
        fake.backend_name = "lockstep";
        let error = run(&compiled, &mut fake, Level::L2)
            .expect_err("no slot clock")
            .to_string();
        assert!(
            error.contains("lockstep") && error.contains("slot"),
            "{error}"
        );
        assert_eq!(fake.height.get(), 1);
    }
}

#[test]
fn peer_to_peer_submission_is_refused_during_compilation() {
    let compiled = compiled(
        json!([{
            "at": {"height": 2}, "do": "submit", "node": 1, "via": "p2p"
        }]),
        &[],
    );
    let chain = Chain::new(&compiled.scenario).unwrap();
    let error = specs(&compiled.scenario, &chain)
        .expect_err("not built")
        .to_string();
    assert!(
        error.contains("peer-to-peer submit is not built"),
        "{error}"
    );
}

#[test]
fn equal_empty_pools_do_not_hold_the_previous_batch() {
    let compiled = compiled(
        json!([
            {"at": {"height": 2}, "do": "submit", "node": 1},
            {"at": {"afterBlocks": 0}, "check": {"api": {"projection": "pool"}}}
        ]),
        &[("batch", 0)],
    );
    let mut fake = Fake::new(2);
    fake.page_everywhere(
        "/api/transactions/unconfirmed?page=1&limit=100",
        one_page(json!([])),
    );
    let (report, _) = run_on(&mut fake, &compiled);
    assert!(outcome(&report, "api pool").is_failure(), "{report:#?}");
}

#[test]
fn a_missing_tip_is_retried_until_the_node_recovers() {
    let compiled = compiled(
        json!([{
            "at": {"height": 2}, "check": {"invariant": ["singleTip"], "withinBlocks": 2}
        }]),
        &[],
    );
    let mut fake = Fake::new(2);
    fake.unavailable.insert(2, (2, 3));
    let (report, _) = run_on(&mut fake, &compiled);
    assert_eq!(*outcome(&report, "singletip"), Outcome::Pass);
    assert_eq!(fake.height.get(), 4);
    assert!(
        report.results[0]
            .label
            .contains("compared [node 1, node 2]")
    );
}

#[test]
fn a_stopped_relay_is_exempt_until_its_relay_starts_again() {
    for role in ["node", "relay", "forger"] {
        for resume in [None, Some("start"), Some("restart")] {
            let mut steps =
                vec![json!({"at": {"height": 1}, "do": "stop", "node": 2, "role": role})];
            if let Some(action) = resume {
                steps.push(json!({"at": {"height": 2}, "do": action, "node": 2, "role": role}));
            }
            steps.push(json!({"at": {"height": 2}, "check": {"invariant": ["singleTip"]}}));
            let compiled = compiled(json!(steps), &[]);
            let mut fake = Fake::new(2);
            fake.unavailable.insert(2, (2, 20));
            let (report, _) = run_on(&mut fake, &compiled);
            let exempt = role != "forger" && resume.is_none();
            assert_eq!(
                outcome(&report, "singletip").is_failure(),
                !exempt,
                "{role} {resume:?}: {report:#?}"
            );
            if exempt {
                assert!(
                    report
                        .results
                        .last()
                        .unwrap()
                        .label
                        .contains("node 2 (deliberately stopped)")
                );
            }
        }
    }
}

#[test]
fn a_never_answering_node_is_labelled_exempt_unless_started() {
    for start in [false, true] {
        let mut steps = Vec::new();
        if start {
            steps.push(json!({"at": {"height": 1}, "do": "start", "node": 2}));
        }
        steps.push(json!({"at": {"height": 2}, "check": {"invariant": ["singleTip"]}}));
        let compiled = compiled(json!(steps), &[]);
        let mut fake = Fake::new(2);
        fake.unavailable.insert(2, (1, 20));
        let (report, _) = run_on(&mut fake, &compiled);
        assert_eq!(outcome(&report, "singletip").is_failure(), start);
        if !start {
            assert!(
                report.results[0]
                    .label
                    .contains("node 2 (never answered; held-back list unavailable)")
            );
        }
    }
}

#[test]
fn a_batch_in_every_pool_passes_on_the_submit_or_a_later_step() {
    for same_step in [false, true] {
        let mut steps = vec![json!({"at": {"height": 2}, "do": "submit", "node": 1})];
        if !same_step {
            steps.push(json!({"at": {"afterBlocks": 0}}));
        }
        steps.last_mut().unwrap()["check"] = json!({"api": {"projection": "pool"}});
        let compiled = compiled(json!(steps), &[("batch", 0)]);
        let mut fake = Fake::new(2);
        fake.page_everywhere(
            "/api/transactions/unconfirmed?page=1&limit=100",
            one_page(json!([{"id": "id-batch"}])),
        );
        let (report, _) = run_on(&mut fake, &compiled);
        assert_eq!(*outcome(&report, "api pool"), Outcome::Pass, "{report:#?}");
        fake.page(
            2,
            "/api/transactions/unconfirmed?page=1&limit=100",
            one_page(json!([])),
        );
        let (report, _) = run_on(&mut fake, &compiled);
        assert!(outcome(&report, "api pool").is_failure());
    }
}

#[test]
fn a_pool_check_can_name_an_earlier_transaction_batch() {
    let compiled = compiled(
        json!([
            {"at": {"height": 2}, "do": "submit", "node": 1},
            {"at": {"height": 2}, "do": "submit", "node": 1},
            {"at": {"afterBlocks": 0}, "check": {"api": {"projection": "pool(earlier)"}}}
        ]),
        &[("earlier", 0), ("later", 1)],
    );
    let mut fake = Fake::new(2);
    fake.page_everywhere(
        "/api/transactions/unconfirmed?page=1&limit=100",
        one_page(json!([{"id": "id-earlier"}])),
    );
    let (report, _) = run_on(&mut fake, &compiled);
    assert_eq!(*outcome(&report, "api pool"), Outcome::Pass);
}

#[test]
fn settling_counts_blocks_from_the_runs_starting_tip() {
    let mut compiled = compiled(json!([{ "at": {"height": 2} }]), &[]);
    compiled.scenario.settle_blocks = 3;
    let mut fake = Fake::new(1);
    fake.height.set(10);
    let (_, record) = run_on(&mut fake, &compiled);
    assert_eq!(record.steps[0].tip.0, 13);
}

#[test]
fn a_slot_trigger_requires_a_timestamp_from_the_backend() {
    let compiled = compiled(json!([{ "at": {"slot": 2} }]), &[]);
    let (report, record) = run_on(&mut Fake::new(1), &compiled);
    assert!(
        matches!(report.results[0].outcome, Outcome::HarnessError(ref message)
        if message.contains("without a block timestamp"))
    );
    assert!(record.steps.is_empty());
}

#[test]
fn committed_scenarios_keep_their_trigger_plans() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("scenarios");
    let mut dirs = vec![root];
    let mut count = 0;
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                dirs.push(path);
            } else if path.extension().is_some_and(|ext| ext == "json") {
                let scenario = scenario::load(&path).unwrap();
                assert_eq!(scenario.settle_blocks, 0);
                assert!(
                    scenario
                        .steps
                        .iter()
                        .all(|step| step.at.slot.is_none() && step.at.slots_after.is_none())
                );
                let chain = Chain::new(&scenario).unwrap();
                let heights = fire_heights(&scenario, chain.round_length).unwrap();
                let compiled = Compiled {
                    scenario,
                    transactions: Vec::new(),
                };
                let mut fake = Fake::new(7);
                fake.limit = 5 * chain.round_length;
                // Isolate trigger timing from the actions and checks of each scenario.
                let mut compiled = compiled;
                for step in &mut compiled.scenario.steps {
                    step.action = None;
                    step.check = None;
                }
                let (report, record) = run_on(&mut fake, &compiled);
                assert_eq!(report.failed(), 0, "{report:#?}");
                assert_eq!(
                    record
                        .steps
                        .iter()
                        .map(|step| step.tip.0)
                        .collect::<Vec<_>>(),
                    heights
                );
                count += 1;
            }
        }
    }
    assert!(count > 0);
}
