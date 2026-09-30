//! Every wallet, by address.
//!
//! The repository holds `Arc<Wallet>` values in an `IndexMap` by address, in the order the
//! wallets were first used. A wallet is copied on write only while another clone of the
//! repository (the pool's view, a published snapshot) still shares it, so cloning the repository
//! is cheap. As in the reference, looking a wallet up for a change creates it when it does not
//! exist yet; a created wallet that stays empty is invisible to the rules and to the state digest.
//!
//! The public key and the delegate record of a wallet change only through the repository, which
//! keeps its [`Indexes`] in step with them, in the reference's orders: a wallet created by a
//! lookup is set in the address index, and binding a key or registering a name re-indexes the
//! wallet, as the reference's handlers do. After each transaction a block applies or reverts, its
//! sender and its recipients are re-indexed at the end of the block, as the reference's handlers
//! defer it. The reference's handlers re-index them in the state's wallets whatever wallets they
//! ran on (a pool's view, a collation's copy), so a transaction run on a copy re-indexes its
//! parties here too ([`WalletRepository::reindex_parties`]).

use std::sync::Arc;

use heartwood_crypto::Transaction;
use heartwood_crypto::identities::{Address, PublicKey, PublicKeyBytes};
use heartwood_crypto::transactions::Asset;
use indexmap::IndexMap;

use crate::errors::WalletError;
use crate::persist::{RestoreError, bad};
use crate::wallets::indexes::Indexes;
use crate::wallets::wallet::{Delegate, Wallet};

/// Every wallet of the state, with the public-key and username indexes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalletRepository {
    network: u8,
    wallets: IndexMap<Address, Arc<Wallet>>,
    indexes: Indexes,
}

impl WalletRepository {
    /// An empty repository for the network with address byte `network`.
    pub fn new(network: u8) -> WalletRepository {
        WalletRepository {
            network,
            wallets: IndexMap::new(),
            indexes: Indexes::default(),
        }
    }

    /// The address byte of the network.
    pub fn network(&self) -> u8 {
        self.network
    }

    /// The number of wallets, empty ones created by a lookup included.
    pub fn len(&self) -> usize {
        self.wallets.len()
    }

    /// Whether the repository has no wallet.
    pub fn is_empty(&self) -> bool {
        self.wallets.is_empty()
    }

    /// The indexes.
    pub fn indexes(&self) -> &Indexes {
        &self.indexes
    }

    /// Whether a wallet exists at `address`.
    pub fn has_by_address(&self, address: &Address) -> bool {
        self.wallets.contains_key(address)
    }

    /// The wallet at `address`, if it exists.
    pub fn find_by_address(&self, address: &Address) -> Option<&Wallet> {
        self.wallets.get(address).map(Arc::as_ref)
    }

    /// Whether a wallet has bound `public_key`.
    pub fn has_by_public_key(&self, public_key: &PublicKeyBytes) -> bool {
        self.indexes.by_public_key(public_key).is_some()
    }

    /// The wallet that has bound `public_key`.
    pub fn find_by_public_key(&self, public_key: &PublicKeyBytes) -> Option<&Wallet> {
        self.indexes
            .by_public_key(public_key)
            .and_then(|address| self.find_by_address(address))
    }

    /// Whether a delegate is registered as `username`.
    pub fn has_by_username(&self, username: &str) -> bool {
        self.indexes.by_username(username).is_some()
    }

    /// The wallet of the delegate registered as `username`.
    pub fn find_by_username(&self, username: &str) -> Option<&Wallet> {
        self.indexes
            .by_username(username)
            .and_then(|address| self.find_by_address(address))
    }

    /// The wallet at `address`, created empty if it does not exist, to change it. An address that
    /// is not in the address index is set there, as the reference's lookup creates its wallet.
    pub fn get_or_create(&mut self, address: &Address) -> &mut Wallet {
        self.indexes.set_address(*address);
        let entry = self
            .wallets
            .entry(*address)
            .or_insert_with(|| Arc::new(Wallet::new(*address)));
        Arc::make_mut(entry)
    }

    /// The wallet at `address`, to change it, if it exists.
    pub fn wallet_mut(&mut self, address: &Address) -> Option<&mut Wallet> {
        self.wallets.get_mut(address).map(Arc::make_mut)
    }

    /// The wallet of the delegate registered as `username`, to change it.
    pub fn delegate_mut(&mut self, username: &str) -> Result<&mut Wallet, WalletError> {
        let address =
            *self
                .indexes
                .by_username(username)
                .ok_or_else(|| WalletError::UnknownDelegate {
                    username: username.to_owned(),
                })?;
        self.wallet_mut(&address)
            .ok_or(WalletError::UnknownWallet { address })
    }

    /// Bind `public_key` to the wallet at `address` (created if needed) and re-index the wallet.
    pub fn bind_public_key(&mut self, address: &Address, public_key: PublicKey) {
        self.indexes
            .set_public_key(public_key.to_compressed(), *address);
        self.get_or_create(address).set_public_key(Some(public_key));
        self.reindex(address);
    }

    /// Forget the public key of the wallet at `address` and its index entry.
    pub fn forget_public_key(&mut self, address: &Address) {
        if let Some(wallet) = self.wallet_mut(address)
            && let Some(public_key) = wallet.public_key().map(PublicKey::to_compressed)
        {
            wallet.set_public_key(None);
            self.indexes.forget_public_key(&public_key);
        }
    }

    /// Register the wallet at `address` (created if needed) as a new delegate named `username`,
    /// and index the name. Refused when another wallet holds the name.
    pub fn register_delegate(
        &mut self,
        address: &Address,
        username: String,
    ) -> Result<&mut Delegate, WalletError> {
        if self
            .indexes
            .by_username(&username)
            .is_some_and(|holder| holder != address)
        {
            return Err(WalletError::UsernameTaken { username });
        }
        let delegate = Delegate::new(username.clone());
        if let Some(previous) = self.get_or_create(address).set_delegate(Some(delegate)) {
            self.indexes.forget_username(previous.username());
        }
        self.indexes.set_username(username, *address);
        self.reindex(address);
        self.get_or_create(address)
            .delegate_mut()
            .ok_or(WalletError::NotADelegate { address: *address })
    }

    /// Forget the delegate record of the wallet at `address` and its name's index entry,
    /// returning the record.
    pub fn forget_delegate(&mut self, address: &Address) -> Option<Delegate> {
        let delegate = self.wallet_mut(address)?.set_delegate(None)?;
        self.indexes.forget_username(delegate.username());
        Some(delegate)
    }

    /// Every wallet, in the order they were first used.
    pub fn iter(&self) -> impl Iterator<Item = &Wallet> {
        self.wallets.values().map(Arc::as_ref)
    }

    /// Every delegate's wallet, in name order.
    pub fn delegates(&self) -> impl Iterator<Item = &Wallet> {
        self.indexes
            .usernames()
            .filter_map(|(_, address)| self.find_by_address(address))
    }

    /// Every wallet in the reference's address index order (see [`Indexes::addresses_in_order`]).
    pub fn by_address_index(&self) -> impl Iterator<Item = &Wallet> {
        self.indexes
            .addresses_in_order()
            .filter_map(|address| self.find_by_address(address))
    }

    /// Every wallet with a public key, in the reference's public-key index order.
    pub fn by_public_key_index(&self) -> impl Iterator<Item = &Wallet> {
        self.indexes
            .public_keys_in_order()
            .filter_map(|(_, address)| self.find_by_address(address))
    }

    /// Every delegate's wallet, in the reference's username index order.
    pub fn by_username_index(&self) -> impl Iterator<Item = &Wallet> {
        self.indexes
            .usernames_in_order()
            .filter_map(|(_, address)| self.find_by_address(address))
    }

    /// Re-index the wallet at `address`, if it exists (see the module documentation).
    pub(crate) fn reindex(&mut self, address: &Address) {
        if let Some(wallet) = self.wallets.get(address) {
            self.indexes.reindex(wallet);
        }
    }

    /// Queue the sender and the recipients of `transaction`, which a block just applied or
    /// reverted, for a re-index at the end of the block.
    pub(crate) fn defer_reindex(&mut self, transaction: &Transaction) {
        for address in parties(transaction, self.network) {
            self.indexes.defer(address);
        }
    }

    /// Re-index the queued wallets that are in the address index, in the order they were queued,
    /// as the reference's handlers do after a block.
    pub(crate) fn reindex_deferred(&mut self) {
        let deferred = self.indexes.take_deferred();
        self.reindex_parties(deferred);
    }

    /// Re-index the wallets at `addresses` that are in the address index, in order: the parties
    /// ([`parties`]) of transactions the handlers applied or reverted on a copy of these wallets,
    /// which the reference's handlers re-index in the state's wallets too. Only the indexes'
    /// orders change.
    pub fn reindex_parties(&mut self, addresses: impl IntoIterator<Item = Address>) {
        for address in addresses {
            if self.indexes.has_address(&address) {
                self.reindex(&address);
            }
        }
    }
}

/// The sender and the recipients of `transaction` on the network with address byte `network`, in
/// the order the reference's handlers re-index them after they apply or revert it.
pub fn parties(transaction: &Transaction, network: u8) -> impl Iterator<Item = Address> + '_ {
    let data = transaction.data();
    let sender = Address::from_public_key(&data.sender_public_key, network);
    let recipients = match &data.asset {
        Asset::Transfer { transfers } => transfers.as_slice(),
        _ => &[],
    };
    std::iter::once(sender).chain(recipients.iter().map(|item| item.recipient))
}

/// The repository read back from the store, and what changed since the last commit.
impl WalletRepository {
    /// The repository of `wallets`, in their order, on the network with address byte `network`,
    /// with the indexes built again in that one order, as the reference's saved-state loader
    /// re-indexes each wallet it reads. Refused when two wallets hold the same key or the same
    /// name.
    pub fn from_wallets(
        network: u8,
        wallets: Vec<Wallet>,
    ) -> Result<WalletRepository, RestoreError> {
        let mut repository = WalletRepository::new(network);
        for wallet in wallets {
            let address = *wallet.address();
            if let Some(key) = wallet.public_key() {
                if repository.has_by_public_key(&key.to_compressed()) {
                    return Err(RestoreError::DuplicatePublicKey(key.to_hex()));
                }
                (repository.indexes).set_public_key(key.to_compressed(), address);
            }
            if let Some(delegate) = wallet.delegate() {
                if repository.has_by_username(delegate.username()) {
                    return Err(RestoreError::DuplicateUsername(delegate.username().into()));
                }
                (repository.indexes).set_username(delegate.username().into(), address);
            }
            repository.indexes.reindex(&wallet);
            let held = repository.wallets.insert(address, Arc::new(wallet));
            if held.is_some() {
                return Err(bad("wallet", format!("{address} is held twice")));
            }
        }
        Ok(repository)
    }

    /// The wallets that changed since `earlier`, a clone of this repository taken before: those
    /// that `earlier` does not have, and those whose value is no longer shared with it. A wallet
    /// is copied when it changes while a clone shares it, so this compares pointers, not values.
    pub fn changed_since<'a>(
        &'a self,
        earlier: &'a WalletRepository,
    ) -> impl Iterator<Item = &'a Wallet> + 'a {
        self.wallets
            .iter()
            .filter(|(address, wallet)| {
                earlier
                    .wallets
                    .get(*address)
                    .is_none_or(|before| !Arc::ptr_eq(before, wallet))
            })
            .map(|(_, wallet)| wallet.as_ref())
    }
}

#[cfg(test)]
mod tests {
    use heartwood_crypto::KeyPair;
    use heartwood_crypto::transactions::types::Votes;

    use super::*;

    const NETWORK: u8 = 90;

    fn key(passphrase: &str) -> (PublicKey, Address) {
        let keys = KeyPair::from_passphrase(passphrase).unwrap();
        let public_key = keys.public_key().clone();
        let address = Address::from_public_key(&public_key, NETWORK);
        (public_key, address)
    }

    #[test]
    fn balances_never_wrap() {
        let (_, address) = key("balance");
        let mut wallet = Wallet::new(address);
        wallet.credit(u64::MAX).unwrap();
        assert_eq!(
            wallet.credit(1),
            Err(WalletError::BalanceOverflow { address })
        );
        wallet.debit(u64::MAX).unwrap();
        assert_eq!(
            wallet.debit(1),
            Err(WalletError::NegativeBalance { address })
        );
        assert_eq!(wallet.balance(), 0);
    }

    #[test]
    fn lookups_for_a_change_create_empty_wallets() {
        let (_, address) = key("lazy");
        let mut wallets = WalletRepository::new(NETWORK);
        assert!(wallets.find_by_address(&address).is_none());
        assert_eq!(wallets.get_or_create(&address).balance(), 0);
        assert!(wallets.has_by_address(&address));
        assert_eq!(wallets.len(), 1);
    }

    #[test]
    fn keys_and_names_stay_indexed() {
        let (public_key, address) = key("indexed");
        let (_, other) = key("other");
        let compressed = public_key.to_compressed();
        let mut wallets = WalletRepository::new(NETWORK);

        wallets.bind_public_key(&address, public_key);
        assert_eq!(
            wallets.find_by_public_key(&compressed).map(Wallet::address),
            Some(&address)
        );
        wallets
            .register_delegate(&address, "delegate_a".into())
            .unwrap();
        assert_eq!(
            wallets.register_delegate(&other, "delegate_a".into()),
            Err(WalletError::UsernameTaken {
                username: "delegate_a".into()
            })
        );
        wallets
            .register_delegate(&other, "delegate_0".into())
            .unwrap();
        let names: Vec<&str> = wallets
            .delegates()
            .filter_map(Wallet::delegate)
            .map(Delegate::username)
            .collect();
        assert_eq!(names, ["delegate_0", "delegate_a"]);
        wallets.delegate_mut("delegate_a").unwrap().set_nonce(3);
        assert_eq!(
            wallets.find_by_username("delegate_a").map(Wallet::nonce),
            Some(3)
        );

        let forgotten = wallets.forget_delegate(&address).unwrap();
        assert_eq!(forgotten.username(), "delegate_a");
        assert!(!wallets.has_by_username("delegate_a"));
        wallets.forget_public_key(&address);
        assert!(!wallets.has_by_public_key(&compressed));
        assert!(
            wallets
                .find_by_address(&address)
                .unwrap()
                .public_key()
                .is_none()
        );
    }

    #[test]
    fn votes_are_kept_in_the_canonical_order() {
        let (_, address) = key("votes");
        let mut wallet = Wallet::new(address);
        let votes: Votes = [("b".to_owned(), 5_000), ("a".to_owned(), 5_000)]
            .into_iter()
            .collect();
        wallet.set_votes(&votes);
        let names: Vec<&str> = wallet.votes().keys().map(String::as_str).collect();
        assert_eq!(names, ["a", "b"]);
        assert!(wallet.has_voted());
    }
}
