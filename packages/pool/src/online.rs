//! The node-local record of the delegates that operate a node: the version each delegate's node
//! last announced, and the round in which it did.
//!
//! Peers' announcements and the node's own forger set entries (the reference's
//! `delegate.version` attribute); at each round start an entry older than five rounds expires.
//! The pool's pool-entry checks read the record ([`OnlineView`]): a vote for a delegate that has
//! not resigned and has no entry, or a revoke by one, is refused with `ERR_OFFLINE`. The record
//! never enters consensus; `heartwood-blockchain` saves it in the node's store and every start
//! loads it back ([`DelegateVersions::entries`] and [`DelegateVersions::from_entries`] are the
//! rows' form), so it survives a restart with each entry's round.

use std::collections::BTreeMap;

use heartwood_crypto::identities::PublicKeyBytes;
use heartwood_state::wallets::WalletRepository;
use heartwood_transactions::OnlineDelegates;

/// How many rounds an entry lives: at the start of round `r`, an entry of a round below `r − 5`
/// expires.
pub const EXPIRY_ROUNDS: u64 = 5;

/// One delegate's entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OnlineEntry {
    /// The delegate's public key.
    pub public_key: PublicKeyBytes,
    /// The round in which the version was announced.
    pub round: u64,
    /// The announced version.
    pub version: String,
}

/// The record: an entry per delegate public key.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DelegateVersions {
    entries: BTreeMap<PublicKeyBytes, (u64, String)>,
}

impl DelegateVersions {
    /// An empty record.
    pub fn new() -> DelegateVersions {
        DelegateVersions::default()
    }

    /// The record of `entries`, as the node loads it at start.
    pub fn from_entries(entries: impl IntoIterator<Item = OnlineEntry>) -> DelegateVersions {
        DelegateVersions {
            entries: entries
                .into_iter()
                .map(|entry| (entry.public_key, (entry.round, entry.version)))
                .collect(),
        }
    }

    /// Every entry, by public key, as the node stores it.
    pub fn entries(&self) -> Vec<OnlineEntry> {
        self.entries
            .iter()
            .map(|(public_key, (round, version))| OnlineEntry {
                public_key: *public_key,
                round: *round,
                version: version.clone(),
            })
            .collect()
    }

    /// Record that the node of the delegate with `public_key` announced `version` in `round`.
    pub fn set(&mut self, public_key: PublicKeyBytes, round: u64, version: String) {
        self.entries.insert(public_key, (round, version));
    }

    /// The version the delegate with `public_key` last announced, if its entry has not expired.
    pub fn version(&self, public_key: &PublicKeyBytes) -> Option<&str> {
        self.entries
            .get(public_key)
            .map(|(_, version)| version.as_str())
    }

    /// Expire the entries at the start of `round`, as the reference's round start does: an
    /// entry of round 0 or of a round below `round − 5` is removed.
    pub fn expire(&mut self, round: u64) {
        self.entries.retain(|_, (announced, _)| {
            *announced != 0 && announced.saturating_add(EXPIRY_ROUNDS) >= round
        });
    }

    /// The record as the pool-entry checks read it, with delegate names looked up in `wallets`.
    pub fn view<'a>(&'a self, wallets: &'a WalletRepository) -> OnlineView<'a> {
        OnlineView {
            versions: self,
            wallets,
        }
    }
}

/// The record with the wallets that map a delegate name to its key.
#[derive(Debug, Clone, Copy)]
pub struct OnlineView<'a> {
    versions: &'a DelegateVersions,
    wallets: &'a WalletRepository,
}

impl OnlineDelegates for OnlineView<'_> {
    /// Whether the delegate named `username` in the view's wallets has an entry.
    fn is_online(&self, username: &str) -> bool {
        self.wallets
            .find_by_username(username)
            .and_then(|wallet| wallet.public_key())
            .is_some_and(|key| self.versions.version(&key.to_compressed()).is_some())
    }
}

#[cfg(test)]
mod tests {
    use heartwood_crypto::KeyPair;
    use heartwood_crypto::identities::Address;

    use super::*;

    fn key(n: u8) -> PublicKeyBytes {
        PublicKeyBytes::from_array([n; 33])
    }

    #[test]
    fn entries_expire_after_five_rounds() {
        let mut versions = DelegateVersions::new();
        versions.set(key(1), 10, "4.3.1".into());
        versions.set(key(2), 12, "4.3.2".into());
        versions.set(key(3), 0, "0.0.1".into());
        versions.expire(15);
        assert_eq!(versions.version(&key(1)), Some("4.3.1"));
        assert_eq!(versions.version(&key(3)), None, "round 0 always expires");
        versions.expire(16);
        assert_eq!(versions.version(&key(1)), None, "10 < 16 - 5");
        assert_eq!(versions.version(&key(2)), Some("4.3.2"));
        versions.expire(17);
        assert_eq!(versions.version(&key(2)), Some("4.3.2"));
        versions.expire(18);
        assert!(versions.entries().is_empty());
        // Rounds below 5 expire nothing but round 0.
        versions.set(key(4), 1, "v".into());
        versions.expire(3);
        assert_eq!(versions.version(&key(4)), Some("v"));
    }

    #[test]
    fn entries_round_trip() {
        let mut versions = DelegateVersions::new();
        versions.set(key(2), 7, "b".into());
        versions.set(key(1), 8, "a".into());
        versions.set(key(2), 9, "c".into());
        let entries = versions.entries();
        assert_eq!(entries.len(), 2);
        assert_eq!((entries[0].public_key, entries[0].round), (key(1), 8));
        assert_eq!(DelegateVersions::from_entries(entries), versions);
    }

    #[test]
    fn the_view_looks_names_up_in_its_wallets() {
        let keys = KeyPair::from_passphrase("online delegate").unwrap();
        let address = Address::from_public_key(keys.public_key(), 90);
        let mut wallets = WalletRepository::new(90);
        wallets.bind_public_key(&address, keys.public_key().clone());
        wallets.register_delegate(&address, "del_a".into()).unwrap();
        let mut versions = DelegateVersions::new();
        assert!(!versions.view(&wallets).is_online("del_a"));
        versions.set(keys.public_key().to_compressed(), 3, "4.3.1".into());
        assert!(versions.view(&wallets).is_online("del_a"));
        assert!(!versions.view(&wallets).is_online("nobody"));
    }
}
