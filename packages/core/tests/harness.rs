//! The harness's seams of `relay run` (feature `difftest`): the digest and block files, written
//! through the node thread's commit observer, equal the reference judge's lines for the same steps;
//! and the seed switch lets exactly its one address through the address rules.

#![cfg(feature = "difftest")]

mod common;

use std::net::IpAddr;
use std::sync::Arc;

use common::{Setup, corpus, run};
use heartwood_blockchain::events::CommitObserver;
use heartwood_core::app::restored;
use heartwood_core::config::RelayConfig;
use heartwood_core::harness::{Recorder, SeedRules};
use heartwood_crypto::Block;
use heartwood_crypto::utils::hex;
use heartwood_p2p::seams::PeerRules;
use heartwood_p2p::utils::rules::Rules;
use heartwood_transactions::Handlers;
use serde_json::Value;

fn jsonl(text: &str) -> Vec<Value> {
    text.lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[test]
fn the_recorder_writes_the_judges_lines_for_the_judges_steps() {
    let setup = Setup::new("recorder", "");
    let config = RelayConfig::load(
        &run("--network testnet"),
        &setup.process(&[]),
        &setup.home(),
        &setup.root,
    )
    .unwrap();
    let (_, restored) = restored(&config).unwrap();
    let milestones = Arc::new(config.crypto.milestones.clone());
    let mut state = restored.state;
    let blocks_text = std::fs::read_to_string(corpus().join("blocks.jsonl")).unwrap();
    let records = jsonl(&blocks_text);
    let block = |height: u64| {
        let record = &records[usize::try_from(height - 2).unwrap()];
        Block::decode_trusted(
            &hex::decode(record["hex"].as_str().unwrap()).unwrap(),
            &milestones,
        )
        .unwrap()
    };
    let (digests, blocks) = (
        setup.root.join("digests.jsonl"),
        setup.root.join("blocks.jsonl"),
    );
    let mut recorder =
        Recorder::open(Arc::clone(&milestones), Some(&digests), Some(&blocks), true).unwrap();
    let judge = jsonl(&std::fs::read_to_string(corpus().join("ts-digests.jsonl")).unwrap());
    let mut applied = 0;
    for line in &judge {
        let height = line["h"].as_u64().unwrap();
        match line["step"].as_str().unwrap() {
            "genesis" => recorder.restarted(&state),
            "apply" | "reapply" => {
                let block = block(height);
                state.apply_block(&block, &milestones, &Handlers).unwrap();
                recorder.applied(&state, &block);
                applied += 1;
            }
            "revert" => {
                state.revert_block(&milestones, &Handlers).unwrap();
                recorder.reverted(&state);
            }
            other => panic!("{other}"),
        }
    }
    let written = jsonl(&std::fs::read_to_string(&digests).unwrap());
    assert_eq!(written.len(), judge.len());
    for (ours, theirs) in written.iter().zip(&judge) {
        assert_eq!(ours, theirs);
    }
    // One line per applied block, as the corpus writes its blocks.
    let written = jsonl(&std::fs::read_to_string(&blocks).unwrap());
    assert_eq!(written.len(), applied);
    let last = written.last().unwrap();
    assert_eq!(last, records.last().unwrap());
    // A restart starts its running digest again, as its own first line.
    let mut again = Recorder::open(milestones, Some(&digests), None, false).unwrap();
    again.restarted(&state);
    let lines = jsonl(&std::fs::read_to_string(&digests).unwrap());
    let restart = lines.last().unwrap();
    assert_eq!(
        (restart["step"].as_str(), restart["h"].as_u64()),
        (Some("restart"), Some(265))
    );
    assert_eq!(restart["sd"], judge.last().unwrap()["sd"]);
}

#[test]
fn the_seed_alone_passes_the_address_rules() {
    let seed: IpAddr = "127.0.0.1".parse().unwrap();
    let rules = SeedRules::new(Rules::new(100, 25), Some(seed));
    let valid = |ip: &str| rules.is_valid_peer(&mut ip.to_owned(), false, &[seed]);
    assert!(valid("127.0.0.1"));
    assert!(!valid("127.0.0.2"));
    assert!(!valid("10.0.0.1"));
    assert!(
        valid("192.0.2.1")
            == Rules::new(100, 25).is_valid_peer(&mut "192.0.2.1".to_owned(), false, &[])
    );
    let unseeded = SeedRules::new(Rules::new(100, 25), None);
    assert!(!unseeded.is_valid_peer(&mut "127.0.0.1".to_owned(), false, &[]));
}
