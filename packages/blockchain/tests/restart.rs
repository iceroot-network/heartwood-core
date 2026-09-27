//! Restart equality on the committed one-node chain, through the store: the chain and its revert
//! plan are committed one step per chunk, and the node restarts at every height, from the state
//! tables and by a replay from the genesis block. Each restart gives the state that never
//! restarted (the restart line has its sections), every later digest line equals the oracle's,
//! and the store's rows equal the reference node's. Also: chunks of many blocks, a revert across
//! a round end right after a restart, the first start, and the fallback to a replay when the
//! state tables cannot be used.

mod common;

use common::{Chain, Node, assert_same_state, hex_sections, lines};
use heartwood_blockchain::{ReplayReason, RestoreMode, RestorePath};
use heartwood_database::{ChainMeta, SCHEMA_VERSION, StateRow};
use heartwood_state::State;
use heartwood_state::digest;
use heartwood_state::persist::{RestoreError, STATE_FORMAT};
use heartwood_transactions::Handlers;
use serde_json::Value;

/// A state that never restarts, stepped as the chain's steps say.
fn step_forward(chain: &Chain, state: &mut State, (height, _): common::ChainStep) {
    let milestones = chain.milestones();
    match height {
        None => drop(state.revert_block(milestones, &Handlers).unwrap()),
        Some(height) => drop(
            state
                .apply_block(chain.block(height), milestones, &Handlers)
                .unwrap(),
        ),
    }
}

/// The restart line's sections: the state's sections with no missed slot.
fn restart_sections(chain: &Chain, state: &State) -> [String; 5] {
    hex_sections(&digest::records(state, chain.milestones(), &[]).hashes())
}

/// Heights at which the node also restarts by a replay from the genesis block: every height in
/// a release build; in a debug build, the heights around every round end and every seventh.
fn replays_at(height: u32) -> bool {
    !cfg!(debug_assertions) || height.is_multiple_of(7) || matches!(height % 53, 0..=2 | 52)
}

#[test]
fn a_restart_at_every_height_gives_the_same_state_and_the_reference_lines_after_it() {
    let chain = Chain::load();
    let mut node = Node::first_start(&chain, "every-height");
    let mut forward = State::from_genesis(&chain.genesis(), chain.milestones(), &Handlers).unwrap();
    let steps = chain.steps();
    let mut replays = 0;
    let mut index = 0;
    while index < steps.len() {
        // A first apply is a chunk of its own; the reverts and re-applies after it are one chunk.
        let first = steps[index];
        let (Some(height), true) = first else {
            panic!("step {index}")
        };
        let end = (index + 1..steps.len())
            .find(|&i| steps[i].1)
            .unwrap_or(steps.len());
        node.step(first);
        step_forward(&chain, &mut forward, first);
        let line = index + 2;
        assert_eq!(node.last_json(), chain.digests[index + 1], "line {line}");
        let mut modes = vec![RestoreMode::Auto];
        if replays_at(height) {
            modes.push(RestoreMode::Replay);
            replays += 1;
        }
        for mode in modes {
            let path = node.restart(mode);
            let expected = match mode {
                RestoreMode::Auto => RestorePath::Table,
                RestoreMode::Replay => RestorePath::Replay(ReplayReason::Requested),
            };
            assert_eq!(path, expected, "line {line}");
            let context = format!("restart at {height} ({mode:?}), line {line}");
            assert_same_state(&node.state, &forward, &context);
            let [b, s, r, _, w] = restart_sections(&chain, &node.state);
            let [fb, fs, fr, _, fw] = chain.sections(index + 1);
            assert_eq!([b, s, r, w], [fb, fs, fr, fw], "{context}");
        }
        if end > index + 1 {
            node.steps(&steps[index + 1..end]);
            for (offset, &step) in steps[index + 1..end].iter().enumerate() {
                step_forward(&chain, &mut forward, step);
                let at = index + 1 + offset;
                let line = &node.lines[at];
                let json: Value = serde_json::from_str(&line.to_json()).unwrap();
                assert_eq!(json, chain.digests[at], "line {}", at + 1);
            }
        }
        index = end;
    }
    assert_eq!(node.lines.len(), chain.digests.len());
    assert!(replays >= 40, "{replays} replays");
    check_anchors(&mut node);
}

/// The store's rows equal the reference node's: the round rows (per round as sets, since the
/// corpus's export orders them by balance as text; the store gives them in `getRound` order, by
/// balance from the highest, then by key), the missed slots, and each block's burned fee,
/// donations and generator name.
fn check_anchors(node: &mut Node<'_>) {
    let tip = node.chain.tip();
    let database = node.store().database();
    let mut rounds: Vec<Value> = Vec::new();
    for round in 1..=6 {
        let rows = database.round(round).unwrap();
        let order: Vec<(u64, &str)> = (rows.iter()).map(|r| (r.balance, &*r.public_key)).collect();
        let mut expected = order.clone();
        expected.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(b.1)));
        assert_eq!(order, expected, "round {round} is in getRound order");
        for row in rows {
            rounds.push(serde_json::json!({
                "round": row.round,
                "publicKey": row.public_key,
                "balance": row.balance.to_string(),
            }));
        }
    }
    let mut reference = lines("reference/rounds.jsonl");
    let key = |row: &Value| {
        (
            row["round"].as_u64(),
            row["publicKey"].as_str().map(str::to_owned),
        )
    };
    rounds.sort_by_key(key);
    reference.sort_by_key(key);
    assert_eq!(rounds, reference);
    let missed: Vec<Value> = (database.missed_blocks().unwrap().iter())
        .map(|row| {
            let (timestamp, height, username) = (row.timestamp, row.height, &row.username);
            serde_json::json!({"timestamp": timestamp, "height": height, "username": username})
        })
        .collect();
    assert_eq!(missed, lines("reference/missed-blocks.jsonl"));
    let meta: Vec<Value> = (database.blocks(2, tip).unwrap().iter())
        .map(|block| {
            let donations: Value = serde_json::from_str(&block.row.donations).unwrap();
            serde_json::json!({
                "h": block.row.height,
                "username": block.row.username,
                "burnedFee": block.row.burned_fee.to_string(),
                "donations": donations,
            })
        })
        .collect();
    assert_eq!(meta, lines("reference/meta.jsonl"));
}

#[test]
fn a_chunk_of_many_blocks_is_committed_at_once() {
    let chain = Chain::load();
    let milestones = chain.milestones();
    let mut node = Node::first_start(&chain, "chunks");
    let mut forward = node.state.clone();
    for heights in chain.blocks.chunks(100) {
        let mut state = node.state.clone();
        let mut chunk = node.store().chunk().unwrap();
        for block in heights {
            let applied = state.apply_block(block, milestones, &Handlers).unwrap();
            chunk.applied(&state, block, &applied, milestones).unwrap();
            forward.apply_block(block, milestones, &Handlers).unwrap();
        }
        chunk.commit().unwrap();
        assert_eq!(node.restart(RestoreMode::Auto), RestorePath::Table);
        assert_same_state(&node.state, &forward, "after a chunk");
    }
    // A chunk dropped before its commit writes nothing.
    let mut state = node.state.clone();
    let reverted = state.revert_block(milestones, &Handlers).unwrap();
    let mut chunk = node.store().chunk().unwrap();
    chunk.reverted(&state, &reverted, milestones).unwrap();
    drop(chunk);
    assert_eq!(node.restart(RestoreMode::Auto), RestorePath::Table);
    assert_same_state(&node.state, &forward, "after a dropped chunk");
}

#[test]
fn a_revert_across_a_round_end_right_after_a_restart() {
    let chain = Chain::load();
    let milestones = chain.milestones();
    let mut node = Node::first_start(&chain, "revert-after-restart");
    for height in 2..=110 {
        node.step((Some(height), true));
    }
    let lines = node.lines.clone();
    for mode in [RestoreMode::Auto, RestoreMode::Replay] {
        node.restart(mode);
        // Back 54 blocks, across the end of round 2 at 106, then forward again.
        for height in (56..110).rev() {
            let (store, state) = node.parts();
            let reverted = state.revert_block(milestones, &Handlers).unwrap();
            let mut chunk = store.chunk().unwrap();
            chunk.reverted(state, &reverted, milestones).unwrap();
            chunk.commit().unwrap();
            let forward = &lines[height as usize - 1].sections;
            let sections = digest::records(&node.state, milestones, &[]).hashes();
            let key = |s: &digest::SectionHashes| (s.block, s.supply, s.round, s.wallets);
            assert_eq!(key(&sections), key(forward), "{mode:?}: revert to {height}");
        }
        assert_eq!(node.restart(RestoreMode::Auto), RestorePath::Table);
        for height in 57..=110 {
            node.step((Some(height), false));
            let line = node.last_json();
            let forward: Value =
                serde_json::from_str(&lines[height as usize - 1].to_json()).unwrap();
            for key in ["b", "s", "r", "m", "w", "sd", "rw"] {
                assert_eq!(line[key], forward[key], "{mode:?}: reapply {height}");
            }
        }
    }
}

#[test]
fn the_first_start_writes_the_chain_and_its_identity() {
    let chain = Chain::load();
    let mut node = Node::first_start(&chain, "first-start");
    let genesis = chain.genesis();
    let database = node.store().database();
    assert_eq!(
        database.meta().unwrap(),
        Some(ChainMeta {
            schema_version: SCHEMA_VERSION,
            nethash: chain.files.network.nethash.clone(),
            genesis_id: *genesis.id(),
            state_format: STATE_FORMAT,
        })
    );
    let stored = database.last_block().unwrap().unwrap();
    assert_eq!((stored.row.height, stored.row.id), (1, *genesis.id()));
    assert_eq!(stored.row.username, None);
    assert_eq!(stored.transactions.len(), genesis.transactions().len());
    assert_eq!(database.round(1).unwrap().len(), 53);
    let entries = database.milestone_entries().unwrap();
    assert_eq!(entries.iter().map(|e| e.height).collect::<Vec<_>>(), [1]);
    assert_eq!(database.undo_records().unwrap(), []);
    let first = node.state.clone();
    assert_eq!(node.restart(RestoreMode::Auto), RestorePath::Table);
    assert_same_state(&node.state, &first, "restart at 1");
    // The milestone entry at 2 is recorded when the tip reaches it.
    node.step((Some(2), true));
    let heights: Vec<u32> = (node.store().database().milestone_entries().unwrap().iter())
        .map(|e| e.height)
        .collect();
    assert_eq!(heights, [1, 2]);
}

#[test]
fn state_tables_that_cannot_be_used_are_replaced_by_a_replay() {
    let chain = Chain::load();
    let mut node = Node::first_start(&chain, "fallback");
    for height in 2..=60 {
        node.step((Some(height), true));
    }
    let state = node.state.clone();
    let row = node.store().database().state_row().unwrap().unwrap();
    let meta = node.store().database().meta().unwrap().unwrap();

    let put_state = |node: &mut Node<'_>, record: &str| {
        let mut chunk = node.store().database_mut().begin().unwrap();
        chunk
            .put_state(&StateRow {
                record: record.to_owned(),
                ..row.clone()
            })
            .unwrap();
        chunk.commit().unwrap();
    };
    put_state(&mut node, "{\"format\":1}");
    let path = node.restart(RestoreMode::Auto);
    let RestorePath::Replay(ReplayReason::Inconsistent(RestoreError::Record { what, .. })) = path
    else {
        panic!("{path:?}")
    };
    assert_eq!(what, "state");
    assert_same_state(&node.state, &state, "after a replay");
    // The replay wrote the state tables again.
    assert_eq!(node.restart(RestoreMode::Auto), RestorePath::Table);
    assert_same_state(&node.state, &state, "after the replay's tables");

    // A state record of another format, and tables of another format.
    let record = serde_json::to_string(&node.state.state_record()).unwrap();
    put_state(
        &mut node,
        &record.replacen("\"format\":1", "\"format\":2", 1),
    );
    let path = node.restart(RestoreMode::Auto);
    assert_eq!(
        path,
        RestorePath::Replay(ReplayReason::Inconsistent(RestoreError::Format(2)))
    );
    let mut chunk = node.store().database_mut().begin().unwrap();
    chunk
        .put_meta(&ChainMeta {
            state_format: 0,
            ..meta.clone()
        })
        .unwrap();
    chunk.commit().unwrap();
    let path = node.restart(RestoreMode::Auto);
    assert_eq!(path, RestorePath::Replay(ReplayReason::Format { found: 0 }));
    assert_eq!(node.store().database().meta().unwrap(), Some(meta));
    assert_eq!(node.restart(RestoreMode::Auto), RestorePath::Table);
    assert_same_state(&node.state, &state, "after the format's replay");

    // A state that is only asked to be replayed.
    let path = node.restart(RestoreMode::Replay);
    assert_eq!(path, RestorePath::Replay(ReplayReason::Requested));
    assert_same_state(&node.state, &state, "after a requested replay");
    assert_eq!(node.restart(RestoreMode::Auto), RestorePath::Table);
}
