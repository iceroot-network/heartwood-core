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

use std::fmt;
use std::str::FromStr;

use heartwood_crypto::identities::{Address, PublicKey};
use heartwood_crypto::utils::hex;
use serde_json::Value;

use crate::errors::Error;
use crate::fixture::Fixture;
use crate::subject::Case;
use crate::vectors::Expected;

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
    let bytes = hex::decode(case.input.get("hex")?.as_str()?).ok()?;
    // A standard header: marker, version, network, type group (4), type (2), nonce (8), then
    // the sender's public key.
    let network = *bytes.get(2)?;
    let key = PublicKey::from_bytes(bytes.get(17..50)?).ok()?;
    let sender = Address::from_public_key(&key, network);
    let fixture = Fixture::from_json(case.input.get("fixture")?).ok()?;
    let bound = fixture
        .wallets
        .iter()
        .any(|w| w.address == sender && w.public_key.is_some());
    (!bound).then_some(sender)
}

/// The oracle's answer to `case` with the documented differences applied: for a transaction the
/// reference refused whose sender had no key, the key it bound is left out of its wallets (see
/// [`refused_unbound_sender`] and [`l1::without_bound_key`]).
pub fn documented_expected(case: &Case<'_>, expected: &Expected) -> Expected {
    let Some(sender) = refused_unbound_sender(case, expected) else {
        return expected.clone();
    };
    let mut expected = expected.clone();
    if let Expected::Output(output) = &mut expected
        && let Some(w) = output.pointer("/after/W").and_then(Value::as_str)
    {
        let w = l1::without_bound_key(w, &sender);
        output["after"]["W"] = Value::String(w);
    }
    expected
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
