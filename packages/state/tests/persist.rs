//! The state's records on the committed one-node chain, in one pass over the chain and its revert
//! plan: at every step the changed wallets, the state record and the last undo record read back
//! equal as JSON text, and `changed_since` lists exactly the wallets the step changed; at every
//! height, a state restored from all its records equals the state it was written from, and the
//! replay goes on from it with every digest line equal to the reference's. Then `State::restore`
//! refuses each kind of inconsistent record.

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

use heartwood_crypto::blocks::factory;
use heartwood_crypto::identities::{Address, PublicKey};
use heartwood_crypto::{Block, Milestones, Network};
use heartwood_state::digest::{self, DigestLine, Step};
use heartwood_state::persist::{
    RestoreError, STATE_FORMAT, StateRecord, TxUndoRecord, UndoRecord, WalletRecord,
};
use heartwood_state::round_state::MissedBlock;
use heartwood_state::undo::BlockUndo;
use heartwood_state::{State, Wallet, WalletRepository};
use heartwood_transactions::Handlers;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

/// The committed one-node chain: its crypto files, blocks, revert plan and the oracle's digest
/// lines.
struct Chain {
    network: u8,
    milestones: Milestones,
    genesis: Block,
    blocks: Vec<Block>,
    plan: Vec<(u32, u32)>,
    digests: Vec<Value>,
}

fn corpus() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tools/difftest/corpus/txgen-one-node")
}

fn read(name: &str) -> String {
    fs::read_to_string(corpus().join(name)).unwrap()
}

fn lines(name: &str) -> Vec<Value> {
    read(name)
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

impl Chain {
    fn load() -> Chain {
        let network = Network::load(&read("crypto/network.json")).unwrap();
        let milestones = Milestones::load(&read("crypto/milestones.json"), &network).unwrap();
        let json: Value = serde_json::from_str(&read("crypto/genesisBlock.json")).unwrap();
        let genesis = factory::from_json(&json, &milestones).unwrap();
        let blocks = lines("blocks.jsonl")
            .iter()
            .map(|line| {
                let bytes = hex::decode(line["hex"].as_str().unwrap()).unwrap();
                let block = Block::receive(&bytes, &milestones).unwrap();
                assert_eq!(block.id().to_hex(), line["id"].as_str().unwrap());
                block
            })
            .collect();
        let plan: Value = serde_json::from_str(&read("revert-plan.json")).unwrap();
        let plan = plan["steps"]
            .as_array()
            .unwrap()
            .iter()
            .map(|step| {
                let number = |key: &str| u32::try_from(step[key].as_u64().unwrap()).unwrap();
                (number("after"), number("blocks"))
            })
            .collect();
        Chain {
            network: network.pub_key_hash,
            milestones,
            genesis,
            blocks,
            plan,
            digests: lines("ts-digests.jsonl"),
        }
    }

    fn block(&self, height: u32) -> &Block {
        &self.blocks[height as usize - 2]
    }

    /// The steps of the chain after the genesis step, with the revert plan: a height to apply
    /// (`Some`), or a revert (`None`); the flag marks a first apply.
    fn steps(&self) -> Vec<(Option<u32>, bool)> {
        let mut steps = Vec::new();
        for block in &self.blocks {
            let height = block.height();
            steps.push((Some(height), true));
            for &(_, count) in self.plan.iter().filter(|(after, _)| *after == height) {
                steps.extend(std::iter::repeat_n((None, false), count as usize));
                steps.extend((height - count + 1..=height).map(|h| (Some(h), false)));
            }
        }
        steps
    }
}

/// A replay of the chain's steps that writes the digest lines as the L2 replay does.
struct Replay<'c> {
    chain: &'c Chain,
    state: State,
    previous: [u8; 32],
    lines: Vec<DigestLine>,
}

impl<'c> Replay<'c> {
    fn new(chain: &'c Chain) -> Replay<'c> {
        let state = State::from_genesis(&chain.genesis, &chain.milestones, &Handlers).unwrap();
        let mut replay = Replay {
            chain,
            state,
            previous: [0; 32],
            lines: Vec::new(),
        };
        replay.line(Step::Genesis, &[], None);
        replay
    }

    fn line(&mut self, step: Step, missed: &[MissedBlock], reward: Option<u64>) {
        let milestones = &self.chain.milestones;
        let line = digest::height_digest(
            &self.state,
            milestones,
            step,
            missed,
            reward,
            &self.previous,
        );
        self.previous = line.chain_digest;
        self.lines.push(line);
    }

    /// Run one step of [`Chain::steps`].
    fn step(&mut self, (height, first): (Option<u32>, bool)) {
        let milestones = &self.chain.milestones;
        match height {
            None => {
                self.state.revert_block(milestones, &Handlers).unwrap();
                self.line(Step::Revert, &[], None);
            }
            Some(height) => {
                let block = self.chain.block(height);
                let generator = block.header().generator_public_key;
                let reward = self
                    .state
                    .reward_for_block_in_round(height, &generator, milestones)
                    .unwrap()
                    .reward;
                let applied = self
                    .state
                    .apply_block(block, milestones, &Handlers)
                    .unwrap();
                let step = if first { Step::Apply } else { Step::Reapply };
                self.line(step, &applied.missed, Some(reward));
            }
        }
    }

    /// The last line, as the oracle's line of the same step is written.
    fn last_json(&self) -> Value {
        serde_json::from_str(&self.lines.last().unwrap().to_json()).unwrap()
    }
}

/// The records of a state: its wallets, its state record and its undo records.
type Records = (Vec<WalletRecord>, StateRecord, Vec<UndoRecord>);

/// `value` written as JSON text and read back.
fn round_trip<T: Serialize + DeserializeOwned>(value: &T) -> T {
    serde_json::from_str(&serde_json::to_string(value).unwrap()).unwrap()
}

/// The records of `state` as a store keeps them, read back as JSON text.
fn records(state: &State) -> Records {
    let wallets = state
        .wallets()
        .iter()
        .map(|w| round_trip(&w.to_record()))
        .collect();
    let journal = state
        .journal()
        .iter()
        .map(|u| round_trip(&UndoRecord::from_undo(u)));
    (
        wallets,
        round_trip(&state.state_record()),
        journal.collect(),
    )
}

/// The state that `records` describe, over the chain's blocks.
fn restore(chain: &Chain, (wallets, record, journal): Records) -> Result<State, RestoreError> {
    let wallets = wallets
        .into_iter()
        .map(|record| Wallet::from_record(record, chain.network))
        .collect::<Result<Vec<Wallet>, _>>()?;
    let wallets = WalletRepository::from_wallets(chain.network, wallets)?;
    let journal = journal
        .into_iter()
        .map(|undo| {
            let block = chain.block(undo.h).clone();
            undo.into_undo(block, chain.network)
        })
        .collect::<Result<Vec<BlockUndo>, _>>()?;
    let last = journal
        .last()
        .map_or(&chain.genesis, |undo| &undo.block)
        .clone();
    State::restore(
        chain.genesis.clone(),
        last,
        wallets,
        record,
        journal,
        &chain.milestones,
    )
}

/// `changed_since` lists exactly the wallets of `wallets` whose value differs from `earlier`'s
/// (each step writes a wallet only when its value changes), and nothing against itself.
fn check_changed_since(wallets: &WalletRepository, earlier: &WalletRepository) {
    let listed: BTreeSet<&Address> = wallets
        .changed_since(earlier)
        .map(Wallet::address)
        .collect();
    for wallet in wallets.iter() {
        let changed = earlier.find_by_address(wallet.address()) != Some(wallet);
        assert_eq!(
            listed.contains(wallet.address()),
            changed,
            "{}",
            wallet.address()
        );
    }
    assert!(wallets.changed_since(wallets).next().is_none());
}

#[test]
fn every_record_reads_back_and_a_restart_at_every_height_goes_on_as_the_reference() {
    let chain = Chain::load();
    let mut replay = Replay::new(&chain);
    let steps = chain.steps();
    assert_eq!(steps.len() + 1, chain.digests.len());
    for (index, step) in steps.into_iter().enumerate() {
        let earlier = replay.state.wallets().clone();
        replay.step(step);
        let line = format!("line {}", index + 2);
        assert_eq!(replay.last_json(), chain.digests[index + 1], "{line}");
        let state = &replay.state;
        check_changed_since(state.wallets(), &earlier);
        for wallet in state.wallets().changed_since(&earlier) {
            let record = round_trip(&wallet.to_record());
            assert_eq!(&Wallet::from_record(record, chain.network).unwrap(), wallet);
        }
        let record = round_trip(&state.state_record());
        assert_eq!(record, state.state_record());
        assert_eq!(record.format, STATE_FORMAT);
        if let Some(undo) = state.journal().last() {
            let record = round_trip(&UndoRecord::from_undo(undo));
            assert_eq!(
                &record.into_undo(undo.block.clone(), chain.network).unwrap(),
                undo
            );
        }
        // At each height's first apply, restart from the records: the restored state is the
        // state, its restart line has the forward line's sections, and the replay goes on with
        // it, so every later line is a line after a restart.
        if step.1 {
            let restored = restore(&chain, records(state)).unwrap();
            assert_eq!(&restored, state, "{line}");
            let sections = digest::records(&restored, &chain.milestones, &[]).hashes();
            let forward = &replay.lines.last().unwrap().sections;
            let key = |s: &digest::SectionHashes| (s.block, s.supply, s.round, s.wallets);
            assert_eq!(key(&sections), key(forward), "{line}");
            replay.state = restored;
        }
    }
}

/// The records of the chain's state at height 60, where every kind of record has a value: a
/// round started at 54, delegates with votes, resignations and a second key.
fn records_at_60(chain: &Chain) -> Records {
    let mut replay = Replay::new(chain);
    for height in 2..=60 {
        replay.step((Some(height), true));
    }
    let records = records(&replay.state);
    restore(chain, records.clone()).unwrap();
    records
}

/// The wallet record of the first delegate that some wallet votes for.
fn voted_delegate(wallets: &[WalletRecord]) -> usize {
    let name = wallets.iter().find_map(|w| w.vs.first()).unwrap().0.clone();
    let is_named = |w: &WalletRecord| w.d.as_ref().is_some_and(|d| d.u == name);
    wallets.iter().position(is_named).unwrap()
}

#[test]
fn inconsistent_records_are_refused() {
    let chain = Chain::load();
    let records = records_at_60(&chain);
    let refused = |edit: &dyn Fn(&mut Records)| {
        let mut records = records.clone();
        edit(&mut records);
        restore(&chain, records).unwrap_err()
    };
    let record_of = |error: RestoreError| match error {
        RestoreError::Record { what, .. } => what,
        other => panic!("{other:?}"),
    };

    assert_eq!(refused(&|(_, s, _)| s.format = 2), RestoreError::Format(2));
    assert!(matches!(
        refused(&|(_, s, _)| s.height = 59),
        RestoreError::LastBlock {
            record_height: 59,
            block_height: 60,
            ..
        }
    ));
    assert!(matches!(
        refused(&|(_, s, _)| s.block_id = chain.block(59).id().to_hex()),
        RestoreError::LastBlock { .. }
    ));
    assert_eq!(
        refused(&|(_, _, j)| {
            j.remove(10);
        }),
        RestoreError::Journal { expected: 12 }
    );
    assert!(matches!(
        refused(&|(_, _, j)| {
            j.pop();
        }),
        RestoreError::LastBlock { .. }
    ));
    assert_eq!(record_of(refused(&|(_, _, j)| j[3].h = 6)), "undo");
    assert_eq!(
        record_of(refused(&|(_, _, j)| j[3].t.push(TxUndoRecord::None))),
        "undo"
    );

    let delegate = voted_delegate(&records.0);
    let username = records.0[delegate].d.as_ref().unwrap().u.clone();
    let vote_balance = RestoreError::VoteBalance { username };
    let raise = |w: &mut Vec<WalletRecord>| {
        let d = w[delegate].d.as_mut().unwrap();
        d.vb = (d.vb.parse::<u64>().unwrap() + 1).to_string();
    };
    assert_eq!(refused(&|(w, _, _)| raise(w)), vote_balance);
    assert_eq!(
        refused(&|(w, _, _)| w[delegate].d.as_mut().unwrap().vc += 1),
        vote_balance
    );
    assert!(matches!(
        refused(
            &|(w, _, _)| w.iter_mut().find(|w| !w.vs.is_empty()).unwrap().vs[0].0 = "nobody".into()
        ),
        RestoreError::VoteBalance { .. }
    ));
    assert!(matches!(
        refused(&|(w, _, _)| {
            let wallet = w
                .iter_mut()
                .find(|w| w.bal != "0" && w.vs.is_empty())
                .unwrap();
            wallet.bal = (wallet.bal.parse::<u64>().unwrap() - 1).to_string();
        }),
        RestoreError::Supply(_)
    ));
    assert_eq!(
        refused(&|(_, s, _)| s.round.f.swap(0, 1)),
        RestoreError::Forging
    );
    assert_eq!(
        refused(&|(_, _, j)| j[51].r.as_mut().unwrap().p.f.swap(0, 1)),
        RestoreError::Forging
    );
    assert_eq!(record_of(refused(&|(_, s, _)| s.round.rh = 1)), "round");
    assert_eq!(
        record_of(refused(&|(_, s, _)| s.totals.rewards = "1x".into())),
        "totals"
    );

    // Keys and names are held once.
    assert!(matches!(
        refused(&|(w, _, _)| {
            let named = w.iter().find(|w| w.d.is_some()).unwrap().d.clone();
            w.iter_mut().find(|w| w.d.is_none()).unwrap().d = named;
        }),
        RestoreError::DuplicateUsername(_)
    ));
    assert!(matches!(
        refused(&|(w, _, _)| w.push(w[0].clone())),
        RestoreError::DuplicatePublicKey(_) | RestoreError::Record { .. }
    ));
    // The generator point, compressed and in full: two addresses, one key.
    let compressed = PublicKey::from_hex(G_COMPRESSED).unwrap();
    let full = PublicKey::from_hex(G_FULL).unwrap();
    let wallet_of = |key: &PublicKey| WalletRecord {
        a: Address::from_public_key(key, chain.network).to_base58(),
        pk: Some(key.to_hex()),
        bal: "0".into(),
        n: "0".into(),
        spk: None,
        d: None,
        v: Vec::new(),
        vs: Vec::new(),
    };
    assert_eq!(
        refused(&|(w, _, _)| w.extend([wallet_of(&compressed), wallet_of(&full)])),
        RestoreError::DuplicatePublicKey(full.to_hex())
    );

    // Values that do not read.
    for edit in [
        &(|w: &mut WalletRecord| w.bal = "-1".into()) as &dyn Fn(&mut WalletRecord),
        &|w| w.bal = "18446744073709551616".into(),
        &|w| w.n = "+1".into(),
        &|w| w.a = "not an address".into(),
        &|w| w.pk = Some(full.to_hex()),
        &|w| w.spk = Some("02".into()),
        &|w| w.d.as_mut().unwrap().rs = Some(2),
        &|w| w.d.as_mut().unwrap().lb = Some("zz".into()),
    ] {
        let delegate = records.0.iter().position(|w| w.d.is_some()).unwrap();
        assert_eq!(
            record_of(refused(&|(w, _, _)| edit(&mut w[delegate]))),
            "wallet"
        );
    }
}

/// The secp256k1 generator point, compressed and in full.
const G_COMPRESSED: &str = "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798";
const G_FULL: &str = "0479be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798\
                      483ada7726a3c4655da4fbfc0e1108a8fd17b448a68554199c47d08ffb10d4b8";

#[test]
fn records_have_the_digest_keys_and_refuse_unknown_fields() {
    let chain = Chain::load();
    let (wallets, state, journal) = records_at_60(&chain);
    let text = serde_json::to_string(&wallets[0]).unwrap();
    assert!(text.starts_with("{\"a\":"), "{text}");
    let unknown = text.replacen("{", "{\"x\":1,", 1);
    assert!(serde_json::from_str::<WalletRecord>(&unknown).is_err());
    let text = serde_json::to_string(&state).unwrap();
    assert!(
        text.starts_with(&format!("{{\"format\":{STATE_FORMAT},\"height\":60,")),
        "{text}"
    );
    let text = serde_json::to_string(&journal[51]).unwrap();
    assert!(text.contains("\"r\":{\"p\":{\"round\":1,"), "{text}");
}
