//! The five levels of differential testing and where each one plugs in.
//!
//! | Level | What it compares | Where it plugs in | Arrives with |
//! |---|---|---|---|
//! | L0 | golden vectors against Heartwood, byte-exact and accept/reject | [`l0`]: records to [`crate::subject`] | `heartwood-crypto` (runs) |
//! | L1 | random and mutated cases, one oracle call each | [`l1`]: [`crate::oracle::OracleServer`] and the same subjects, driven by `proptest` | the state machine's operations |
//! | L2 | block sequences replayed on both sides, a state digest per block | [`l2`]: `heartwood-state`'s machine against the oracle's headless replay of the reference's state | the state machine |
//! | L3 | the same scenario on a reference devnet and a Heartwood devnet | a scenario runner over both devnets | the first Heartwood devnet |
//! | L4 | a mixed network of reference and Heartwood nodes | the scenario runner with mixed nodes | mixed networks |
//!
//! L0, L1 and L2 run: L0 and L1 with the subjects of [`crate::subjects`], L2 with the state
//! machine of `heartwood-state` and the judge's digest lines of a corpus ([`crate::corpus`]).

pub mod l0;
pub mod l1;
pub mod l2;
pub mod pool;

use std::fmt;
use std::str::FromStr;

use heartwood_crypto::identities::{Address, PublicKey};
use heartwood_crypto::utils::hex;
use serde_json::Value;

use crate::errors::Error;
use crate::fixture::Fixture;
use crate::report::Outcome;
use crate::subject::{Case, Registry, Verdict};
use crate::vectors::Expected;

/// The reason of a case that differs only by the key the reference binds for a refused sender
/// (see [`refused_unbound_sender`]).
pub const BOUND_KEY: &str = "a refused transaction from a sender without a public key: the \
     reference binds the key before its checks run and keeps it after the refusal, Heartwood \
     leaves the wallet as it was (the verdict and the error class are the same)";

/// A level of differential testing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    /// Golden vectors written by the reference.
    L0,
    /// Random and mutated cases, one oracle call each.
    L1,
    /// Block sequences replayed on both sides.
    L2,
    /// The same scenario on a reference devnet and a Heartwood devnet.
    L3,
    /// A mixed network of reference and Heartwood nodes.
    L4,
}

impl Level {
    /// Every level, in order.
    pub const ALL: [Level; 5] = [Level::L0, Level::L1, Level::L2, Level::L3, Level::L4];

    /// The level's title in reports.
    pub fn title(self) -> &'static str {
        match self {
            Level::L0 => "L0 golden vectors",
            Level::L1 => "L1 property tests against the Node oracle",
            Level::L2 => "L2 block-sequence replay and state digests",
            Level::L3 => "L3 live devnet scenarios",
            Level::L4 => "L4 mixed network of reference and Heartwood nodes",
        }
    }

    /// What the level first runs Heartwood code with.
    pub fn arrives_with(self) -> &'static str {
        match self {
            Level::L0 => "heartwood-crypto",
            Level::L1 => "the state machine's operations",
            Level::L2 => "the state machine",
            Level::L3 => "the first Heartwood devnet",
            Level::L4 => "mixed networks",
        }
    }

    /// Whether this build can run the level.
    pub fn is_available(self) -> bool {
        matches!(self, Level::L0 | Level::L1 | Level::L2)
    }
}

/// The sender of the transaction of a `state.applyTransaction` case when the reference refused
/// it and the case's fixture gives the sender no public key: the reference has then bound the
/// key (it binds it before its checks), where Heartwood leaves the state as it was.
pub fn refused_unbound_sender(case: &Case<'_>, expected: &Expected) -> Option<Address> {
    if case.op != "state.applyTransaction" {
        return None;
    }
    let Expected::Output(output) = expected else {
        return None;
    };
    output.get("error")?;
    let (sender, _) = sender_of(case)?;
    let fixture = Fixture::from_json(case.input.get("fixture")?).ok()?;
    (!has_key(&fixture, &sender)).then_some(sender)
}

/// The sender of the transaction of a `state.applyTransaction` case, and the network byte.
fn sender_of(case: &Case<'_>) -> Option<(Address, u8)> {
    let bytes = hex::decode(case.input.get("hex")?.as_str()?).ok()?;
    // A standard header: marker, version, network, type group (4), type (2), nonce (8), then
    // the sender's public key.
    let network = *bytes.get(2)?;
    let key = PublicKey::from_bytes(bytes.get(17..50)?).ok()?;
    Some((Address::from_public_key(&key, network), network))
}

/// Whether `fixture` gives the wallet at `address` a public key.
fn has_key(fixture: &Fixture, address: &Address) -> bool {
    fixture
        .wallets
        .iter()
        .any(|w| &w.address == address && w.public_key.is_some())
}

/// What is wrong when the `#W` section after the revert in a `state.applyTransaction` answer
/// (`output.reverted.W`) is not the fixture's own section; `None` when it is, or when the answer
/// holds no revert. A sender that the fixture gives no key keeps the key its transaction bound
/// while its nonce stays above 0, since a revert forgets a key only at nonce 0, on both sides; no
/// chain has such a wallet, so its key is left out of the comparison.
pub fn unrestored_fixture(case: &Case<'_>, output: &Value) -> Option<String> {
    let reverted = output.pointer("/reverted/W")?.as_str()?;
    let fixture = match Fixture::from_json(case.input.get("fixture")?) {
        Ok(fixture) => fixture,
        Err(e) => return Some(e.to_string()),
    };
    let (sender, network) = sender_of(case)?;
    let wallets = match fixture.to_wallets(network) {
        Ok(wallets) => wallets,
        Err(e) => return Some(e.to_string()),
    };
    let expected = heartwood_state::digest::wallet_section(&wallets);
    let reverted = if has_key(&fixture, &sender) {
        reverted.to_owned()
    } else {
        l1::without_bound_key(reverted, &sender)
    };
    if reverted == expected {
        return None;
    }
    let wallets = |w: &str| {
        crate::digests::sections_from_json(&serde_json::json!({
            "B": "", "S": "", "R": "", "M": "", "W": w,
        }))
    };
    let differences = match (wallets(&expected), wallets(&reverted)) {
        (Some(a), Some(b)) => crate::digests::section_differences(&a, &b).join("; "),
        _ => String::new(),
    };
    Some(format!(
        "the fixture (first) and the state after the revert differ: {differences}"
    ))
}

/// The oracle's answer to `case` with the documented differences applied: for a transaction the
/// reference refused whose sender had no key, the key it bound is left out of its wallets (see
/// [`refused_unbound_sender`] and [`l1::without_bound_key`]).
pub fn documented_expected(case: &Case<'_>, expected: &Expected) -> Expected {
    without_bound_sender_key(case, expected).unwrap_or_else(|| expected.clone())
}

/// The oracle's answer without the key it bound for a refused sender that had none, when that
/// changes the answer; `None` otherwise.
fn without_bound_sender_key(case: &Case<'_>, expected: &Expected) -> Option<Expected> {
    let sender = refused_unbound_sender(case, expected)?;
    let Expected::Output(output) = expected else {
        return None;
    };
    let w = output.pointer("/after/W").and_then(Value::as_str)?;
    let without = l1::without_bound_key(w, &sender);
    if without == w {
        return None;
    }
    let mut output = output.clone();
    output["after"]["W"] = Value::String(without);
    Some(Expected::Output(output))
}

/// Judge `case` against the oracle's `expected` answer as [`l0::judge_case`] does, with the
/// documented differences applied. A case that passes only once the key the reference bound for
/// a refused sender is left out counts as that documented difference ([`BOUND_KEY`]), so reports
/// list it. For a `state.applyTransaction` case that passes, the revert on each side must also
/// give back the fixture ([`unrestored_fixture`]); where it does not, the case is a harness
/// error, since the two sides then agree on a revert that is wrong.
pub fn judge_documented(case: &Case<'_>, expected: &Expected, registry: &Registry) -> Outcome {
    let outcome = match without_bound_sender_key(case, expected) {
        Some(adjusted) => match l0::judge_case(case, &adjusted, registry) {
            Outcome::Pass => Outcome::Documented(BOUND_KEY.to_owned()),
            other => other,
        },
        None => l0::judge_case(case, expected, registry),
    };
    if outcome.is_failure() || case.op != "state.applyTransaction" {
        return outcome;
    }
    let theirs = match expected {
        Expected::Output(output) => Some(output.clone()),
        Expected::Error(_) => None,
    };
    let ours = match registry.get(case.op).map(|subject| subject.run(case)) {
        Some(Ok(Verdict::Output(output))) => Some(output),
        _ => None,
    };
    for (side, output) in [("the reference", theirs), ("Heartwood", ours)] {
        if let Some(problem) = output.and_then(|output| unrestored_fixture(case, &output)) {
            return Outcome::HarnessError(format!(
                "{side}'s revert does not give back the fixture: {problem}"
            ));
        }
    }
    outcome
}

impl fmt::Display for Level {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Level::L0 => "l0",
            Level::L1 => "l1",
            Level::L2 => "l2",
            Level::L3 => "l3",
            Level::L4 => "l4",
        };
        f.write_str(name)
    }
}

impl FromStr for Level {
    type Err = Error;

    fn from_str(s: &str) -> Result<Level, Error> {
        Level::ALL
            .into_iter()
            .find(|l| l.to_string().eq_ignore_ascii_case(s))
            .ok_or_else(|| Error::Usage(format!("unknown level {s} (l0 to l4)")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refused_sender_without_a_key_loses_the_key_the_reference_bound() {
        use heartwood_crypto::Aux;
        use heartwood_crypto::transactions::builders::TransactionBuilder;
        use heartwood_crypto::transactions::types::TransferItem;
        use serde_json::json;

        use crate::strategies::transactions::TxContext;
        use crate::strategies::{keys, wallet_keys};

        let vectors = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../packages/crypto/tests/vectors");
        let set = crate::vectors::VectorSet::load(&vectors).unwrap();
        let devnet = crate::subjects::Devnet::from_vectors(&set)
            .unwrap()
            .unwrap();
        let context =
            TxContext::new(devnet.network.clone(), l1::devnet_milestones(&set).unwrap()).unwrap();
        let sender = wallet_keys(0);
        let address = Address::from_public_key(sender.public_key(), 90);
        let recipient = Address::from_public_key(keys("l1 recipient 0").public_key(), 90);
        let tx = TransactionBuilder::transfer(vec![TransferItem {
            amount: 1,
            recipient,
        }])
        .nonce(1)
        .fee(1)
        .build(&sender, Aux::fixed([1; 32]), context.milestones.at(2))
        .unwrap();
        let input = json!({
            "fixture": [{"a": address.to_base58(), "bal": "0", "n": "0"}],
            "hex": hex::encode(tx.bytes()),
            "lastBlockHeight": 1,
        });
        let case = Case {
            op: "state.applyTransaction",
            network: "devnet",
            height: 2,
            input: &input,
        };
        let bound = format!(
            "#W\n{{\"a\":\"{}\",\"pk\":\"{}\",\"bal\":\"0\",\"n\":\"0\"}}\n",
            address.to_base58(),
            sender.public_key().to_hex()
        );
        let refused = Expected::Output(
            json!({"error": {"class": "InsufficientBalanceError"}, "after": {"W": bound}}),
        );
        assert_eq!(refused_unbound_sender(&case, &refused), Some(address));
        let Expected::Output(out) = documented_expected(&case, &refused) else {
            panic!("an output");
        };
        assert_eq!(out["after"]["W"], json!("#W\n"));
        let applied = Expected::Output(json!({"applied": {"W": "#W\n"}}));
        assert_eq!(refused_unbound_sender(&case, &applied), None);
    }

    #[test]
    fn a_revert_must_give_back_the_fixture() {
        use heartwood_crypto::Aux;
        use heartwood_crypto::transactions::builders::TransactionBuilder;
        use heartwood_crypto::transactions::types::TransferItem;
        use serde_json::json;

        use crate::strategies::{keys, wallet_keys};

        let sender = wallet_keys(0);
        let address = Address::from_public_key(sender.public_key(), 90);
        let recipient = Address::from_public_key(keys("l1 recipient 0").public_key(), 90);
        let vectors = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../packages/crypto/tests/vectors");
        let set = crate::vectors::VectorSet::load(&vectors).unwrap();
        let context = crate::strategies::transactions::TxContext::new(
            crate::subjects::Devnet::from_vectors(&set)
                .unwrap()
                .unwrap()
                .network,
            l1::devnet_milestones(&set).unwrap(),
        )
        .unwrap();
        let tx = TransactionBuilder::transfer(vec![TransferItem {
            amount: 1,
            recipient,
        }])
        .nonce(4)
        .fee(1)
        .build(&sender, Aux::fixed([1; 32]), context.milestones.at(2))
        .unwrap();
        // The sender has no key in the fixture and a nonce of 3.
        let a = address.to_base58();
        let input = json!({
            "fixture": [{"a": a, "bal": "100", "n": "3"}],
            "hex": hex::encode(tx.bytes()),
            "lastBlockHeight": 1,
        });
        let case = Case {
            op: "state.applyTransaction",
            network: "devnet",
            height: 2,
            input: &input,
        };
        let pk = sender.public_key().to_hex();
        let reverted = |w: String| json!({"applied": {"W": "#W\n"}, "reverted": {"W": w}});
        // Given back, with or without the key the transaction bound.
        let plain = format!("#W\n{{\"a\":\"{a}\",\"bal\":\"100\",\"n\":\"3\"}}\n");
        assert_eq!(unrestored_fixture(&case, &reverted(plain)), None);
        let keyed = format!("#W\n{{\"a\":\"{a}\",\"pk\":\"{pk}\",\"bal\":\"100\",\"n\":\"3\"}}\n");
        assert_eq!(unrestored_fixture(&case, &reverted(keyed)), None);
        // A balance that the revert did not give back.
        let short = format!("#W\n{{\"a\":\"{a}\",\"bal\":\"99\",\"n\":\"3\"}}\n");
        let problem = unrestored_fixture(&case, &reverted(short)).unwrap();
        assert!(problem.contains("bal"), "{problem}");
        // No revert in the answer: nothing to check.
        assert_eq!(
            unrestored_fixture(&case, &json!({"error": {"class": "X"}})),
            None
        );
    }

    #[test]
    fn levels_parse_and_l0_to_l2_run() {
        assert_eq!("L2".parse::<Level>().unwrap(), Level::L2);
        assert!("l5".parse::<Level>().is_err());
        let available: Vec<Level> = Level::ALL
            .into_iter()
            .filter(|l| l.is_available())
            .collect();
        assert_eq!(available, vec![Level::L0, Level::L1, Level::L2]);
    }
}
