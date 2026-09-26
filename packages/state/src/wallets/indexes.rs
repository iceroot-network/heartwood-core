//! The indexes of the wallet repository: public key to address, and delegate name to address.
//!
//! Both are `BTreeMap`s: the order in which the reference's indexes iterate is never observable
//! (the ranking sorts completely, and sums do not depend on order), so a sorted map gives the same
//! results on every machine.

use std::collections::BTreeMap;

use heartwood_crypto::identities::{Address, PublicKeyBytes};

/// The public-key and username indexes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Indexes {
    public_keys: BTreeMap<PublicKeyBytes, Address>,
    usernames: BTreeMap<String, Address>,
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
        self.usernames
            .iter()
            .map(|(username, address)| (username.as_str(), address))
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
}
