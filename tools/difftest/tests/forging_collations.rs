//! A forger's collation after a pool session in which the two pools came to hold different
//! transactions on purpose: minimal cases of P17 (`forger.collateAndForge`), each with the
//! reference's answer as the oracle gives it, the session it ran in the notes. Each case is the
//! pool's documented difference it names, and only while the forging answers differ as the pools
//! do: a block or a collation that the pools' difference does not explain fails.

use std::path::Path;

use heartwood_crypto::Transaction;
use heartwood_crypto::utils::hex;
use heartwood_difftest::levels::{self, l1};
use heartwood_difftest::report::Outcome;
use heartwood_difftest::subject::{self, Case, Documented, Registry, Verdict};
use heartwood_difftest::subjects::{Devnet, collation_difference};
use heartwood_difftest::vectors::{Expected, VectorSet};
use serde_json::{Value, json};

/// The devnet of the committed vectors.
fn devnet() -> Devnet {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packages/crypto/tests/vectors");
    let set = VectorSet::load(&dir).expect("the committed vectors load");
    l1::devnet_milestones(&set).expect("the devnet milestone file");
    Devnet::from_vectors(&set)
        .unwrap()
        .expect("a genesis class")
}

fn registry() -> Registry {
    subject::heartwood(Some(devnet()))
}

/// The cases: `{op, network, height, name, difference, input, output}`, one per line.
fn cases() -> Vec<Value> {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/forging/collations.jsonl");
    let text = std::fs::read_to_string(&path).expect("the cases are committed");
    text.lines()
        .map(|line| serde_json::from_str(line).expect("a case is JSON"))
        .collect()
}

/// The judge's outcome for `record`'s input with `output` as the reference's answer.
fn judged(record: &Value, output: Value, registry: &Registry) -> Outcome {
    let case = Case {
        op: record["op"].as_str().unwrap(),
        network: record["network"].as_str().unwrap(),
        height: record["height"].as_u64().unwrap(),
        input: &record["input"],
    };
    levels::judge_documented(&case, &Expected::Output(output), registry)
}

/// Heartwood's own answer to `record`'s input.
fn heartwood(record: &Value, registry: &Registry) -> Value {
    let case = Case {
        op: record["op"].as_str().unwrap(),
        network: record["network"].as_str().unwrap(),
        height: record["height"].as_u64().unwrap(),
        input: &record["input"],
    };
    match registry.get(case.op).unwrap().run(&case) {
        Ok(Verdict::Output(output)) => output,
        other => panic!("{}: {other:?}", record["name"]),
    }
}

#[test]
fn each_case_is_the_pools_documented_difference_it_names() {
    let registry = registry();
    let cases = cases();
    assert_eq!(cases.len(), 14);
    let wrong: Vec<(&Value, Outcome)> = cases
        .iter()
        .filter_map(|record| {
            let difference = match record["difference"].as_str() {
                Some("fullPoolEviction") => Documented::FullPoolEviction,
                Some("onlineView") => Documented::OnlineView,
                other => panic!("unknown difference {other:?}"),
            };
            let outcome = judged(record, record["output"].clone(), &registry);
            let documented = Outcome::Documented(difference.reason().into());
            (outcome != documented).then_some((&record["name"], outcome))
        })
        .collect();
    assert!(wrong.is_empty(), "{wrong:#?}");
    // In each, the collations differ, and so do the blocks.
    for record in &cases {
        let ours = heartwood(record, &registry);
        assert_ne!(
            ours["collated"], record["output"]["collated"],
            "{}",
            record["name"]
        );
        assert_ne!(ours["id"], record["output"]["id"], "{}", record["name"]);
    }
}

#[test]
fn a_forging_difference_the_pools_do_not_explain_fails() {
    let registry = registry();
    for record in &cases() {
        let name = &record["name"];
        // The reference's block with its last byte changed: not the block Heartwood's forger makes
        // from the reference's collated transactions.
        let mut block = record["output"].clone();
        let hex = block["hex"].as_str().unwrap().to_owned();
        let last = if hex.ends_with('0') { "1" } else { "0" };
        block["hex"] = Value::from(format!("{}{last}", &hex[..hex.len() - 1]));
        assert!(judged(record, block, &registry).is_failure(), "{name}");
        // The transactions both sides collated, left out of the reference's collation.
        let ours = heartwood(record, &registry);
        let both = ours["collated"].as_array().unwrap();
        let mut collation = record["output"].clone();
        let list = collation["collated"].as_array_mut().unwrap();
        let before = list.len();
        list.retain(|id| !both.contains(id));
        assert!(
            list.len() < before,
            "{name}: no transaction both sides collated"
        );
        assert!(judged(record, collation, &registry).is_failure(), "{name}");
        // Without the reference's session nothing shows why the pools differ.
        let mut bare = record["output"].clone();
        bare["notes"].as_object_mut().unwrap().remove("session");
        assert!(judged(record, bare, &registry).is_failure(), "{name}");
        // With Heartwood's own session in its place the pools agree: the forger differs.
        let mut agreeing = record["output"].clone();
        agreeing["notes"]["session"] = ours["notes"]["session"].clone();
        assert!(judged(record, agreeing, &registry).is_failure(), "{name}");
        // Heartwood's collation without the transactions only its pool held, with room in the
        // block: they are left out for no reason.
        let theirs = record["output"]["collated"].as_array().unwrap();
        let mut omitted = ours.clone();
        let list = omitted["collated"].as_array_mut().unwrap();
        list.retain(|id| theirs.contains(id));
        if list.len() < ours["collated"].as_array().unwrap().len() {
            let case = Case {
                op: record["op"].as_str().unwrap(),
                network: record["network"].as_str().unwrap(),
                height: record["height"].as_u64().unwrap(),
                input: &record["input"],
            };
            let outcome =
                collation_difference(&case, &record["output"], &omitted, &registry, Vec::new());
            assert!(outcome.is_failure(), "{name}: {outcome:?}");
        }
        // Heartwood's own block with its last byte changed, a byte added or its id changed.
        let case = Case {
            op: record["op"].as_str().unwrap(),
            network: record["network"].as_str().unwrap(),
            height: record["height"].as_u64().unwrap(),
            input: &record["input"],
        };
        let mut corrupt = ours.clone();
        let hex = corrupt["hex"].as_str().unwrap().to_owned();
        let last = if hex.ends_with('0') { "1" } else { "0" };
        corrupt["hex"] = Value::from(format!("{}{last}", &hex[..hex.len() - 1]));
        let outcome =
            collation_difference(&case, &record["output"], &corrupt, &registry, Vec::new());
        assert!(outcome.is_failure(), "{name}: {outcome:?}");
        let mut longer = ours.clone();
        longer["hex"] = Value::from(format!("{hex}00"));
        let outcome =
            collation_difference(&case, &record["output"], &longer, &registry, Vec::new());
        assert!(outcome.is_failure(), "{name}: {outcome:?}");
        let mut renamed = ours.clone();
        renamed["id"] = record["output"]["id"].clone();
        let outcome =
            collation_difference(&case, &record["output"], &renamed, &registry, Vec::new());
        assert!(outcome.is_failure(), "{name}: {outcome:?}");
    }
}

#[test]
fn a_pool_order_against_a_senders_nonce_order_fails() {
    // Heartwood's pool before the collation with a sender's two transactions swapped in its
    // priority order, and its collation in that order: the block the reference cannot witness
    // (the later nonce first) must not pass as the pools' difference.
    let registry = registry();
    let mut checked = 0;
    for record in &cases() {
        let name = &record["name"];
        let ours = heartwood(record, &registry);
        let last = record["input"]["steps"].as_array().unwrap().len() - 1;
        let pool = &ours["notes"]["session"]["results"][last]["contents"];
        let Some(pair) = (pool["bySender"].as_array().unwrap().iter())
            .map(|sender| sender[1].as_array().unwrap().clone())
            .find(|list| list.len() >= 2)
        else {
            continue;
        };
        let swap = |list: &mut Vec<Value>| {
            let (Some(a), Some(b)) = (
                list.iter().position(|id| *id == pair[0]),
                list.iter().position(|id| *id == pair[1]),
            ) else {
                return;
            };
            list.swap(a, b);
        };
        let mut inverted = ours.clone();
        swap(
            inverted["notes"]["session"]["results"][last]["contents"]["highest"]
                .as_array_mut()
                .unwrap(),
        );
        swap(inverted["collated"].as_array_mut().unwrap());
        let case = Case {
            op: record["op"].as_str().unwrap(),
            network: record["network"].as_str().unwrap(),
            height: record["height"].as_u64().unwrap(),
            input: &record["input"],
        };
        let outcome =
            collation_difference(&case, &record["output"], &inverted, &registry, Vec::new());
        assert!(
            format!("{outcome:?}").contains("in the priority order"),
            "{name}: {outcome:?}"
        );
        checked += 1;
    }
    assert!(checked > 0, "no case holds a sender with two transactions");
}

#[test]
fn a_tie_order_no_eviction_or_held_difference_explains_fails() {
    // In the cases where the reference evicted a transfer and admitted it again within one batch,
    // the two pools held the same transactions after every step: only the eviction note says
    // that the transfer's sender came back last among equal fees. Without the evicted ids in the
    // note, the swapped order of the two collations is no longer explained.
    let registry = registry();
    let mut checked = 0;
    for record in cases().iter().filter(|record| {
        (record["name"].as_str()).is_some_and(|name| name.contains("in the same batch"))
    }) {
        let name = &record["name"];
        assert_eq!(
            judged(record, record["output"].clone(), &registry),
            Outcome::Documented(Documented::FullPoolEviction.reason().into()),
            "{name}"
        );
        let mut silent = record["output"].clone();
        for step in silent["notes"]["session"]["results"]
            .as_array_mut()
            .unwrap()
        {
            let evictions = &mut step["result"]["notes"]["evictions"];
            for eviction in evictions.as_array_mut().into_iter().flatten() {
                eviction["evicted"] = Value::Array(Vec::new());
            }
        }
        let outcome = judged(record, silent, &registry);
        assert!(
            format!("{outcome:?}").contains("the pools' difference does not concern"),
            "{name}: {outcome:?}"
        );
        checked += 1;
    }
    assert_eq!(checked, 2);
}

#[test]
fn a_concerned_transaction_placed_across_a_higher_fee_fails() {
    // The reference evicted a 50M transfer and admitted it again last among equal fees, after a
    // 60M transfer: its collation is the 60M one, the other sender's 50M one, then the evicted
    // one. Heartwood's pool, priority order, collation and block made with the evicted transfer
    // first, ahead of the 60M one, is no tie the senders' order decides: it must fail, although
    // the evicted transfer's sender is one the pools' difference concerns.
    let devnet = devnet();
    let registry = subject::heartwood(Some(devnet.clone()));
    let record = (cases().into_iter())
        .find(|record| {
            (record["name"].as_str()).is_some_and(|name| name.contains("ahead of the 60M one"))
        })
        .expect("the case is committed");
    assert_eq!(
        judged(&record, record["output"].clone(), &registry),
        Outcome::Documented(Documented::FullPoolEviction.reason().into())
    );
    let ours = heartwood(&record, &registry);
    let evicted = (record["output"]["notes"]["session"]["results"]
        .as_array()
        .unwrap()
        .iter())
    .flat_map(|step| {
        step["result"]["notes"]["evictions"]
            .as_array()
            .cloned()
            .unwrap_or_default()
    })
    .flat_map(|eviction| eviction["evicted"].as_array().cloned().unwrap_or_default())
    .next()
    .expect("an eviction is noted");
    let height = record["height"].as_u64().unwrap();
    let fee = |id: &Value| {
        let text = ours["notes"]["transactions"][id.as_str().unwrap()]
            .as_str()
            .unwrap();
        let bytes = hex::decode(text).unwrap();
        let params = devnet.milestones.at(u32::try_from(height).unwrap());
        Transaction::decode_trusted(&bytes, params).unwrap().fee()
    };
    let mut order: Vec<Value> = vec![evicted.clone()];
    order.extend(
        (ours["collated"].as_array().unwrap().iter())
            .filter(|id| **id != evicted)
            .cloned(),
    );
    assert!(
        order[1..].iter().any(|id| fee(id) > fee(&evicted)),
        "a higher fee follows the evicted transfer"
    );
    let steps = record["input"]["steps"].as_array().unwrap().len();
    let mut moved = ours.clone();
    moved["notes"]["session"]["results"][steps - 1]["contents"]["highest"] = json!(order);
    moved["notes"]["session"]["results"][steps]["result"]["ids"] = json!(order);
    moved["notes"]["session"]["results"][steps]["contents"]["highest"] = json!(order);
    moved["collated"] = json!(order);
    let transactions: Vec<Value> = (order.iter())
        .map(|id| ours["notes"]["transactions"][id.as_str().unwrap()].clone())
        .collect();
    let input = &record["input"];
    let forge = json!({
        "privateKey": input["privateKey"], "aux": input["aux"],
        "previousBlock": {"id": input["previousBlock"]["id"], "height": ours["notes"]["tip"]},
        "timestamp": input["timestamp"], "reward": input["reward"], "transactions": transactions,
    });
    let request = Case {
        op: "forger.forge",
        network: "devnet",
        height,
        input: &forge,
    };
    match registry.get("forger.forge").unwrap().run(&request) {
        Ok(Verdict::Output(block)) => {
            moved["hex"] = block["hex"].clone();
            moved["id"] = block["id"].clone();
        }
        other => panic!("forger.forge: {other:?}"),
    }
    let case = Case {
        op: record["op"].as_str().unwrap(),
        network: record["network"].as_str().unwrap(),
        height,
        input,
    };
    let outcome = collation_difference(&case, &record["output"], &moved, &registry, Vec::new());
    assert!(
        outcome.is_failure()
            && format!("{outcome:?}").contains("the next transaction of another sender"),
        "{outcome:?}"
    );
}
