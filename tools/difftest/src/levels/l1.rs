//! L1: random and mutated cases, one oracle call each, compared with the same subjects as L0.
//!
//! Each property draws its inputs from [`crate::strategies`], sends each case to
//! `oracle-server.js`, runs the case's subject, and compares the two results; a failing case is
//! shrunk to a minimal one, which the run writes to its regression directory.
//!
//! One difference is documented and applied here: when the reference refuses a transaction whose
//! sender has no public key, it has already bound the key (it binds it before its checks), and
//! Heartwood leaves the state unchanged. For those cases the sender's key is left out of the
//! oracle's wallet records before they are compared (the refused-unbound-sender predicate). A
//! handler case whose revert does not give back its fixture, on either side, fails as a harness
//! error even when both sides agree.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;

use heartwood_crypto::crypto::hash::sha256;
use heartwood_crypto::identities::Address;
use proptest::strategy::{BoxedStrategy, Strategy, Union};
use proptest::test_runner::{Config, RngAlgorithm, TestCaseError, TestError, TestRng, TestRunner};
use serde_json::{Value, json};

use crate::errors::{Error, Result};
use crate::fixture::Fixture;
use crate::oracle::OracleServer;
use crate::report::{CaseResult, Outcome, Report};
use crate::strategies::transactions::{TxContext, fee_cases, handler_cases};
use crate::strategies::{
    api_js, api_page, api_query, api_state, exchange, fixtures, forger_decide, forging, kernel,
    monitor, quorum, served, state, verifier,
};
use crate::subject::{Case, Registry};
use crate::vectors::Expected;

/// A property of L1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Property {
    /// The dynamic fee floor (`fee.minimum`).
    FeeFloor,
    /// Rounds (`round.calculate`, `round.isNew`).
    Rounds,
    /// Forging slots and chaining (`forging.info`, `block.isChained`).
    Slots,
    /// The forging shuffle (`state.shuffle`).
    Shuffle,
    /// The delegate ranking (`state.rank`).
    Ranking,
    /// A voter's shares (`wallet.voteAmounts`).
    VoteAmounts,
    /// One transaction applied to a fixture and reverted (`state.applyTransaction`).
    Handlers,
    /// The kernel's text formats (`kernel.stringify`, `kernel.dotenv`, `kernel.ip`).
    KernelText,
    /// The state digest of a fixture (`state.digest`).
    Digest,
    /// A pool session over a fixture (`pool.run`).
    Pool,
    /// The reference's message formats (`text.*`).
    Text,
    /// The crypto files, loaded with the strict schema (`config.load`).
    Config,
    /// The block processor (`chain.process`).
    Processor,
    /// The peer verifier (`verifier.checkState`).
    Verifier,
    /// The client's requests and the peers' replies (`p2p.validateRequest`, `wire.checkReply`).
    Exchange,
    /// The network monitor's decisions (`monitor.forkDecision`, `monitor.networkHeight`).
    Monitor,
    /// Forging from a pool session (`forger.collateAndForge`).
    Forging,
    /// The network state a relay gives its forger (`quorum.analyze`).
    Quorum,
    /// The forger's decisions in a slot (`forger.decide`).
    Decisions,
    /// The public routes' answers (`p2p.controller`).
    Served,
    /// The API's listings over the state and their pages (`api.delegatesPage`, `api.walletsPage`,
    /// `api.searchPage`).
    Listings,
    /// The API's wallet search (`api.walletsLike`).
    WalletSearch,
    /// The API's query grammar (`api.parseQuery`).
    QueryGrammar,
}

impl Property {
    /// Every property.
    pub const ALL: [Property; 23] = [
        Property::FeeFloor,
        Property::Rounds,
        Property::Slots,
        Property::Shuffle,
        Property::Ranking,
        Property::VoteAmounts,
        Property::Handlers,
        Property::KernelText,
        Property::Digest,
        Property::Pool,
        Property::Text,
        Property::Config,
        Property::Processor,
        Property::Verifier,
        Property::Exchange,
        Property::Monitor,
        Property::Forging,
        Property::Quorum,
        Property::Decisions,
        Property::Served,
        Property::Listings,
        Property::WalletSearch,
        Property::QueryGrammar,
    ];

    /// The property's name on the command line.
    pub const fn name(self) -> &'static str {
        match self {
            Property::FeeFloor => "fee-floor",
            Property::Rounds => "rounds",
            Property::Slots => "slots",
            Property::Shuffle => "shuffle",
            Property::Ranking => "ranking",
            Property::VoteAmounts => "vote-amounts",
            Property::Handlers => "handlers",
            Property::KernelText => "kernel-text",
            Property::Digest => "digest",
            Property::Pool => "pool",
            Property::Text => "text",
            Property::Config => "config",
            Property::Processor => "processor",
            Property::Verifier => "verifier",
            Property::Exchange => "exchange",
            Property::Monitor => "monitor",
            Property::Forging => "forging",
            Property::Quorum => "quorum",
            Property::Decisions => "decisions",
            Property::Served => "served",
            Property::Listings => "listings",
            Property::WalletSearch => "wallet-search",
            Property::QueryGrammar => "query-grammar",
        }
    }

    /// The property's short name (P1 to P23), which the command line accepts too.
    pub const fn number(self) -> &'static str {
        match self {
            Property::FeeFloor => "P1",
            Property::Rounds => "P2",
            Property::Slots => "P3",
            Property::Shuffle => "P4",
            Property::Ranking => "P5",
            Property::VoteAmounts => "P6",
            Property::Handlers => "P7",
            Property::KernelText => "P8",
            Property::Digest => "P9",
            Property::Pool => "P10",
            Property::Text => "P11",
            Property::Config => "P12",
            Property::Processor => "P13",
            Property::Verifier => "P14",
            Property::Exchange => "P15",
            Property::Monitor => "P16",
            Property::Forging => "P17",
            Property::Quorum => "P18",
            Property::Decisions => "P19",
            Property::Served => "P20",
            Property::Listings => "P21",
            Property::WalletSearch => "P22",
            Property::QueryGrammar => "P23",
        }
    }
}

impl fmt::Display for Property {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl FromStr for Property {
    type Err = Error;

    fn from_str(s: &str) -> Result<Property> {
        Property::ALL
            .into_iter()
            .find(|property| property.name() == s || property.number().eq_ignore_ascii_case(s))
            .ok_or_else(|| Error::Usage(format!("unknown L1 property {s}")))
    }
}

/// What an L1 run covers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct L1Options {
    /// The properties to run.
    pub properties: Vec<Property>,
    /// Cases per property.
    pub cases: u32,
    /// The seed of the case generator.
    pub seed: u64,
    /// Where the minimal failing case of each failed property is written, one JSON line per
    /// property file.
    pub regressions: Option<PathBuf>,
    /// How often P13 must reach each reachable result of the processor and each unchained status
    /// in the reference's answers; 0 asks nothing (the counts are reported all the same).
    pub min_reach: u32,
    /// How often P19 must reach each action and refusal of the reference's forging attempt; 0
    /// asks nothing (the counts are reported all the same).
    pub decision_reach: u32,
    /// How often P20 must reach each public route, and a tenth as often the getBlocks cut; 0 asks
    /// nothing.
    pub served_reach: u32,
}

/// One case of a property, as the oracle takes it.
#[derive(Debug, Clone)]
pub struct L1Case {
    /// The operation.
    pub op: &'static str,
    /// The rule height of the request.
    pub height: u64,
    /// The operation's input.
    pub input: Value,
}

impl L1Case {
    fn plain((op, height, input): kernel::OpCase) -> L1Case {
        L1Case { op, height, input }
    }
}

/// The cases of `property`.
pub fn strategy(property: Property, context: &Arc<TxContext>) -> BoxedStrategy<L1Case> {
    match property {
        Property::FeeFloor => fee_cases(context.clone())
            .prop_map(|(height, input)| L1Case {
                op: "fee.minimum",
                height,
                input,
            })
            .boxed(),
        Property::Rounds => kernel::round_cases(context.clone())
            .prop_map(L1Case::plain)
            .boxed(),
        Property::Slots => kernel::slot_cases().prop_map(L1Case::plain).boxed(),
        Property::Shuffle => state::shuffle_cases().prop_map(L1Case::plain).boxed(),
        Property::Ranking => state::ranking_cases().prop_map(L1Case::plain).boxed(),
        Property::VoteAmounts => state::vote_amount_cases().prop_map(L1Case::plain).boxed(),
        Property::Handlers => handler_cases(context.clone())
            .prop_map(|c| L1Case {
                op: "state.applyTransaction",
                height: c.height,
                input: c.input,
            })
            .boxed(),
        Property::KernelText => kernel::kernel_text_cases().prop_map(L1Case::plain).boxed(),
        Property::Digest => fixtures()
            .prop_map(|f: Fixture| L1Case {
                op: "state.digest",
                height: 2,
                input: json!({"fixture": f.to_json()}),
            })
            .boxed(),
        Property::Pool => crate::strategies::pool::pool_cases(context.clone())
            .prop_map(L1Case::plain)
            .boxed(),
        Property::Text => crate::strategies::text::text_cases()
            .prop_map(L1Case::plain)
            .boxed(),
        Property::Config => crate::strategies::config::config_cases(context.clone())
            .prop_map(L1Case::plain)
            .boxed(),
        Property::Processor => crate::strategies::processor::processor_cases(context)
            .prop_map(L1Case::plain)
            .boxed(),
        Property::Verifier => verifier::verifier_cases(context)
            .prop_map(L1Case::plain)
            .boxed(),
        Property::Exchange => exchange::exchange_cases(context)
            .prop_map(L1Case::plain)
            .boxed(),
        Property::Monitor => monitor::monitor_cases(context)
            .prop_map(L1Case::plain)
            .boxed(),
        Property::Forging => forging::forging_cases(context)
            .prop_map(L1Case::plain)
            .boxed(),
        Property::Quorum => quorum::quorum_cases(context)
            .prop_map(L1Case::plain)
            .boxed(),
        Property::Decisions => forger_decide::decision_cases(context)
            .prop_map(L1Case::plain)
            .boxed(),
        Property::Served => served::served_cases(context)
            .prop_map(L1Case::plain)
            .boxed(),
        Property::Listings => Union::new([
            api_state::listing_cases(context),
            api_page::page_cases(context),
        ])
        .prop_map(L1Case::plain)
        .boxed(),
        Property::WalletSearch => api_js::wallet_search_cases(context)
            .prop_map(L1Case::plain)
            .boxed(),
        Property::QueryGrammar => api_query::query_cases(context)
            .prop_map(L1Case::plain)
            .boxed(),
    }
}

/// The reference's action in a decision case (`decide forged`, `decide stop`, ...), a refusal
/// with its reason (`decide refused: quorum`), and an error answer by its class.
fn decision_outcomes(expected: &Expected, seen: &mut BTreeMap<String, usize>) {
    let key = match expected {
        Expected::Output(output) => {
            let action = output["action"].as_str().unwrap_or("?");
            match output["reason"].as_str() {
                Some(reason) => format!("decide {action}: {reason}"),
                None => format!("decide {action}"),
            }
        }
        Expected::Error(error) => format!("decide error: {}", error.class),
    };
    *seen.entry(key).or_insert(0) += 1;
}

/// The outcomes of the reference in a pool session's answer, counted: each error code, an
/// `ERR_APPLY` of the cold-wallet refusal apart (listed even when it never occurs, since whether
/// the pool can reach it is a question the report answers), and every accepted entry.
fn pool_outcomes(expected: &Expected, seen: &mut BTreeMap<String, usize>) {
    seen.entry(COLD_WALLET_OUTCOME.to_owned()).or_insert(0);
    let Expected::Output(output) = expected else {
        return;
    };
    for step in output["results"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
    {
        let result = &step["result"];
        let accepted = result["accept"].as_array().map_or(0, Vec::len);
        if accepted > 0 {
            *seen.entry("pool accepted".into()).or_insert(0) += accepted;
        }
        for error in result["errors"]
            .as_object()
            .into_iter()
            .flat_map(|e| e.values())
        {
            let code = error["type"].as_str().unwrap_or("?");
            let message = error["message"].as_str().unwrap_or("");
            let key = if code == "ERR_APPLY" && message.ends_with(COLD_WALLET) {
                COLD_WALLET_OUTCOME.to_owned()
            } else {
                format!("pool {code}")
            };
            *seen.entry(key).or_insert(0) += 1;
        }
        if result["notes"]["fullPoolEviction"] == Value::Bool(true) {
            *seen.entry("pool full-pool eviction".into()).or_insert(0) += 1;
        }
    }
}

/// The reference's `ColdWalletError` text, which ends a pool refusal that wraps it.
const COLD_WALLET: &str = "Wallet is not allowed to spend before funding is confirmed";
/// The outcome under which a pool run counts the cold-wallet refusals.
const COLD_WALLET_OUTCOME: &str = "pool ERR_APPLY (cold wallet)";

/// `section` (a `#W` section) without the public key of `address`, and without its record when
/// the key was all that listed it: what the reference's wallets would be had it not bound the
/// key of a sender whose transaction it refused.
pub fn without_bound_key(section: &str, address: &Address) -> String {
    let wanted = address.to_base58();
    let mut out = String::new();
    for (i, line) in section.lines().enumerate() {
        if i == 0 {
            out.push_str(line);
            out.push('\n');
            continue;
        }
        let Ok(Value::Object(mut record)) = serde_json::from_str::<Value>(line) else {
            out.push_str(line);
            out.push('\n');
            continue;
        };
        if record.get("a").and_then(Value::as_str) == Some(wanted.as_str()) {
            record.shift_remove("pk");
            let listed = record.get("bal").and_then(Value::as_str) != Some("0")
                || record.get("n").and_then(Value::as_str) != Some("0")
                || record.len() > 3;
            if !listed {
                continue;
            }
        }
        out.push_str(&Value::Object(record).to_string());
        out.push('\n');
    }
    out
}

/// The results of the reference's processor that `process` can reach; `Reverted` and `Corrupted`
/// are not reachable from it.
pub const PROCESSOR_RESULTS: [&str; 4] = [
    "Accepted",
    "DiscardedButCanBeBroadcasted",
    "Rejected",
    "Rollback",
];

/// The unchained statuses of the reference's unchained handler.
pub const UNCHAINED_STATUSES: [&str; 6] = [
    "NotReadyToAcceptNewHeight",
    "AlreadyInBlockchain",
    "EqualToLastBlock",
    "GeneratorMismatch",
    "DoubleForging",
    "InvalidTimestamp",
];

/// The outcome classes of a processor answer (P13): its result, its unchained status and its
/// handler, or the error class of an answer that failed.
fn processor_outcomes(expected: &Expected, seen: &mut BTreeMap<String, usize>) {
    let mut count = |class: String| *seen.entry(class).or_insert(0) += 1;
    match expected {
        Expected::Output(o) => {
            for (key, name) in [
                ("result", "result"),
                ("unchained", "unchained"),
                ("handler", "handler"),
            ] {
                if let Some(value) = o.get(key).and_then(Value::as_str) {
                    count(format!("P13 {name} {value}"));
                }
            }
        }
        Expected::Error(e) => count(format!("P13 error: {}", e.class)),
    }
}

/// The classes of `wanted` that `seen` counts fewer than `minimum` times, each with its count.
fn reached_fewer(
    seen: &BTreeMap<String, usize>,
    minimum: u32,
    wanted: impl Iterator<Item = String>,
) -> Vec<(String, usize)> {
    let minimum = usize::try_from(minimum).unwrap_or(usize::MAX);
    wanted
        .map(|class| {
            let n = seen.get(&class).copied().unwrap_or(0);
            (class, n)
        })
        .filter(|(_, n)| *n < minimum)
        .collect()
}

/// The reachable processor results and unchained statuses that `seen` counts fewer than
/// `minimum` times, each with its count.
pub fn under_reached(seen: &BTreeMap<String, usize>, minimum: u32) -> Vec<(String, usize)> {
    let wanted = (PROCESSOR_RESULTS.iter().map(|r| format!("P13 result {r}"))).chain(
        UNCHAINED_STATUSES
            .iter()
            .map(|u| format!("P13 unchained {u}")),
    );
    reached_fewer(seen, minimum, wanted)
}

/// The outcomes of a forging decision that P19 must reach: each action of the reference's attempt
/// (`forger-decide.js`), and each reason it refuses for. An answer that is an error is no
/// action and is not listed. P19 must also reach the forger's paths of [`forger_decide::PATHS`].
pub const DECISION_CLASSES: [&str; 10] = [
    "decide stop",
    "decide error",
    "decide forged",
    "decide late",
    "decide sameSlot",
    "decide refused: unknown",
    "decide refused: coldStart",
    "decide refused: belowMinimum",
    "decide refused: alreadyForged",
    "decide refused: quorum",
];

/// The decision actions, refusals and paths that `seen` counts fewer than `minimum` times, each
/// with its count.
pub fn decisions_under_reached(
    seen: &BTreeMap<String, usize>,
    minimum: u32,
) -> Vec<(String, usize)> {
    let wanted = DECISION_CLASSES.iter().chain(&forger_decide::PATHS);
    reached_fewer(seen, minimum, wanted.map(|c| (*c).to_owned()))
}

/// The error class of a refused transaction in an oracle answer (P7).
fn refusal_class(expected: &Expected) -> Option<String> {
    match expected {
        Expected::Output(o) => o
            .pointer("/error/class")
            .and_then(Value::as_str)
            .map(str::to_owned),
        Expected::Error(e) => Some(format!("error: {}", e.class)),
    }
}

/// An L1 run: the report, and the error classes the handler cases reached.
#[derive(Debug, Clone)]
pub struct L1Run {
    /// One result per case (the passes, skips and documented differences) and the minimal failing
    /// case of each failed property.
    pub report: Report,
    /// For the handler property: how often each outcome occurred (`applied` or an error class);
    /// for the pool property, each error code of the reference's answers (`pool ERR_...`), the
    /// accepted entries and the full-pool evictions; for the processor property, each result,
    /// unchained status and handler of the reference's answers (`P13 result ...`); for the
    /// decisions property, each action of the reference's attempt (`decide forged`, ...), a
    /// refusal with its reason.
    pub classes: BTreeMap<String, usize>,
}

fn seed_for(seed: u64, property: Property) -> [u8; 32] {
    sha256(format!("heartwood/l1/{}/{seed}", property.name()).as_bytes())
}

/// Run the properties of `options` against the oracle `server` and the subjects of `registry`,
/// with the devnet `context` the transaction strategies build in.
pub fn run(
    server: &mut OracleServer,
    registry: &Registry,
    context: &Arc<TxContext>,
    options: &L1Options,
) -> Result<L1Run> {
    let server = RefCell::new(server);
    let mut results = Vec::new();
    let mut classes = BTreeMap::new();
    for property in &options.properties {
        let recorded: RefCell<Vec<(L1Case, Outcome)>> = RefCell::new(Vec::new());
        let harness: RefCell<Option<Error>> = RefCell::new(None);
        let seen: RefCell<BTreeMap<String, usize>> = RefCell::new(BTreeMap::new());
        let config = Config {
            cases: options.cases,
            failure_persistence: None,
            max_shrink_iters: 512,
            max_global_rejects: options.cases.saturating_mul(8).max(1_024),
            ..Config::default()
        };
        let rng = TestRng::from_seed(RngAlgorithm::ChaCha, &seed_for(options.seed, *property));
        let mut runner = TestRunner::new_with_rng(config, rng);
        // A property whose strategy is still a stub is skipped, as its subject's cases would be.
        let cases = match crate::store::guarded(|| strategy(*property, context)) {
            Ok(cases) => cases,
            Err(crate::store::Unbuilt(message)) => {
                results.push(CaseResult {
                    file: format!("l1/{}", property.name()),
                    line: 0,
                    op: property.number().to_owned(),
                    label: property.number().into(),
                    height: 0,
                    outcome: Outcome::Skipped(format!("not built yet: {message}")),
                });
                continue;
            }
        };
        let outcome = runner.run(&cases, |case| {
            if harness.borrow().is_some() {
                return Err(TestCaseError::fail("the oracle failed"));
            }
            let answer = match server
                .borrow_mut()
                .call(case.op, "devnet", case.height, &case.input)
            {
                Ok(answer) => answer,
                Err(e) => {
                    *harness.borrow_mut() = Some(e);
                    return Err(TestCaseError::fail("the oracle failed"));
                }
            };
            let answer = match answer {
                Ok(output) => Expected::Output(output),
                Err(error) => Expected::Error(error),
            };
            let request = Case {
                op: case.op,
                network: "devnet",
                height: case.height,
                input: &case.input,
            };
            if *property == Property::Handlers {
                let class = refusal_class(&answer).unwrap_or_else(|| "applied".into());
                *seen.borrow_mut().entry(class).or_insert(0) += 1;
            }
            if *property == Property::Pool {
                pool_outcomes(&answer, &mut seen.borrow_mut());
            }
            if *property == Property::Processor {
                processor_outcomes(&answer, &mut seen.borrow_mut());
            }
            if *property == Property::Decisions {
                decision_outcomes(&answer, &mut seen.borrow_mut());
                forger_decide::paths(&case.input, &answer, &mut seen.borrow_mut());
            }
            if *property == Property::Served {
                served::outcomes(&case.input, &answer, &mut seen.borrow_mut());
            }
            let judged = super::judge_documented(&request, &answer, registry);
            let failed = judged.is_failure();
            let message = format!("{judged:?}");
            recorded.borrow_mut().push((case, judged));
            if failed {
                Err(TestCaseError::fail(message))
            } else {
                Ok(())
            }
        });
        if let Some(e) = harness.into_inner() {
            return Err(e);
        }
        let seen = seen.into_inner();
        let file = format!("l1/{}", property.name());
        if *property == Property::Processor {
            results.extend(processor_checks(&file, &seen, options.min_reach));
        }
        if *property == Property::Decisions {
            results.extend(decision_checks(&file, &seen, options.decision_reach));
        }
        if *property == Property::Served {
            results.extend(served::reach_checks(&file, &seen, options.served_reach));
        }
        if matches!(
            *property,
            Property::Handlers | Property::Pool | Property::Processor | Property::Decisions
        ) {
            for (class, n) in seen {
                *classes.entry(class).or_insert(0) += n;
            }
        }
        let recorded = recorded.into_inner();
        for (i, (case, judged)) in recorded.iter().enumerate() {
            if judged.is_failure() {
                continue;
            }
            results.push(CaseResult {
                file: file.clone(),
                line: i + 1,
                op: case.op.to_owned(),
                label: format!("{} {}", property.number(), case.op),
                height: case.height,
                outcome: judged.clone(),
            });
        }
        match outcome {
            Ok(()) => {}
            Err(TestError::Fail(reason, minimal)) => {
                let judged = recorded
                    .iter()
                    .rev()
                    .find(|(c, _)| c.input == minimal.input && c.op == minimal.op)
                    .map(|(_, j)| j.clone())
                    .unwrap_or_else(|| Outcome::HarnessError(reason.to_string()));
                if let Some(dir) = &options.regressions {
                    write_regression(dir, *property, &minimal)?;
                }
                let input = minimal.input.to_string();
                let shown: String = input.chars().take(600).collect();
                let records = explain(&mut server.borrow_mut(), registry, &minimal);
                results.push(CaseResult {
                    file: file.clone(),
                    line: 0,
                    op: minimal.op.to_owned(),
                    label: format!(
                        "{} minimal case {} {shown}{records}",
                        property.number(),
                        minimal.op
                    ),
                    height: minimal.height,
                    outcome: judged,
                });
            }
            Err(TestError::Abort(reason)) => results.push(CaseResult {
                file: file.clone(),
                line: 0,
                op: "abort".into(),
                label: property.number().into(),
                height: 0,
                outcome: Outcome::HarnessError(format!("the property stopped: {reason}")),
            }),
        }
    }
    let mut source = format!(
        "{} cases per property, seed {}, properties {}",
        options.cases,
        options.seed,
        options
            .properties
            .iter()
            .map(|p| p.number())
            .collect::<Vec<_>>()
            .join(" ")
    );
    if options.properties.contains(&Property::Processor) {
        let corpora = crate::strategies::processor::corpora();
        source.push_str(&format!("; P13 on {}", corpora.used.join(", ")));
        if options.min_reach > 0 {
            source.push_str(&format!(
                ", each reachable result and unchained status at least {} times",
                options.min_reach
            ));
        }
    }
    if options.properties.contains(&Property::Decisions) && options.decision_reach > 0 {
        source.push_str(&format!(
            "; P19 each action, refusal and path at least {} times",
            options.decision_reach
        ));
    }
    let report = Report {
        title: super::Level::L1.title().to_string(),
        source,
        files: options.properties.len(),
        results,
    };
    Ok(L1Run { report, classes })
}

/// P13's own checks: a corpus of its generator that is missing is a skipped part (a strict run
/// fails on it), and each reachable result and unchained status must be reached `min_reach` times.
fn processor_checks(file: &str, seen: &BTreeMap<String, usize>, min_reach: u32) -> Vec<CaseResult> {
    let check = |label: String, outcome: Outcome| CaseResult {
        file: file.to_owned(),
        line: 0,
        op: Property::Processor.number().to_owned(),
        label,
        height: 0,
        outcome,
    };
    let mut out: Vec<CaseResult> = (crate::strategies::processor::corpora().missing.into_iter())
        .map(|why| {
            check(
                "P13 corpus".into(),
                Outcome::Skipped(format!("the cases drew on the other corpora: {why}")),
            )
        })
        .collect();
    if min_reach > 0 {
        let under = under_reached(seen, min_reach);
        let label =
            format!("P13 reached each reachable result and unchained status {min_reach} times");
        out.push(check(
            label,
            if under.is_empty() {
                Outcome::Pass
            } else {
                let list: Vec<String> = under.iter().map(|(c, n)| format!("{c} {n}")).collect();
                Outcome::HarnessError(format!("fewer: {}", list.join(", ")))
            },
        ));
    }
    out
}

/// P19's own check: each of the reference's actions and refusals, and each path of
/// [`forger_decide::PATHS`], must be reached `min_reach` times; nothing is checked without a
/// minimum.
fn decision_checks(file: &str, seen: &BTreeMap<String, usize>, min_reach: u32) -> Vec<CaseResult> {
    if min_reach == 0 {
        return Vec::new();
    }
    let under = decisions_under_reached(seen, min_reach);
    let outcome = if under.is_empty() {
        Outcome::Pass
    } else {
        let list: Vec<String> = under.iter().map(|(c, n)| format!("{c} {n}")).collect();
        Outcome::HarnessError(format!("fewer: {}", list.join(", ")))
    };
    vec![CaseResult {
        file: file.to_owned(),
        line: 0,
        op: Property::Decisions.number().to_owned(),
        label: format!(
            "P19 reached each action, refusal and path of the reference {min_reach} times"
        ),
        height: 0,
        outcome,
    }]
}

/// The wallet records where the two sides differ on `case`, by wallet and field, as extra
/// lines for the report; empty when the answers hold no wallet sections.
fn explain(server: &mut OracleServer, registry: &Registry, case: &L1Case) -> String {
    let Ok(Ok(theirs)) = server.call(case.op, "devnet", case.height, &case.input) else {
        return String::new();
    };
    let request = Case {
        op: case.op,
        network: "devnet",
        height: case.height,
        input: &case.input,
    };
    let theirs = match super::documented_expected(&request, &Expected::Output(theirs)) {
        Expected::Output(o) => o,
        Expected::Error(_) => return String::new(),
    };
    let Some(Ok(crate::subject::Verdict::Output(ours))) =
        registry.get(case.op).map(|s| s.run(&request))
    else {
        return String::new();
    };
    let mut out = String::new();
    for path in ["/applied/W", "/reverted/W", "/after/W", "/sections/W"] {
        let (Some(a), Some(b)) = (
            theirs.pointer(path).and_then(Value::as_str),
            ours.pointer(path).and_then(Value::as_str),
        ) else {
            continue;
        };
        let only_wallets = |w: &str| {
            crate::digests::sections_from_json(&json!({
                "B": "", "S": "", "R": "", "M": "", "W": w,
            }))
        };
        if let (Some(a), Some(b)) = (only_wallets(a), only_wallets(b)) {
            for line in crate::digests::section_differences(&a, &b) {
                out.push_str(&format!("\n      {path}: {line}"));
            }
        }
    }
    out
}

fn write_regression(dir: &std::path::Path, property: Property, case: &L1Case) -> Result<()> {
    std::fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
    let path = dir.join(format!("{}.jsonl", property.name()));
    let line =
        json!({"op": case.op, "network": "devnet", "height": case.height, "input": case.input});
    let mut text = std::fs::read_to_string(&path).unwrap_or_default();
    text.push_str(&format!("{line}\n"));
    std::fs::write(&path, text).map_err(|e| Error::io(&path, e))
}

/// The devnet's milestone file, JSON text: the chain definition in the vector set's genesis class
/// whose milestone file the meta records name.
pub fn devnet_milestones(set: &crate::vectors::VectorSet) -> Result<String> {
    let problem = |message: &str| Error::Vectors {
        path: set.dir.clone(),
        message: message.to_owned(),
    };
    let wanted = set
        .files
        .iter()
        .find_map(|f| f.meta.network["milestonesSha256"].as_str())
        .ok_or_else(|| problem("no meta record names the milestones"))?;
    set.files
        .iter()
        .filter(|f| f.meta.class == "V10-genesis")
        .flat_map(|f| &f.records)
        .filter_map(|r| match &r.expected {
            Expected::Output(o) => o["files"]["crypto/milestones.json"].as_str(),
            Expected::Error(_) => None,
        })
        .find(|text| heartwood_crypto::utils::hex::encode(&sha256(text.as_bytes())) == wanted)
        .map(str::to_owned)
        .ok_or_else(|| {
            problem("no chain definition of the genesis class has the devnet's milestone file")
        })
}

#[cfg(test)]
mod tests {
    use heartwood_crypto::identities::KeyPair;

    use super::*;

    #[test]
    fn properties_parse_by_name_and_number() {
        assert_eq!("P7".parse::<Property>().unwrap(), Property::Handlers);
        assert_eq!("p1".parse::<Property>().unwrap(), Property::FeeFloor);
        assert_eq!("digest".parse::<Property>().unwrap(), Property::Digest);
        assert_eq!("P10".parse::<Property>().unwrap(), Property::Pool);
        assert_eq!("p12".parse::<Property>().unwrap(), Property::Config);
        assert_eq!("P13".parse::<Property>().unwrap(), Property::Processor);
        assert_eq!(
            "processor".parse::<Property>().unwrap(),
            Property::Processor
        );
        assert_eq!("P16".parse::<Property>().unwrap(), Property::Monitor);
        assert_eq!("P17".parse::<Property>().unwrap(), Property::Forging);
        assert_eq!("P18".parse::<Property>().unwrap(), Property::Quorum);
        assert_eq!("P19".parse::<Property>().unwrap(), Property::Decisions);
        assert_eq!("P20".parse::<Property>().unwrap(), Property::Served);
        assert_eq!("P21".parse::<Property>().unwrap(), Property::Listings);
        assert_eq!("p22".parse::<Property>().unwrap(), Property::WalletSearch);
        assert_eq!(
            "query-grammar".parse::<Property>().unwrap(),
            Property::QueryGrammar
        );
        assert!("P24".parse::<Property>().is_err());
    }

    #[test]
    fn processor_answers_are_counted_and_their_reach_checked() {
        let mut seen = BTreeMap::new();
        let answer = |o: Value| Expected::Output(o);
        for status in UNCHAINED_STATUSES {
            let o =
                json!({"result": "Rejected", "handler": "UnchainedHandler", "unchained": status});
            processor_outcomes(&answer(o), &mut seen);
        }
        for result in PROCESSOR_RESULTS {
            processor_outcomes(
                &answer(json!({"result": result, "handler": "H"})),
                &mut seen,
            );
        }
        assert_eq!(seen["P13 result Rejected"], 7);
        assert_eq!(seen["P13 unchained DoubleForging"], 1);
        assert_eq!(seen["P13 handler UnchainedHandler"], 6);
        assert!(under_reached(&seen, 1).is_empty());
        let under = under_reached(&seen, 2);
        assert_eq!(under.len(), 9, "{under:?}");
        assert!(!under.iter().any(|(c, _)| c == "P13 result Rejected"));
        seen.remove("P13 unchained InvalidTimestamp");
        assert_eq!(
            under_reached(&seen, 1),
            [("P13 unchained InvalidTimestamp".to_owned(), 0)]
        );
        let checks = processor_checks("l1/processor", &seen, 1);
        let reach = checks.last().unwrap();
        assert!(reach.outcome.is_failure(), "{reach:?}");
        assert!(
            processor_checks("l1/processor", &seen, 0)
                .iter()
                .all(|c| !c.outcome.is_failure())
        );
    }

    #[test]
    fn every_decision_action_and_refusal_must_reach_the_minimum() {
        let mut seen = BTreeMap::new();
        let output = |action: &str, reason: Option<&str>| {
            Expected::Output(json!({"action": action, "reason": reason}))
        };
        for class in DECISION_CLASSES {
            let class = class.trim_start_matches("decide ");
            let (action, reason) = match class.split_once(": ") {
                Some((action, reason)) => (action, Some(reason)),
                None => (class, None),
            };
            decision_outcomes(&output(action, reason), &mut seen);
        }
        assert_eq!(DECISION_CLASSES.len(), 10);
        // Every action and refusal, but no path of the forger.
        let checks = decision_checks("l1/decisions", &seen, 1);
        let Outcome::HarnessError(message) = &checks[0].outcome else {
            panic!("{checks:?}");
        };
        for path in forger_decide::PATHS {
            assert!(message.contains(&format!("{path} 0")), "{message}");
        }
        for path in forger_decide::PATHS {
            seen.insert(path.to_owned(), 2);
        }
        let checks = decision_checks("l1/decisions", &seen, 1);
        assert!(checks.iter().all(|c| !c.outcome.is_failure()), "{checks:?}");
        assert_eq!(checks.len(), 1);
        // One action reached once, in a run that asks for two.
        let checks = decision_checks("l1/decisions", &seen, 2);
        let Outcome::HarnessError(message) = &checks[0].outcome else {
            panic!("{checks:?}");
        };
        assert!(message.contains("decide sameSlot 1"), "{message}");
        // One refusal never reached.
        seen.remove("decide refused: coldStart");
        let checks = decision_checks("l1/decisions", &seen, 1);
        let Outcome::HarnessError(message) = &checks[0].outcome else {
            panic!("{checks:?}");
        };
        assert!(message.contains("decide refused: coldStart 0"), "{message}");
        // A run without a minimum checks nothing.
        assert!(decision_checks("l1/decisions", &seen, 0).is_empty());
    }

    #[test]
    fn the_bound_key_of_a_refused_sender_is_left_out() {
        let key = KeyPair::from_passphrase("l1 test").unwrap();
        let address = Address::from_public_key(key.public_key(), 90);
        let a = address.to_base58();
        let pk = key.public_key().to_hex();
        let section = format!(
            "#W\n{{\"a\":\"{a}\",\"pk\":\"{pk}\",\"bal\":\"0\",\"n\":\"0\"}}\n{{\"a\":\"dX\",\"bal\":\"1\",\"n\":\"0\"}}\n"
        );
        assert_eq!(
            without_bound_key(&section, &address),
            "#W\n{\"a\":\"dX\",\"bal\":\"1\",\"n\":\"0\"}\n"
        );
        let funded = format!("#W\n{{\"a\":\"{a}\",\"pk\":\"{pk}\",\"bal\":\"5\",\"n\":\"0\"}}\n");
        assert_eq!(
            without_bound_key(&funded, &address),
            format!("#W\n{{\"a\":\"{a}\",\"bal\":\"5\",\"n\":\"0\"}}\n")
        );
    }
}
