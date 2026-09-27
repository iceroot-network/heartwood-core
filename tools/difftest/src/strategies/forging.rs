//! The cases of P17, forging: the pool fixtures and random pools of P10, a random round (previous
//! block, slot, reward), a delegate key and an aux, for `forger.collateAndForge` (the collated
//! ids, the removed ids and the block's bytes must be the reference's).
//!
//! A case is a P10 session whose own collation at the end is replaced by the forger's (validate
//! on, nothing excluded), with the forger's inputs: a secret key, an aux, the previous block's id,
//! a timestamp and a reward. The reward is most often the devnet's, sometimes 0 or any 64-bit
//! value, so that a reward beyond the schema's maximum is refused on both sides.

use std::sync::Arc;

use heartwood_crypto::SecretKey;
use heartwood_crypto::utils::hex;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use serde_json::{Value, json};

use super::kernel::OpCase;
use super::pool::pool_cases;
use super::transactions::TxContext;

/// The forger's inputs of a case.
#[derive(Debug, Clone)]
struct Round {
    key: [u8; 32],
    aux: [u8; 32],
    previous: [u8; 32],
    timestamp: u32,
    reward: u64,
}

fn round() -> impl Strategy<Value = Round> {
    let reward = prop_oneof![
        6 => Just(180_000_000u64),
        1 => Just(0u64),
        1 => any::<u64>(),
    ];
    (
        any::<[u8; 32]>(),
        any::<[u8; 32]>(),
        any::<[u8; 32]>(),
        any::<u32>(),
        reward,
    )
        .prop_filter("a secret key", |(key, ..)| {
            SecretKey::from_bytes(*key).is_ok()
        })
        .prop_map(|(key, aux, previous, timestamp, reward)| Round {
            key,
            aux,
            previous,
            timestamp,
            reward,
        })
}

/// A P10 session as a forger's collation, with `round`'s inputs.
fn forging_case((_, height, mut input): OpCase, round: &Round) -> OpCase {
    if let Some(steps) = input.get_mut("steps").and_then(Value::as_array_mut)
        && steps.last().is_some_and(|step| step["op"] == "collate")
    {
        steps.pop();
    }
    input["privateKey"] = json!(hex::encode(&round.key));
    input["aux"] = json!(hex::encode(&round.aux));
    input["previousBlock"] = json!({"id": hex::encode(&round.previous)});
    input["timestamp"] = json!(round.timestamp);
    input["reward"] = json!(round.reward.to_string());
    ("forger.collateAndForge", height, input)
}

/// The cases of P17.
pub fn forging_cases(context: &Arc<TxContext>) -> BoxedStrategy<OpCase> {
    (pool_cases(context.clone()), round())
        .prop_map(|(case, round)| forging_case(case, &round))
        .boxed()
}

#[cfg(test)]
mod tests {
    use proptest::strategy::ValueTree;
    use proptest::test_runner::TestRunner;

    use super::*;

    fn context() -> Arc<TxContext> {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../packages/crypto/tests/vectors");
        let set = crate::vectors::VectorSet::load(&dir).unwrap();
        let devnet = crate::subjects::Devnet::from_vectors(&set)
            .unwrap()
            .unwrap();
        let text = crate::levels::l1::devnet_milestones(&set).unwrap();
        Arc::new(TxContext::new(devnet.network, text).unwrap())
    }

    #[test]
    fn cases_are_pool_sessions_with_the_forgers_inputs() {
        let mut runner = TestRunner::deterministic();
        let strategy = forging_cases(&context());
        let mut rewards = std::collections::BTreeSet::new();
        for _ in 0..100 {
            let (op, height, input) = strategy.new_tree(&mut runner).unwrap().current();
            assert_eq!(op, "forger.collateAndForge");
            assert_eq!(height, input["lastBlockHeight"].as_u64().unwrap() + 1);
            let steps = input["steps"].as_array().unwrap();
            assert!(steps.iter().all(|step| step["op"] != "collate"));
            let key = input["privateKey"].as_str().unwrap();
            assert!(SecretKey::from_hex(key).is_ok());
            assert_eq!(input["aux"].as_str().unwrap().len(), 64);
            assert_eq!(input["previousBlock"]["id"].as_str().unwrap().len(), 64);
            rewards.insert(input["reward"].as_str().unwrap().to_owned());
        }
        assert!(
            rewards.contains("180000000") && rewards.len() > 2,
            "{rewards:?}"
        );
    }
}
