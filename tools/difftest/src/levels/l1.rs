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
use proptest::strategy::{BoxedStrategy, Strategy};
use proptest::test_runner::{Config, RngAlgorithm, TestCaseError, TestError, TestRng, TestRunner};
use serde_json::{Value, json};

use crate::errors::{Error, Result};
use crate::fixture::Fixture;
use crate::oracle::OracleServer;
use crate::report::{CaseResult, Outcome, Report};
use crate::strategies::transactions::{TxContext, fee_cases, handler_cases};
use crate::strategies::{fixtures, kernel, state};
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
}

impl Property {
    /// Every property.
    pub const ALL: [Property; 12] = [
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
        }
    }

    /// The property's short name, P1 to P12, which the command line accepts too.
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
    }
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
    /// accepted entries and the full-pool evictions.
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
        if matches!(*property, Property::Handlers | Property::Pool) {
            for (class, n) in seen.into_inner() {
                *classes.entry(class).or_insert(0) += n;
            }
        }
        let recorded = recorded.into_inner();
        let file = format!("l1/{}", property.name());
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
    let report = Report {
        title: super::Level::L1.title().to_string(),
        source: format!(
            "{} cases per property, seed {}, properties {}",
            options.cases,
            options.seed,
            options
                .properties
                .iter()
                .map(|p| p.number())
                .collect::<Vec<_>>()
                .join(" ")
        ),
        files: options.properties.len(),
        results,
    };
    Ok(L1Run { report, classes })
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
        assert!("P13".parse::<Property>().is_err());
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
