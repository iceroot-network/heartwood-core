//! The applied milestone entries: a start refuses a configuration in which an entry the
//! tip has reached changed, disappeared, or appeared at or below the tip; an entry above the tip,
//! and the keys of network and pool policy, may change. The digests themselves: key order does
//! not count, and neither does an excluded key.

mod common;

use common::{Chain, Node, files_with, read};
use heartwood_blockchain::milestones::{EXCLUDED_KEYS, entry_digests, first_changed, reached};
use heartwood_blockchain::{RestoreMode, RestorePath, StartError, StoreError};
use heartwood_database::MilestoneEntryRow;
use heartwood_transactions::Handlers;
use serde_json::{Value, json};

/// The corpus's milestone file changed by `edit`.
fn edited(edit: impl FnOnce(&mut Vec<Value>)) -> String {
    let mut entries: Vec<Value> = serde_json::from_str(&read("crypto/milestones.json")).unwrap();
    edit(&mut entries);
    serde_json::to_string_pretty(&entries).unwrap()
}

fn restart_with(
    node: &mut Node<'_>,
    text: &str,
) -> Result<RestorePath, StartError<heartwood_transactions::TransactionError>> {
    node.restart_with(&files_with(text), RestoreMode::Auto)
        .map(|restored| restored.path)
}

#[test]
fn a_changed_applied_entry_refuses_the_start() {
    let chain = Chain::load();
    let mut node = Node::first_start(&chain, "milestones");
    for height in 2..=10 {
        node.step((Some(height), true));
    }
    let heights: Vec<u32> = (node.store().database().milestone_entries().unwrap().iter())
        .map(|entry| entry.height)
        .collect();
    assert_eq!(heights, [1, 2]);

    // A consensus key of an applied entry.
    let text = edited(|e| e[0]["blocksToRevokeDelegateResignation"] = json!(107));
    let error = restart_with(&mut node, &text).unwrap_err();
    assert_eq!(error, StartError::MilestoneChanged { height: 1 });
    let text = edited(|e| {
        e[1]["donations"]["dZDbtMv86KMqo8iPMtAJLhoT5BVKfyVokA"]["basisPoints"] = json!(400)
    });
    let error = restart_with(&mut node, &text).unwrap_err();
    assert_eq!(
        error.to_string(),
        "the milestone at height 2 changed after it was applied; roll back to height 1 or resync"
    );
    // A removed entry, and a new one at or below the tip.
    let error = restart_with(&mut node, &edited(|e| drop(e.remove(1)))).unwrap_err();
    assert_eq!(error, StartError::MilestoneChanged { height: 2 });
    let text = edited(|e| e.push(json!({"height": 10, "blocksToRevokeDelegateResignation": 106})));
    let error = restart_with(&mut node, &text).unwrap_err();
    assert_eq!(error, StartError::MilestoneChanged { height: 10 });

    // The version floor and the static fees are not consensus keys; an entry above the tip
    // has not been applied.
    let text = edited(|e| {
        e[0]["p2p"]["minimumVersions"] = json!([">=4.3.1"]);
        e[0]["fees"]["staticFees"]["transfer"] = json!(1);
        e.push(json!({"height": 11, "blocksToRevokeDelegateResignation": 106}));
    });
    assert_eq!(restart_with(&mut node, &text), Ok(RestorePath::Table));
    // The tip reaches the new entry: from then on it is applied.
    let files = files_with(&text);
    let milestones = &files.milestones;
    let (store, state) = node.parts();
    let block = chain.block(11);
    let applied = state.apply_block(block, milestones, &Handlers).unwrap();
    let mut chunk = store.chunk().unwrap();
    chunk.applied(state, block, &applied, milestones).unwrap();
    chunk.commit().unwrap();
    let digests = entry_digests(&text).unwrap();
    assert_eq!(
        node.store().database().milestone_entries().unwrap(),
        digests
    );
    let error = restart_with(&mut node, &read("crypto/milestones.json")).unwrap_err();
    assert_eq!(error, StartError::MilestoneChanged { height: 11 });
    assert_eq!(restart_with(&mut node, &text), Ok(RestorePath::Table));

    // A revert below an entry's height takes its digest back.
    let (store, state) = node.parts();
    let reverted = state.revert_block(milestones, &Handlers).unwrap();
    let mut chunk = store.chunk().unwrap();
    chunk.reverted(state, &reverted, milestones).unwrap();
    chunk.commit().unwrap();
    assert_eq!(
        node.store().database().milestone_entries().unwrap(),
        digests[..2]
    );
    assert_eq!(
        restart_with(&mut node, &read("crypto/milestones.json")),
        Ok(RestorePath::Table)
    );
}

#[test]
fn digests_leave_out_key_order_and_the_policy_keys() {
    assert_eq!(EXCLUDED_KEYS, ["p2p", "fees.staticFees"]);
    let digest = |entry: Value| entry_digests(&json!([entry]).to_string()).unwrap()[0];
    let base = digest(json!({"height": 1, "reward": 5, "fees": {"x": 1}}));
    assert_eq!(base.height, 1);
    assert_eq!(
        digest(json!({"fees": {"x": 1}, "reward": 5, "height": 1})),
        base
    );
    assert_eq!(
        digest(json!({"height": 1, "reward": 5, "fees": {"x": 1, "staticFees": {"a": 1}}})),
        base
    );
    assert_eq!(
        digest(json!({"height": 1, "reward": 5, "fees": {"x": 1}, "p2p": {"minimumVersions": []}})),
        base
    );
    assert_ne!(
        digest(json!({"height": 1, "reward": 6, "fees": {"x": 1}})),
        base
    );
    assert_ne!(
        digest(json!({"height": 2, "reward": 5, "fees": {"x": 1}})),
        base
    );
    // An object left empty without its excluded keys is no key at all: `fees` holds only the
    // static fees, so it is pool policy whole.
    let bare = digest(json!({"height": 1}));
    assert_eq!(
        digest(json!({"height": 1, "fees": {"staticFees": {}}})),
        bare
    );
    assert_eq!(digest(json!({"height": 1, "fees": {}})), bare);
    assert_ne!(digest(json!({"height": 1, "burn": {}})), bare);

    let refused = |text: &str| matches!(entry_digests(text), Err(StoreError::Milestones(_)));
    assert!(refused("{}"));
    assert!(refused("[{\"reward\": 5}]"));
    assert!(refused("[{\"height\": 4294967296}]"));
    assert!(refused("not json"));
}

#[test]
fn entries_reached_and_the_first_changed_height() {
    let entry = |height: u32, byte: u8| MilestoneEntryRow {
        height,
        digest: [byte; 32],
    };
    let configured = [entry(1, 1), entry(5, 5), entry(9, 9)];
    assert_eq!(reached(&configured, 0, 1), [entry(1, 1)]);
    assert_eq!(reached(&configured, 1, 9), [entry(5, 5), entry(9, 9)]);
    assert_eq!(reached(&configured, 5, 8), []);
    assert_eq!(first_changed(&configured[..2], &configured, 8), None);
    assert_eq!(first_changed(&configured[..2], &configured, 9), Some(9));
    assert_eq!(
        first_changed(&[entry(1, 1), entry(5, 6)], &configured, 8),
        Some(5)
    );
    assert_eq!(
        first_changed(&[entry(1, 1), entry(4, 4)], &configured, 8),
        Some(4)
    );
    assert_eq!(first_changed(&[entry(1, 1)], &configured, 8), Some(5));
    assert_eq!(first_changed(&[], &configured, 0), None);
}
