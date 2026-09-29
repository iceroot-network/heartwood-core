//! `snapshot rollback` on the committed one-node chain (53 delegates, tip 265): the reference's
//! targets and refusals with their lines; the rows a rollback deletes, as the reference deletes
//! them (the targets and counts the reference's own command gave on this chain); the state tables
//! gone with them, so that the next start replays for want of a state; and that start writing the
//! rows of the round after the target again, as the reference's start does, so that the store
//! holds that round's rows and, after the re-sync to the tip, every round's rows as the reference
//! node's. A start from the state tables at a round end leaves the rows as they are.

mod common;

use std::collections::{BTreeMap, BTreeSet};

use common::{Chain, Node, hex_sections, lines};
use heartwood_blockchain::rollback::{self, RollbackPlan, RollbackRefusal, Rolled};
use heartwood_blockchain::{ReplayReason, RestoreMode, RestorePath};
use heartwood_kernel::log::Level;
use heartwood_state::digest;
use serde_json::Value;

const TIP: u32 = 265;

/// A node whose store holds the chain up to `tip`, applied in one chunk.
fn node_at<'c>(chain: &'c Chain, name: &str, tip: u32) -> Node<'c> {
    let mut node = Node::first_start(chain, name);
    let steps: Vec<_> = (2..=tip).map(|h| (Some(h), true)).collect();
    node.steps(&steps);
    node
}

/// The plan of `--height` and `--number` on the chain at `current`.
fn plan(
    chain: &Chain,
    height: Option<i64>,
    number: Option<i64>,
    current: u32,
) -> Result<RollbackPlan, RollbackRefusal> {
    rollback::plan(height, number, current, chain.milestones())
}

/// The lines of a refusal, as `level: text`.
fn texts(lines: &[(Level, String)]) -> Vec<String> {
    lines
        .iter()
        .map(|(level, text)| format!("{}: {text}", level.name()))
        .collect()
}

/// The reference node's round rows of the chain, by round, as sets of (public key, balance).
fn reference_rounds() -> BTreeMap<u64, BTreeSet<(String, String)>> {
    let mut out: BTreeMap<u64, BTreeSet<(String, String)>> = BTreeMap::new();
    for row in lines("reference/rounds.jsonl") {
        let key = row["publicKey"].as_str().unwrap().to_owned();
        let balance = match &row["balance"] {
            Value::String(text) => text.clone(),
            other => other.to_string(),
        };
        out.entry(row["round"].as_u64().unwrap())
            .or_default()
            .insert((key, balance));
    }
    out
}

/// The store's rows of `round`, as a set of (public key, balance).
fn stored_round(node: &mut Node<'_>, round: u64) -> BTreeSet<(String, String)> {
    let rows = node.store().database().round(round).unwrap();
    let set: BTreeSet<_> = rows
        .iter()
        .map(|row| (row.public_key.clone(), row.balance.to_string()))
        .collect();
    assert_eq!(set.len(), rows.len(), "round {round}: a key twice");
    set
}

/// The store's round rows per round, from round 1 to the first round without rows.
fn stored_rounds(node: &mut Node<'_>) -> BTreeMap<u64, BTreeSet<(String, String)>> {
    let mut out = BTreeMap::new();
    for round in 1.. {
        let rows = stored_round(node, round);
        if rows.is_empty() {
            break;
        }
        out.insert(round, rows);
    }
    out
}

/// A copy of the database files of the store in `from` (the file, its log and its index) in a
/// fresh directory named `name`.
fn copy_store(from: &std::path::Path, name: &str) -> std::path::PathBuf {
    let copy = common::scratch(name);
    std::fs::create_dir_all(copy.join("database")).unwrap();
    for entry in std::fs::read_dir(from.join("database")).unwrap() {
        let entry = entry.unwrap();
        std::fs::copy(entry.path(), copy.join("database").join(entry.file_name())).unwrap();
    }
    copy
}

/// The oracle's last forward line (apply or reapply) of each height.
fn forward_lines(chain: &Chain) -> BTreeMap<u64, &Value> {
    let forward = chain.digests.iter().filter(|line| line["step"] != "revert");
    forward
        .map(|line| (line["h"].as_u64().unwrap(), line))
        .collect()
}

#[test]
fn the_target_is_the_last_block_of_the_round_before_the_requested_heights_round() {
    let chain = Chain::load();
    let target = |height: Option<i64>, number: Option<i64>| {
        let plan = plan(&chain, height, number, TIP).unwrap();
        (plan.target, plan.round, plan.requested)
    };
    // The heights the reference's command was run with on this chain, and their rounds' edges.
    assert_eq!(target(Some(264), None), (212, 5, 264));
    assert_eq!(target(Some(213), None), (212, 5, 213));
    assert_eq!(target(Some(212), None), (159, 4, 212));
    assert_eq!(target(Some(180), None), (159, 4, 180));
    assert_eq!(target(Some(160), None), (159, 4, 160));
    assert_eq!(target(Some(133), None), (106, 3, 133));
    assert_eq!(target(Some(107), None), (106, 3, 107));
    assert_eq!(target(Some(106), None), (53, 2, 106));
    assert_eq!(target(Some(60), None), (53, 2, 60));
    assert_eq!(target(Some(54), None), (53, 2, 54));
    // --number counts back from the tip; --height wins over it; a 0 counts as absent.
    assert_eq!(target(None, Some(1)), (212, 5, 264));
    assert_eq!(target(None, Some(60)), (159, 4, 205));
    assert_eq!(target(Some(133), Some(1)), (106, 3, 133));
    assert_eq!(target(Some(0), Some(60)), (159, 4, 205));
    // The target is below the requested height, at a round's end, for every height it accepts.
    let milestones = chain.milestones();
    for height in 54..TIP {
        let plan = plan(&chain, Some(i64::from(height)), None, TIP).unwrap();
        assert!(plan.target < height && plan.target.is_multiple_of(53), "{height}");
        assert!(height - plan.target <= 53, "{height}");
        let next = heartwood_kernel::calculate_round(plan.target + 1, milestones);
        assert_eq!(next.round, plan.round, "{height}");
    }
}

#[test]
fn each_refusal_gives_the_references_lines() {
    let chain = Chain::load();
    let refused = |height, number, current| plan(&chain, height, number, current).unwrap_err();
    let no_request = RollbackRefusal::NoRequest;
    assert_eq!(refused(None, None, TIP), no_request);
    assert_eq!(refused(Some(0), None, TIP), no_request);
    assert_eq!(refused(None, Some(0), TIP), no_request);
    assert_eq!(refused(Some(0), Some(0), TIP), no_request);
    assert_eq!(
        texts(&no_request.lines()),
        ["error: Please specify either a height or number of blocks to roll back"]
    );
    let invalid = |height| RollbackRefusal::Invalid { height };
    assert_eq!(refused(Some(-5), None, TIP), invalid(-5));
    assert_eq!(refused(None, Some(TIP.into()), TIP), invalid(0));
    assert_eq!(refused(None, Some(300), TIP), invalid(-35));
    assert_eq!(
        texts(&invalid(-1234).lines()),
        [
            "info: Rolling back the blockchain",
            "error: Rollback height -1,234 is invalid"
        ]
    );
    let not_below = |height| RollbackRefusal::NotBelowCurrent {
        height,
        current: TIP,
    };
    assert_eq!(refused(Some(265), None, TIP), not_below(265));
    assert_eq!(refused(Some(300), None, TIP), not_below(300));
    assert_eq!(refused(None, Some(-5), TIP), not_below(270));
    assert_eq!(refused(Some(i64::MAX), None, TIP), not_below(i64::MAX));
    let grouped = RollbackRefusal::NotBelowCurrent {
        height: 5000,
        current: 4000,
    };
    assert_eq!(
        texts(&grouped.lines()),
        [
            "info: Rolling back the blockchain",
            "error: Rollback height 5,000 is not less than the current height 4,000"
        ]
    );
    // Any height of round 1: the reference finds no block at the round's first height minus 1.
    let no_block = RollbackRefusal::NoBlock {
        round_height: 1,
        current: TIP,
    };
    for height in [1, 40, 53] {
        assert_eq!(refused(Some(height), None, TIP), no_block, "{height}");
    }
    assert_eq!(refused(None, Some(264), TIP), no_block);
    assert_eq!(
        texts(&no_block.lines()),
        [
            "info: Rolling back the blockchain",
            "info: Last block height is: 265",
            "error: Rollback failed",
            "error: Error: Cannot find block on height 1"
        ]
    );
}

#[test]
fn a_rollback_deletes_the_rows_the_reference_deletes_and_the_state_tables() {
    let chain = Chain::load();
    let mut node = node_at(&chain, "rollback-rows", TIP);
    let dir = node.dir.clone();
    // (height, target, blocks, transactions, missed rows, round rows): the reference's own
    // command on this chain.
    let cases = [
        (264, 212, 53, 0, 0, 106),
        (180, 159, 106, 1, 7, 159),
        (133, 106, 159, 1, 8, 212),
        (60, 53, 212, 40, 8, 265),
    ];
    for (height, target, blocks, transactions, missed, round_rows) in cases {
        let copy = copy_store(&dir, &format!("rollback-rows-{height}"));
        let mut store = heartwood_blockchain::ChainStore::open(&common::settings(&copy)).unwrap();
        let plan = plan(&chain, Some(height), None, TIP).unwrap();
        let rolled = rollback::rollback(&mut store, &plan, chain.milestones()).unwrap();
        let round = plan.round;
        assert_eq!(
            rolled,
            Rolled {
                from: TIP,
                target,
                round,
                blocks,
                transactions,
                missed,
                round_rows,
            },
            "--height {height}"
        );
        assert_eq!(
            rollback::lines(&plan, &rolled),
            [
                "Rolling back the blockchain".to_owned(),
                "Last block height is: 265".to_owned(),
                format!(
                    "Rolling back blockchain to last finished round {round} with last block height {target}"
                ),
            ]
        );
        let database = store.database();
        assert_eq!(database.last_block().unwrap().unwrap().row.height, target);
        assert!(database.state_row().unwrap().is_none(), "{height}");
        assert!(database.wallets().unwrap().is_empty(), "{height}");
        let undo = database.undo_records().unwrap();
        assert_eq!(undo.last().map(|u| u.height), Some(target), "{height}");
        assert!(database.round(round).unwrap().is_empty(), "{height}");
        assert_eq!(database.round(round - 1).unwrap().len(), 53, "{height}");
        let left = database.missed_blocks().unwrap();
        assert!(left.iter().all(|row| row.height <= target), "{height}");
        assert_eq!(left.len(), 8 - usize::try_from(missed).unwrap(), "{height}");
    }
    // From a tip of 214: the reference deleted two blocks and round 5's rows.
    let mut short = node_at(&chain, "rollback-rows-214", 214);
    let plan = plan(&chain, Some(213), None, 214).unwrap();
    let rolled = rollback::rollback(short.store(), &plan, chain.milestones()).unwrap();
    assert_eq!(
        (rolled.target, rolled.blocks, rolled.transactions),
        (212, 2, 0)
    );
    assert_eq!((rolled.missed, rolled.round_rows), (0, 53));
    // The rollback wrote nothing into the store it was copied from.
    node.restart(RestoreMode::Auto);
    assert_eq!(node.state.height(), TIP);
}

/// Roll the chain back from its tip with `--height height`, start the node, and check the start
/// and the round after the target; then re-sync to the tip and check every round's rows.
fn start_after_rollback(height: i64, target: u32, next_round: u64) {
    let chain = Chain::load();
    let mut node = node_at(&chain, &format!("start-after-{height}"), TIP);
    let plan = plan(&chain, Some(height), None, TIP).unwrap();
    rollback::rollback(node.store(), &plan, chain.milestones()).unwrap();
    assert!(stored_round(&mut node, next_round).is_empty());
    let path = node.restart(RestoreMode::Auto);
    assert_eq!(path, RestorePath::Replay(ReplayReason::NoState));
    assert_eq!(node.state.height(), target);
    // The start's line at the target is the judge's line there.
    let forward = forward_lines(&chain);
    let want = forward[&u64::from(target)];
    let [b, s, r, _, w] =
        hex_sections(&digest::records(&node.state, chain.milestones(), &[]).hashes());
    let want_sections = ["b", "s", "r", "w"].map(|k| want[k].as_str().unwrap().to_owned());
    assert_eq!([b, s, r, w], want_sections, "the start's line at {target}");
    // The rows of the round after the target, as the reference's start saves them again.
    let reference = reference_rounds();
    assert_eq!(stored_round(&mut node, next_round), reference[&next_round]);
    assert_eq!(stored_round(&mut node, next_round).len(), 53);
    // The re-sync: every line as the judge's, and every round's rows as the reference node's.
    let resync: Vec<_> = (target + 1..=TIP).map(|h| (Some(h), true)).collect();
    let first = node.lines.len();
    node.steps(&resync);
    for line in &node.lines[first..] {
        let ours: Value = serde_json::from_str(&line.to_json()).unwrap();
        let theirs = forward[&ours["h"].as_u64().unwrap()];
        for key in ["h", "id", "rw", "b", "s", "r", "m", "w", "sd"] {
            assert_eq!(ours[key], theirs[key], "{key} at {}", ours["h"]);
        }
    }
    let rows = stored_rounds(&mut node);
    assert_eq!(rows, reference, "the round rows after the re-sync");
    assert_eq!(rows.values().map(BTreeSet::len).sum::<usize>(), 318);
    assert!(rows.values().all(|round| round.len() == 53));
}

#[test]
fn the_start_after_a_rollback_to_212_writes_round_5_and_the_resync_restores_every_row() {
    // Missed-slot rows are left below the target (the reference's start needs no backfill).
    start_after_rollback(264, 212, 5);
}

#[test]
fn the_start_after_a_rollback_to_106_writes_round_3_and_the_resync_restores_every_row() {
    // No missed-slot row is left (the reference's start runs its productivity backfill).
    start_after_rollback(133, 106, 3);
}

#[test]
fn a_start_from_the_state_tables_at_a_round_end_leaves_the_rows_as_they_are() {
    let chain = Chain::load();
    let mut node = node_at(&chain, "table-round-end", 212);
    let before = stored_rounds(&mut node);
    assert_eq!(before.keys().copied().collect::<Vec<_>>(), [1, 2, 3, 4, 5]);
    assert_eq!(node.restart(RestoreMode::Auto), RestorePath::Table);
    assert_eq!(stored_rounds(&mut node), before);
    // A start by a replay at the same tip writes the same rows.
    assert_eq!(
        node.restart(RestoreMode::Replay),
        RestorePath::Replay(ReplayReason::Requested)
    );
    assert_eq!(stored_rounds(&mut node), before);
    // Inside a round nothing is written.
    let mut inside = node_at(&chain, "table-inside", 230);
    let rows = stored_rounds(&mut inside);
    assert_eq!(inside.restart(RestoreMode::Auto), RestorePath::Table);
    assert_eq!(stored_rounds(&mut inside), rows);
}

#[test]
fn a_start_at_a_round_end_writes_the_next_rounds_rows_a_revert_deleted() {
    // A revert to a round's last block deletes the next round's rows, which the running node
    // does not save again; the reference's start at that tip saves them, and so does this one.
    let chain = Chain::load();
    let mut node = node_at(&chain, "revert-round-end", 214);
    node.steps(&[(None, false), (None, false)]);
    assert_eq!(node.state.height(), 212);
    assert!(stored_round(&mut node, 5).is_empty());
    assert_eq!(node.restart(RestoreMode::Auto), RestorePath::Table);
    assert_eq!(stored_round(&mut node, 5), reference_rounds()[&5]);
}
