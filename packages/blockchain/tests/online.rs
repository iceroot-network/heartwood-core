//! The pool's online record in the store: saved, it comes back from every later start, by either
//! path, with each entry's round, so it expires after the restart as it would have without it.

mod common;

use common::{Chain, scratch, start};
use heartwood_blockchain::{RestoreMode, RestorePath};
use heartwood_crypto::identities::PublicKeyBytes;
use heartwood_pool::DelegateVersions;

fn key(n: u8) -> PublicKeyBytes {
    PublicKeyBytes::from_array([n; 33])
}

#[test]
fn the_online_record_comes_back_from_every_start_with_its_rounds() {
    let chain = Chain::load();
    let dir = scratch("online-record");
    let (mut store, restored) = start(&dir, &chain.files, RestoreMode::Auto).unwrap();
    assert_eq!(restored.path, RestorePath::FirstStart);
    assert_eq!(restored.online, DelegateVersions::new());

    let mut record = DelegateVersions::new();
    record.set(key(2), 3, "4.3.1".into());
    record.set(key(3), 9, "4.3.2".into());
    store.save_online(&record).unwrap();
    drop(store);
    for mode in [RestoreMode::Auto, RestoreMode::Replay] {
        let (store, restored) = start(&dir, &chain.files, mode).unwrap();
        assert_eq!(restored.online, record);
        // The rounds came back: the round start of round 9 expires the entry of round 3 only.
        let mut loaded = restored.online;
        loaded.expire(9);
        assert_eq!(loaded.version(&key(2)), None);
        assert_eq!(loaded.version(&key(3)), Some("4.3.2"));
        let mut kept = record.clone();
        kept.expire(9);
        assert_eq!(loaded, kept);
        drop(store);
    }

    // A saved record replaces the stored one, an empty one included.
    let (mut store, _) = start(&dir, &chain.files, RestoreMode::Auto).unwrap();
    let mut changed = DelegateVersions::new();
    changed.set(key(4), 12, "4.3.3".into());
    store.save_online(&changed).unwrap();
    assert_eq!(store.online().unwrap(), changed);
    drop(store);
    let (mut store, restored) = start(&dir, &chain.files, RestoreMode::Auto).unwrap();
    assert_eq!(restored.online, changed);
    store.save_online(&DelegateVersions::new()).unwrap();
    drop(store);
    let (_, restored) = start(&dir, &chain.files, RestoreMode::Auto).unwrap();
    assert_eq!(restored.online, DelegateVersions::new());
}
