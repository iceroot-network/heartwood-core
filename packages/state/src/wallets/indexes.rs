//! The indexes of the wallet repository: the address index, public key to address, and delegate
//! name to address, each in the reference's order.
//!
//! The reference keeps each index in a JavaScript `Map`, which iterates in insertion order, and
//! its API lists wallets in these orders and breaks ties by them. So each index here keeps its
//! keys in that order: a key set anew goes to the end, a key set again keeps its place, and a key
//! forgotten and set again moves to the end. The reference re-indexes a wallet (`index(wallet)`)
//! by forgetting every key it holds and setting each again, which moves them all to the end, and
//! drops the address of a wallet that holds nothing, and its handlers do that for every
//! transaction's sender and recipients at the end of the block; the repository does the same
//! (`WalletRepository::reindex_deferred`).
//!
//! The ranking, the round and their checks read the names in name order ([`Indexes::usernames`]),
//! so no rule depends on these orders. Equality of indexes ignores the order.

use heartwood_crypto::identities::{Address, PublicKeyBytes};
use indexmap::{IndexMap, IndexSet};

use crate::wallets::wallet::Wallet;

/// The address, public-key and username indexes, in the reference's orders.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Indexes {
    addresses: IndexSet<Address>,
    public_keys: IndexMap<PublicKeyBytes, Address>,
    usernames: IndexMap<String, Address>,
    deferred: Vec<Address>,
}

impl Indexes {
    /// The address whose wallet has bound `public_key`.
    pub fn by_public_key(&self, public_key: &PublicKeyBytes) -> Option<&Address> {
        self.public_keys.get(public_key)
    }

    /// The address of the delegate named `username`.
    pub fn by_username(&self, username: &str) -> Option<&Address> {
        self.usernames.get(username)
    }

    /// Every delegate name with its address, in name order.
    pub fn usernames(&self) -> impl Iterator<Item = (&str, &Address)> {
        let mut names: Vec<(&str, &Address)> = self.usernames_in_order().collect();
        names.sort_unstable_by(|a, b| a.0.cmp(b.0));
        names.into_iter()
    }

    /// Every address in the reference's address index order: the order the wallets were created
    /// or last re-indexed, without the wallets a re-index found holding nothing.
    pub fn addresses_in_order(&self) -> impl Iterator<Item = &Address> {
        self.addresses.iter()
    }

    /// Every bound public key with its address, in the reference's public-key index order: the
    /// order the keys were set, a key forgotten and set again, or re-indexed, moving to the end.
    pub fn public_keys_in_order(&self) -> impl Iterator<Item = (&PublicKeyBytes, &Address)> {
        self.public_keys.iter()
    }

    /// Every delegate name with its address, in the reference's username index order: the order
    /// the names were set, a name forgotten and set again, or re-indexed, moving to the end.
    /// [`Indexes::usernames`] keeps the name order for the ranking, the round and their checks.
    pub fn usernames_in_order(&self) -> impl Iterator<Item = (&str, &Address)> {
        self.usernames
            .iter()
            .map(|(username, address)| (username.as_str(), address))
    }

    /// Set `address` in the address index when it is not there, as a lookup that creates its
    /// wallet does.
    pub(crate) fn set_address(&mut self, address: Address) {
        self.addresses.insert(address);
    }

    pub(crate) fn set_public_key(&mut self, public_key: PublicKeyBytes, address: Address) {
        self.public_keys.insert(public_key, address);
    }

    pub(crate) fn forget_public_key(&mut self, public_key: &PublicKeyBytes) {
        self.public_keys.shift_remove(public_key);
    }

    pub(crate) fn set_username(&mut self, username: String, address: Address) {
        self.usernames.insert(username, address);
    }

    pub(crate) fn forget_username(&mut self, username: &str) {
        self.usernames.shift_remove(username);
    }

    /// Re-index `wallet` as the reference's `index(wallet)` does: its address, its public key and
    /// its name leave their indexes and are set again at the end; the address only while the
    /// wallet holds something (a key, a balance, a nonce, a second key or a delegate record).
    pub(crate) fn reindex(&mut self, wallet: &Wallet) {
        let address = *wallet.address();
        self.addresses.shift_remove(&address);
        let holds = wallet.public_key().is_some()
            || wallet.balance() != 0
            || wallet.nonce() != 0
            || wallet.second_public_key().is_some()
            || wallet.is_delegate();
        if holds {
            self.addresses.insert(address);
        }
        if let Some(key) = wallet.public_key().map(|key| key.to_compressed())
            && let Some(holder) = self.public_keys.shift_remove(&key)
        {
            self.public_keys.insert(key, holder);
        }
        if let Some(delegate) = wallet.delegate()
            && let Some(holder) = self.usernames.shift_remove(delegate.username())
        {
            self.usernames
                .insert(delegate.username().to_owned(), holder);
        }
    }

    /// Queue `address` for a re-index at the end of the block.
    pub(crate) fn defer(&mut self, address: Address) {
        self.deferred.push(address);
    }

    /// The queued addresses, in the order they were queued, leaving the queue empty.
    pub(crate) fn take_deferred(&mut self) -> Vec<Address> {
        std::mem::take(&mut self.deferred)
    }

    /// Whether `address` is in the address index.
    pub(crate) fn has_address(&self, address: &Address) -> bool {
        self.addresses.contains(address)
    }
}
