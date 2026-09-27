//! The node-local record of the delegates that operate a node: the version each delegate's node
//! last announced, and the round in which it did.
//!
//! Peers' announcements and the node's own forger set entries (the reference's
//! `delegate.version` attribute); at each round start an entry older than five rounds expires.
//! The pool's pool-entry checks read the record ([`OnlineView`]): a vote for a delegate that has
//! not resigned and has no entry, or a revoke by one, is refused with `ERR_OFFLINE`. The record
//! never enters consensus; the node stores it in its database and loads it at start, so it
//! survives a restart.

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

    /// Expire the entries at the start of `round`, as the reference's round start does.
    pub fn expire(&mut self, round: u64) {
        let _ = round;
        todo!("expire the entries older than five rounds")
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
    fn is_online(&self, username: &str) -> bool {
        let _ = (username, self.versions, self.wallets);
        todo!("whether the delegate has an entry")
    }
}
