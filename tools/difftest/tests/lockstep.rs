//! The lockstep producer: a chain forged by Heartwood with a reference beside it that builds the
//! transactions, judges the pools' verdicts and applies every block.
//!
//! Most tests run against a twin: a reference made of Heartwood's own state, pool and builder,
//! which can also be told to answer another digest line or another verdict, so the producer's
//! stops are tested without the oracle. The integration tests (`#[ignore = "integration"]`) run
//! the chain against the real reference (`DIFFTEST_TEST_SOLAR_DIR` or `DIFFTEST_TEST_IMAGE`) and
//! through the judge and the replays.

use std::path::{Path, PathBuf};

use heartwood_crypto::blocks::factory;
use heartwood_crypto::identities::{Address, PublicKeyBytes};
use heartwood_crypto::transactions::builders::TransactionBuilder;
use heartwood_crypto::transactions::types::{Memo, TransferItem, Votes};
use heartwood_crypto::utils::hex;
use heartwood_crypto::{Aux, KeyPair};
use heartwood_difftest::cli::{LockstepArgs, OracleArgs};
use heartwood_difftest::corpus::{self, Corpus};
use heartwood_difftest::errors::Error;
use heartwood_difftest::lockstep::heart::Heart;
use heartwood_difftest::lockstep::reference::{BlockAnswer, BuiltTransaction, Reference};
use heartwood_difftest::lockstep::{self, LockstepError, Recipe};
use heartwood_difftest::store::ScratchDir;
use heartwood_kernel::config::crypto::{self, CryptoTexts};
use heartwood_state::digest::Step;
use serde_json::{Value, json};

const SEED: &str = "lockstep-test";

fn devnet() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packages/cli/src/profiles/devnet.json")
}

/// A reference made of Heartwood's own code.
#[derive(Default)]
struct Twin {
    heart: Option<Heart>,
    /// The height whose digest line the twin answers wrongly.
    wrong_line_at: Option<u32>,
    /// Whether the twin refuses the first transaction it accepts.
    wrong_verdict: bool,
    /// The digest lines of the blocks it applied: height and step.
    applied: Vec<(u32, String)>,
    /// Every result of its pool, as the producer read it.
    verdicts: Vec<Value>,
    /// The reverts it ran: the blocks reverted.
    reverts: Vec<u32>,
}

impl Twin {
    fn heart(&mut self) -> Result<&mut Heart, LockstepError> {
        self.heart
            .as_mut()
            .ok_or_else(|| LockstepError::Oracle("the twin is not open".into()))
    }
}

fn oracle_error(e: impl std::fmt::Display) -> LockstepError {
    LockstepError::Oracle(e.to_string())
}

fn text<'a>(spec: &'a Value, key: &str) -> Result<&'a str, LockstepError> {
    spec[key]
        .as_str()
        .ok_or_else(|| oracle_error(format!("the spec has no {key}")))
}

impl Reference for Twin {
    fn open(&mut self, crypto: &Value, _options: &Value) -> Result<Value, LockstepError> {
        let file = |name: &str| Some(crypto[name].to_string());
        let files = crypto::parse(&CryptoTexts {
            network: file("network"),
            milestones: file("milestones"),
            genesis: file("genesisBlock"),
            exceptions: file("exceptions"),
        })
        .map_err(oracle_error)?;
        let genesis =
            factory::from_json(&files.genesis, &files.milestones).map_err(oracle_error)?;
        let heart = Heart::open(&files, &genesis)?;
        let line = heart.genesis_line();
        self.heart = Some(heart);
        Ok(line)
    }

    fn build(
        &mut self,
        height: u32,
        specs: &[Value],
    ) -> Result<Vec<BuiltTransaction>, LockstepError> {
        let heart = self.heart()?;
        let params = heart.milestones.at(height);
        let mut out = Vec::new();
        for spec in specs {
            let asset = &spec["asset"];
            let mut builder = match text(spec, "kind")? {
                "transfer" => TransactionBuilder::transfer(
                    asset["recipients"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(|pair| {
                            Ok(TransferItem {
                                amount: pair[1]
                                    .as_str()
                                    .unwrap_or("0")
                                    .parse()
                                    .map_err(oracle_error)?,
                                recipient: Address::from_base58(
                                    pair[0].as_str().unwrap_or_default(),
                                )
                                .map_err(oracle_error)?,
                            })
                        })
                        .collect::<Result<_, LockstepError>>()?,
                ),
                "vote" => {
                    let mut votes = Votes::new();
                    for pair in asset["votes"].as_array().into_iter().flatten() {
                        let percent: u16 = pair[1]
                            .as_str()
                            .unwrap_or("0")
                            .parse()
                            .map_err(oracle_error)?;
                        votes.insert(
                            pair[0].as_str().unwrap_or_default().to_owned(),
                            percent * 100,
                        );
                    }
                    TransactionBuilder::vote(votes)
                }
                "secondSignature" => TransactionBuilder::second_signature(
                    KeyPair::from_passphrase(text(asset, "secondPassphrase")?)
                        .map_err(oracle_error)?
                        .public_key()
                        .to_compressed(),
                ),
                "delegateRegistration" => {
                    TransactionBuilder::delegate_registration(text(asset, "username")?)
                }
                "burn" => {
                    TransactionBuilder::burn(text(asset, "amount")?.parse().map_err(oracle_error)?)
                }
                other => return Err(oracle_error(format!("the twin does not build {other}"))),
            };
            builder = builder
                .nonce(text(spec, "nonce")?.parse().map_err(oracle_error)?)
                .network(params.network());
            if let Some(fee) = spec["fee"].as_str() {
                builder = builder.fee(fee.parse().map_err(oracle_error)?);
            }
            if let Some(memo) = spec["memo"].as_str() {
                builder = builder.memo(Memo::new(memo).map_err(oracle_error)?);
            }
            let keys = KeyPair::from_passphrase(text(spec, "passphrase")?).map_err(oracle_error)?;
            let transaction = builder
                .build(&keys, Aux::fixed([7; 32]), params)
                .map_err(oracle_error)?;
            out.push(BuiltTransaction {
                id: transaction.id().to_hex(),
                hex: hex::encode(transaction.bytes()),
            });
        }
        Ok(out)
    }

    fn submit(&mut self, now: i64, hexes: &[String]) -> Result<Value, LockstepError> {
        let bytes = hexes
            .iter()
            .map(|h| hex::decode(h).map_err(oracle_error))
            .collect::<Result<Vec<_>, _>>()?;
        let wrong = self.wrong_verdict;
        let mut result = self.heart()?.submit(now, bytes)?;
        if wrong {
            self.wrong_verdict = false;
            if let Some(id) = result["accept"].get(0).cloned() {
                result["accept"] = json!([]);
                result["invalid"] = json!([id.clone()]);
                result["errors"] = json!({ id.as_str().unwrap_or_default(): {"type": "ERR_APPLY", "message": "refused"} });
            }
        }
        self.verdicts.push(result.clone());
        Ok(result)
    }

    fn online(&mut self, round: u64, delegates: &[String]) -> Result<(), LockstepError> {
        let keys = delegates
            .iter()
            .map(|d| PublicKeyBytes::from_hex(d).map_err(oracle_error))
            .collect::<Result<Vec<_>, _>>()?;
        self.heart()?.set_online(round, &keys);
        Ok(())
    }

    fn apply(
        &mut self,
        hex: &str,
        step: &str,
        _expire_at: Option<u64>,
    ) -> Result<BlockAnswer, LockstepError> {
        let bytes = heartwood_crypto::utils::hex::decode(hex).map_err(oracle_error)?;
        let which = if step == "reapply" {
            Step::Reapply
        } else {
            Step::Apply
        };
        let wrong_at = self.wrong_line_at;
        let heart = self.heart()?;
        let posted = heart.apply(&bytes, which)?;
        let height = heart.height();
        let mut line = posted.line;
        if wrong_at == Some(height) {
            line["id"] = json!("00");
        }
        self.applied.push((height, step.to_owned()));
        Ok(BlockAnswer {
            accepted: true,
            line: Some(line),
            refusal: None,
        })
    }

    fn revert(&mut self, count: u32) -> Result<Vec<Value>, LockstepError> {
        self.reverts.push(count);
        Ok(self.heart()?.revert(count)?.0)
    }

    fn close(&mut self) -> Result<(), LockstepError> {
        Ok(())
    }
}

fn transfer(sender: &str, to: &[&str], amount: &str) -> Value {
    json!({"kind": "transfer", "sender": sender,
        "asset": {"recipients": to.iter().map(|t| json!([t, amount])).collect::<Vec<_>>()}})
}

/// A recipe the twin can build: funding, an attested vote, a vote for a delegate that is not
/// attested, a registration, a burn and a second signature; more steps when `more` are given.
fn recipe(more: Vec<Value>) -> Recipe {
    let mut steps: Vec<Value> = (1..=5)
        .map(|round| {
            json!({"step": "online", "round": round,
                "delegates": {"allBut": ["genesis_51", "genesis_52", "genesis_53"]}})
        })
        .collect();
    steps.push(json!({"step": "transactions", "height": 2,
        "build": [transfer("genesis-1", &["alice", "bob", "carol", "dave"], "1000000000000")]}));
    steps.push(json!({"step": "transactions", "height": 3, "build": [
        {"kind": "vote", "sender": "alice", "asset": {"votes": [["genesis_1", "100"]]}},
        {"kind": "vote", "sender": "bob", "asset": {"votes": [["genesis_52", "100"]]},
         "refused": "ERR_OFFLINE"},
        {"kind": "delegateRegistration", "sender": "carol", "fee": "7600000000", "asset": {"username": "lockstep_1"}},
        {"kind": "transfer", "sender": "dave", "memo": "lockstep",
         "asset": {"recipients": [["alice", "250000000"]]}},
    ]}));
    steps.push(json!({"step": "transactions", "height": 5, "build": [
        {"kind": "burn", "sender": "dave", "asset": {"amount": "100000000"}},
        {"kind": "secondSignature", "sender": "bob",
         "asset": {"secondPassphrase": "lockstep second passphrase"}}]}));
    steps.extend(more);
    serde_json::from_value(json!({"wallets": ["alice", "bob", "carol", "dave"], "steps": steps}))
        .expect("the recipe reads")
}

fn make_in(
    twin: &mut Twin,
    dir: &Path,
    profile: &Path,
    recipe: &Recipe,
) -> Result<lockstep::MadeChain, LockstepError> {
    lockstep::make_with(twin, SEED, profile.to_str().expect("utf-8"), 5, recipe, dir)
}

fn files(dir: &Path) -> Vec<(String, Vec<u8>)> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) {
        let mut entries: Vec<_> = std::fs::read_dir(dir)
            .expect("a directory")
            .map(|e| e.expect("entry").path())
            .collect();
        entries.sort();
        for path in entries {
            if path.is_dir() {
                walk(root, &path, out);
            } else {
                let name = path
                    .strip_prefix(root)
                    .expect("inside")
                    .to_string_lossy()
                    .into_owned();
                out.push((name, std::fs::read(&path).expect("a file")));
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out
}

#[test]
fn a_five_round_chain_is_reproduced_byte_for_byte() {
    let scratch = ScratchDir::new("lockstep-test").expect("scratch");
    let (a, b) = (scratch.path().join("a"), scratch.path().join("b"));
    let recipe = recipe(Vec::new());
    let made = make_in(&mut Twin::default(), &a, &devnet(), &recipe).expect("the first run");
    let again = make_in(&mut Twin::default(), &b, &devnet(), &recipe).expect("the second run");
    assert_eq!((made.tip, made.transactions, made.missed), (265, 6, 0));
    assert_eq!(
        (again.tip, again.transactions),
        (made.tip, made.transactions)
    );
    let (first, second) = (files(&a), files(&b));
    assert_eq!(
        first.iter().map(|f| &f.0).collect::<Vec<_>>(),
        second.iter().map(|f| &f.0).collect::<Vec<_>>()
    );
    for (one, two) in first.iter().zip(&second) {
        assert!(one.1 == two.1, "{} differs between the two runs", one.0);
    }
    let loaded = Corpus::load(&a).expect("an ordinary corpus");
    corpus::verify(&a).expect("its manifest holds");
    assert_eq!(loaded.tip(), 265);
    assert_eq!(loaded.meta["anchorSource"], "heartwood-replay");
    assert_eq!(loaded.meta["seed"], SEED);
    assert_eq!(loaded.meta["forgerAux"].as_str().map(str::len), Some(64));
    assert!(loaded.restart_plan.is_some());
    // An ordinary corpus's revert plan: every round end and the random reverts of its seed.
    let seed: u64 = loaded.meta["revertSeed"]
        .as_str()
        .and_then(|s| s.parse().ok())
        .expect("a revert seed");
    let generated = corpus::RevertPlan::generate(seed, 265, 53);
    assert!(!generated.steps.is_empty());
    assert_eq!(loaded.revert_plan.steps.len(), generated.steps.len());
    let adjusted = loaded.meta["revertPlanAdjusted"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    for step in &generated.steps {
        let now = adjusted
            .iter()
            .find(|a| a["after"] == step.after_height && a["blocks"] == step.blocks)
            .and_then(|a| a["now"].as_u64())
            .map_or(step.blocks, |n| u32::try_from(n).expect("a count"));
        assert!(
            loaded
                .revert_plan
                .steps
                .iter()
                .any(|s| s.after_height == step.after_height && s.blocks == now),
            "{step:?}"
        );
    }
    // None returns to a round's last block, whose next round's rows the judge's node deletes.
    assert!(
        loaded
            .revert_plan
            .steps
            .iter()
            .all(|s| (s.after_height - s.blocks) % 53 != 0)
    );
    assert_eq!(
        loaded.meta["revertSteps"],
        json!(loaded.revert_plan.steps.len())
    );
}

#[test]
fn every_block_is_accepted_and_the_pools_agree_on_every_transaction() {
    let scratch = ScratchDir::new("lockstep-test").expect("scratch");
    let mut twin = Twin::default();
    make_in(
        &mut twin,
        &scratch.path().join("c"),
        &devnet(),
        &recipe(Vec::new()),
    )
    .expect("the chain");
    // One line per block, each accepted with equal lines (the producer stops otherwise).
    let heights: Vec<u32> = twin.applied.iter().map(|(h, _)| *h).collect();
    assert_eq!(heights, (2..=265).collect::<Vec<u32>>());
    let submitted: Vec<&Value> = twin.verdicts.iter().collect();
    assert_eq!(
        submitted.len(),
        3,
        "funding, the votes and registration, the burn and second signature"
    );
    let accepted = |v: &Value| v["accept"].as_array().map_or(0, Vec::len);
    let refused = |v: &Value| v["errors"].as_object().map_or(0, |m| m.len());
    assert_eq!((accepted(submitted[0]), refused(submitted[0])), (1, 0));
    // The attested vote, the registration and the transfer are admitted; the vote for a delegate
    // that is not attested is refused, as offline.
    assert_eq!((accepted(submitted[1]), refused(submitted[1])), (3, 1));
    let message = submitted[1]["errors"]
        .as_object()
        .and_then(|m| m.values().next())
        .map(|e| {
            (
                e["type"].clone(),
                e["message"].as_str().unwrap_or_default().to_owned(),
            )
        })
        .expect("an error");
    assert_eq!(message.0, "ERR_APPLY");
    assert!(
        message
            .1
            .ends_with("is not operating a node on the network"),
        "{}",
        message.1
    );
    assert_eq!((accepted(submitted[2]), refused(submitted[2])), (2, 0));
}

#[test]
fn a_revert_across_a_round_end_a_missed_pattern_and_a_milestone_go_through() {
    let scratch = ScratchDir::new("lockstep-test").expect("scratch");
    // The donations start inside the chain: a milestone at height 120.
    let text = std::fs::read_to_string(devnet()).expect("the profile");
    assert!(text.contains("\"donationsFromHeight\": 2,"));
    let profile = scratch.path().join("late.json");
    std::fs::write(
        &profile,
        text.replace(
            "\"donationsFromHeight\": 2,",
            "\"donationsFromHeight\": 120,",
        ),
    )
    .expect("write");
    let more = vec![
        json!({"step": "transactions", "height": 52, "build": [transfer("dave", &["alice"], "1000000")]}),
        json!({"step": "transactions", "height": 53, "build": [transfer("dave", &["bob"], "1000000")]}),
        json!({"step": "transactions", "height": 109, "build": [transfer("dave", &["carol"], "1000000")]}),
        json!({"step": "missed", "delegate": "genesis_5", "fromHeight": 20, "toHeight": 60}),
        json!({"step": "revert", "afterHeight": 53, "blocks": 2}),
        json!({"step": "fork", "afterHeight": 110, "blocks": 3}),
        // A second fork over the first one's replacement, whose first block missed a slot.
        json!({"step": "fork", "afterHeight": 110, "blocks": 3}),
        json!({"step": "restart", "height": 54}),
    ];
    let dir = scratch.path().join("r");
    let mut twin = Twin::default();
    let made = make_in(&mut twin, &dir, &profile, &recipe(more)).expect("the chain");
    assert_eq!(made.tip, 265);
    assert!(made.missed >= 1, "the stopped delegate misses a slot");
    // A revert across the end of round 1, and the forks' reverts.
    assert_eq!(twin.reverts, vec![2, 3, 3]);
    let reapplied = twin.applied.iter().filter(|(_, s)| s == "reapply").count();
    assert_eq!(reapplied, 2);
    let loaded = Corpus::load(&dir).expect("the corpus loads");
    // The recipe's revert joins the round-end and random reverts of an ordinary corpus.
    let steps: Vec<(u32, u32)> = loaded
        .revert_plan
        .steps
        .iter()
        .map(|s| (s.after_height, s.blocks))
        .collect();
    assert!(steps.contains(&(53, 2)), "{steps:?}");
    for end in [53, 106, 159, 212, 265] {
        assert!(steps.contains(&(end, 1)), "{steps:?}");
    }
    assert!(steps.windows(2).all(|w| w[0].0 <= w[1].0), "{steps:?}");
    // The missed slots counted are those of the chain written, not of the blocks a fork replaced.
    let missed_rows = std::fs::read_to_string(dir.join("reference/missed-blocks.jsonl"))
        .expect("the missed rows")
        .lines()
        .count();
    assert_eq!(made.missed, missed_rows as u64);
    assert!(
        loaded
            .restart_plan
            .as_ref()
            .is_some_and(|p| p.restarts_at(54))
    );
    // The donations of the milestone are in the chain from 120.
    let rows =
        std::fs::read_to_string(dir.join("reference/meta.jsonl")).expect("the block columns");
    let donated = |h: u64| {
        rows.lines()
            .map(|l| serde_json::from_str::<Value>(l).expect("a row"))
            .find(|r| r["h"] == h)
            .map(|r| r["donations"].as_object().map_or(0, |d| d.len()))
    };
    assert_eq!((donated(119), donated(120)), (Some(0), Some(2)));
}

#[test]
fn a_delegate_the_recipe_registers_forges_its_slots_once_it_is_active() {
    let scratch = ScratchDir::new("lockstep-test").expect("scratch");
    let all_but = json!({"allBut": ["genesis_51", "genesis_52", "genesis_53"]});
    // Round 2 attests the new delegate too, so that a vote for it is admitted.
    let mut round_two: Vec<String> = (1..=50).map(|i| format!("genesis_{i}")).collect();
    round_two.push("lockstep_1".into());
    let mut steps = vec![
        json!({"step": "online", "round": 1, "delegates": all_but}),
        json!({"step": "online", "round": 2, "delegates": {"only": round_two}}),
    ];
    steps.extend(
        (3..=5).map(|round| json!({"step": "online", "round": round, "delegates": all_but})),
    );
    steps.push(json!({"step": "transactions", "height": 2,
        "build": [transfer("genesis-1", &["carol", "dave"], "1000000000000")]}));
    steps.push(json!({"step": "transactions", "height": 3, "build": [
        {"kind": "delegateRegistration", "sender": "carol", "fee": "7600000000",
         "asset": {"username": "lockstep_1"}}]}));
    steps.push(json!({"step": "transactions", "height": 55, "build": [
        {"kind": "vote", "sender": "dave", "asset": {"votes": [["lockstep_1", "100"]]}}]}));
    let recipe: Recipe =
        serde_json::from_value(json!({"wallets": ["carol", "dave"], "steps": steps}))
            .expect("the recipe reads");
    let dir = scratch.path().join("n");
    let made = make_in(&mut Twin::default(), &dir, &devnet(), &recipe).expect("the chain");
    assert_eq!(
        made.missed, 0,
        "the new delegate's slots are forged, not missed"
    );
    let rows =
        std::fs::read_to_string(dir.join("reference/meta.jsonl")).expect("the block columns");
    let forged = rows
        .lines()
        .map(|l| serde_json::from_str::<Value>(l).expect("a row"))
        .filter(|r| r["username"] == "lockstep_1")
        .count();
    assert_eq!(forged, 3, "one block in each of rounds 3 to 5");
}

#[test]
fn the_producer_stops_when_the_reference_answers_another_digest() {
    let scratch = ScratchDir::new("lockstep-test").expect("scratch");
    let mut twin = Twin {
        wrong_line_at: Some(7),
        ..Twin::default()
    };
    let error = make_in(
        &mut twin,
        &scratch.path().join("d"),
        &devnet(),
        &recipe(Vec::new()),
    )
    .expect_err("a stop");
    match error {
        LockstepError::Block { height, detail } => {
            assert_eq!(height, 7);
            assert!(!detail.is_empty());
        }
        other => panic!("another stop: {other}"),
    }
    assert_eq!(
        twin.applied.last().map(|(h, _)| *h),
        Some(7),
        "it stopped at that block"
    );
    assert!(
        !scratch.path().join("d").exists(),
        "no corpus of a chain that disagreed"
    );
}

#[test]
fn the_producer_stops_when_the_reference_answers_another_verdict() {
    let scratch = ScratchDir::new("lockstep-test").expect("scratch");
    let mut twin = Twin {
        wrong_verdict: true,
        ..Twin::default()
    };
    let error = make_in(
        &mut twin,
        &scratch.path().join("e"),
        &devnet(),
        &recipe(Vec::new()),
    )
    .expect_err("a stop");
    match error {
        LockstepError::Pool { id, detail } => {
            assert_eq!(id.len(), 64, "it names the transaction");
            assert!(detail.contains("Heartwood answers"), "{detail}");
        }
        other => panic!("another stop: {other}"),
    }
}

#[test]
fn a_recipe_the_chain_cannot_follow_is_refused_before_anything_is_made() {
    let scratch = ScratchDir::new("lockstep-test").expect("scratch");
    let beyond = recipe(vec![json!({"step": "restart", "height": 9999})]);
    let mut twin = Twin::default();
    let error =
        make_in(&mut twin, &scratch.path().join("f"), &devnet(), &beyond).expect_err("refused");
    assert!(matches!(error, LockstepError::Recipe(_)), "{error}");
    assert!(twin.heart.is_none(), "the reference was not started");
    let unknown = recipe(vec![json!({"step": "transactions", "height": 4, "build": [
        {"kind": "burn", "sender": "nobody", "asset": {"amount": "1"}}]})]);
    let error = make_in(
        &mut Twin::default(),
        &scratch.path().join("g"),
        &devnet(),
        &unknown,
    )
    .expect_err("refused");
    assert!(matches!(error, LockstepError::Recipe(m) if m.contains("nobody")));
    // A revert back to a round's last block: the plan's replay would then run on a judge whose
    // node deleted the next round's rows.
    let round_end = recipe(vec![
        json!({"step": "revert", "afterHeight": 56, "blocks": 3}),
    ]);
    let mut twin = Twin::default();
    let error =
        make_in(&mut twin, &scratch.path().join("i"), &devnet(), &round_end).expect_err("refused");
    assert!(
        matches!(&error, LockstepError::Recipe(m) if m.contains("last block of a round")),
        "{error}"
    );
    assert!(twin.heart.is_none(), "the reference was not started");
}

#[test]
fn a_chain_longer_than_five_rounds_needs_the_long_flag() {
    let scratch = ScratchDir::new("lockstep-test").expect("scratch");
    let args = |rounds, long| LockstepArgs {
        oracle: OracleArgs::default(),
        seed: SEED.into(),
        profile: "devnet".into(),
        rounds,
        recipe: scratch.path().join("none.json"),
        out: scratch.path().join("h"),
        long,
    };
    let refused = lockstep::run(&args(6, false)).expect_err("six rounds");
    assert!(
        matches!(&refused, Error::Usage(m) if m.contains("--long")),
        "{refused}"
    );
    let refused = lockstep::run(&args(51, true)).expect_err("fifty-one rounds");
    assert!(matches!(&refused, Error::Usage(_)), "{refused}");
    // Within the limit the recipe is read next.
    let refused = lockstep::run(&args(5, false)).expect_err("no recipe file");
    assert!(matches!(&refused, Error::Io { .. }), "{refused}");
}

// The integration tests: the real reference.

fn oracle_flags() -> Option<Vec<String>> {
    if let Ok(dir) = std::env::var("DIFFTEST_TEST_SOLAR_DIR") {
        return Some(vec!["--solar-dir".into(), dir]);
    }
    std::env::var("DIFFTEST_TEST_IMAGE")
        .ok()
        .map(|image| vec!["--image".into(), image])
}

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
#[ignore = "integration"]
fn a_chain_with_the_real_reference_goes_through_the_judge_and_the_replays() {
    let Some(oracle) = oracle_flags() else {
        eprintln!("no test oracle configured");
        return;
    };
    let scratch = ScratchDir::new("lockstep-live").expect("scratch");
    let dir = scratch.path().join("chain");
    let d = dir.to_str().expect("utf-8");
    let recipe = Path::new(env!("CARGO_MANIFEST_DIR")).join("recipes/lockstep-5.json");
    let (ok, made) = difftest(
        &[
            "lockstep",
            "make",
            "--seed",
            SEED,
            "--profile",
            devnet().to_str().expect("utf-8"),
            "--rounds",
            "5",
            "--recipe",
            recipe.to_str().expect("utf-8"),
            "--out",
            d,
        ],
        &oracle,
    );
    assert!(ok, "{made}");
    assert!(
        made.contains("to height 265, 8 transactions, 0 missed slots"),
        "{made}"
    );
    let (ok, judged) = difftest(&["corpus", "judge", d, "--processor"], &oracle);
    assert!(ok, "{judged}");
    assert!(
        judged.contains("A4 and A5 do not apply"),
        "the anchor source is read by the judge: {judged}"
    );
    assert!(judged.contains("A1 ok, A2 ok, A3 ok"), "{judged}");
    let meta = std::fs::read_to_string(dir.join("corpus.json")).expect("corpus.json");
    assert!(meta.contains("\"anchorSource\": \"heartwood-replay\""));
    for extra in [
        &["--strict"][..],
        &["--restart", "--strict"],
        &["--processor", "--strict"],
    ] {
        let mut args = vec!["run", "l2", d];
        args.extend(extra);
        let (ok, out) = difftest(&args, &[]);
        assert!(ok, "l2 {extra:?}: {out}");
        assert!(out.contains("0 failed"), "{out}");
    }
    let (ok, out) = difftest(
        &["run", "l2", d, "--forge", "--oracle", "--strict"],
        &oracle,
    );
    assert!(ok, "the forge replay through the reference: {out}");
    assert!(out.contains("264 compared: 264 passed, 0 failed"), "{out}");
}

#[test]
#[ignore = "integration"]
fn a_chain_with_reverts_missed_slots_and_a_milestone_goes_through_the_real_reference() {
    let Some(oracle) = oracle_flags() else {
        eprintln!("no test oracle configured");
        return;
    };
    let scratch = ScratchDir::new("lockstep-live").expect("scratch");
    let text = std::fs::read_to_string(devnet()).expect("the profile");
    let profile = scratch.path().join("late.json");
    std::fs::write(
        &profile,
        text.replace(
            "\"donationsFromHeight\": 2,",
            "\"donationsFromHeight\": 120,",
        ),
    )
    .expect("write");
    let dir = scratch.path().join("chain");
    let d = dir.to_str().expect("utf-8");
    let recipe = Path::new(env!("CARGO_MANIFEST_DIR")).join("recipes/lockstep-revert.json");
    let (ok, made) = difftest(
        &[
            "lockstep",
            "make",
            "--seed",
            SEED,
            "--profile",
            profile.to_str().expect("utf-8"),
            "--rounds",
            "5",
            "--recipe",
            recipe.to_str().expect("utf-8"),
            "--out",
            d,
        ],
        &oracle,
    );
    assert!(ok, "{made}");
    let (ok, judged) = difftest(&["corpus", "judge", d, "--processor"], &oracle);
    assert!(ok, "{judged}");
    // Every revert of the plan (the recipe's, the round ends' and the random ones) is judged.
    let plan = Corpus::load(&dir).expect("the corpus").revert_plan;
    assert!(
        plan.steps
            .iter()
            .any(|s| (s.after_height, s.blocks) == (53, 2))
    );
    let reverted: u32 = plan.steps.iter().map(|s| s.blocks).sum();
    assert!(
        judged.contains(&format!("({reverted} reverts, {reverted} re-applies)")),
        "{judged}"
    );
    assert!(judged.contains("A1 ok, A2 ok, A3 ok"), "{judged}");
    let (ok, out) = difftest(&["run", "l2", d, "--strict"], &[]);
    assert!(ok, "{out}");
}
