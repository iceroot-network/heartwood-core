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
//!
//! A `Map`'s delete and set cost the same whatever its size, and the handlers re-index every
//! party of every transaction, so each index here moves a key to its end at a cost that does not
//! grow with the index (`Ordered`).

use std::fmt;
use std::hash::Hash;

use heartwood_crypto::identities::{Address, PublicKeyBytes};
use indexmap::{Equivalent, IndexMap};

use crate::wallets::wallet::Wallet;

/// Keys and their values in the order the keys were set, as a JavaScript `Map` keeps them: a key
/// set anew goes to the end, a key set again keeps its place, and a key removed and set again goes
/// to the end. A removal leaves a hole in the order, closed up once the holes outnumber the keys,
/// so each operation costs O(1) amortized. Equality ignores the order.
#[derive(Clone)]
struct Ordered<K, V> {
    /// Each key with its place in `order` and its value.
    entries: IndexMap<K, (usize, V)>,
    /// The keys in order, a hole where a key was removed.
    order: Vec<Option<K>>,
}

impl<K: Clone + Eq + Hash, V> Ordered<K, V> {
    fn get<Q: ?Sized + Hash + Equivalent<K>>(&self, key: &Q) -> Option<&V> {
        self.entries.get(key).map(|(_, value)| value)
    }

    /// Set `key` to `value`: in its place if it is there, else at the end.
    fn insert(&mut self, key: K, value: V) {
        if let Some(entry) = self.entries.get_mut(&key) {
            entry.1 = value;
            return;
        }
        self.entries.insert(key.clone(), (self.order.len(), value));
        self.order.push(Some(key));
    }

    fn remove<Q: ?Sized + Hash + Equivalent<K>>(&mut self, key: &Q) -> Option<V> {
        let (place, value) = self.entries.swap_remove(key)?;
        if let Some(hole) = self.order.get_mut(place) {
            *hole = None;
        }
        if self.order.len() > self.entries.len().saturating_mul(2).saturating_add(16) {
            let Ordered { entries, order } = self;
            order.retain(Option::is_some);
            for (place, key) in order.iter().enumerate() {
                if let Some(entry) = key.as_ref().and_then(|key| entries.get_mut(key)) {
                    entry.0 = place;
                }
            }
        }
        Some(value)
    }

    fn iter(&self) -> impl Iterator<Item = (&K, &V)> {
        (self.order.iter().flatten())
            .filter_map(|key| self.entries.get_key_value(key))
            .map(|(key, (_, value))| (key, value))
    }
}

impl<K, V> Default for Ordered<K, V> {
    fn default() -> Self {
        Ordered {
            entries: IndexMap::new(),
            order: Vec::new(),
        }
    }
}

impl<K: Eq + Hash, V: PartialEq> PartialEq for Ordered<K, V> {
    fn eq(&self, other: &Self) -> bool {
        self.entries.len() == other.entries.len()
            && (self.entries.iter())
                .all(|(key, (_, value))| other.entries.get(key).is_some_and(|(_, v)| v == value))
    }
}

impl<K: Eq + Hash, V: Eq> Eq for Ordered<K, V> {}

impl<K: fmt::Debug + Clone + Eq + Hash, V: fmt::Debug> fmt::Debug for Ordered<K, V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map().entries(self.iter()).finish()
    }
}

/// The address, public-key and username indexes, in the reference's orders.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Indexes {
    addresses: Ordered<Address, ()>,
    public_keys: Ordered<PublicKeyBytes, Address>,
    usernames: Ordered<String, Address>,
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
        self.addresses.iter().map(|(address, ())| address)
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
        self.addresses.insert(address, ());
    }

    pub(crate) fn set_public_key(&mut self, public_key: PublicKeyBytes, address: Address) {
        self.public_keys.insert(public_key, address);
    }

    pub(crate) fn forget_public_key(&mut self, public_key: &PublicKeyBytes) {
        self.public_keys.remove(public_key);
    }

    pub(crate) fn set_username(&mut self, username: String, address: Address) {
        self.usernames.insert(username, address);
    }

    pub(crate) fn forget_username(&mut self, username: &str) {
        self.usernames.remove(username);
    }

    /// Re-index `wallet` as the reference's `index(wallet)` does: its address, its public key and
    /// its name leave their indexes and are set again at the end; the address only while the
    /// wallet holds something (a key, a balance, a nonce, a second key or a delegate record).
    pub(crate) fn reindex(&mut self, wallet: &Wallet) {
        let address = *wallet.address();
        self.addresses.remove(&address);
        let holds = wallet.public_key().is_some()
            || wallet.balance() != 0
            || wallet.nonce() != 0
            || wallet.second_public_key().is_some()
            || wallet.is_delegate();
        if holds {
            self.addresses.insert(address, ());
        }
        if let Some(key) = wallet.public_key().map(|key| key.to_compressed())
            && let Some(holder) = self.public_keys.remove(&key)
        {
            self.public_keys.insert(key, holder);
        }
        if let Some(delegate) = wallet.delegate()
            && let Some(holder) = self.usernames.remove(delegate.username())
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
        self.addresses.get(address).is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The same operations on [`Ordered`] and on a list that moves a key by removing it and
    /// pushing it, as a `Map` does, keep the same order, holes closed up on the way.
    #[test]
    fn keys_keep_a_maps_order_through_removals_and_moves() {
        let mut ordered: Ordered<u32, u32> = Ordered::default();
        let mut model: Vec<(u32, u32)> = Vec::new();
        let mut seed = 7u32;
        for step in 0..20_000u32 {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            let key = (seed >> 16) % 300;
            match step % 3 {
                0 | 1 => {
                    ordered.insert(key, step);
                    match model.iter_mut().find(|(k, _)| *k == key) {
                        Some(entry) => entry.1 = step,
                        None => model.push((key, step)),
                    }
                }
                _ => {
                    let removed = ordered.remove(&key);
                    let at = model.iter().position(|(k, _)| *k == key);
                    assert_eq!(removed, at.map(|at| model.remove(at).1));
                }
            }
            if step % 997 == 0 {
                let listed: Vec<(u32, u32)> = ordered.iter().map(|(k, v)| (*k, *v)).collect();
                assert_eq!(listed, model);
                assert!(ordered.order.len() <= 2 * ordered.entries.len() + 17);
            }
        }
        let listed: Vec<(u32, u32)> = ordered.iter().map(|(k, v)| (*k, *v)).collect();
        assert_eq!(listed, model);

        // Equality ignores the order, not the values.
        let mut other: Ordered<u32, u32> = Ordered::default();
        for (key, value) in model.iter().rev() {
            other.insert(*key, *value);
        }
        assert_eq!(ordered, other);
        if let Some((key, value)) = model.first() {
            other.insert(*key, value + 1);
            assert_ne!(ordered, other);
        }
    }
}
