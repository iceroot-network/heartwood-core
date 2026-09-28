//! One sender's pending transactions, in arrival order, with the sender's own view of the
//! wallets.
//!
//! As in the reference, each sender mempool has its own copy of the state's wallets, so a
//! sender's view never sees another sender's pending transactions: a pending transfer to a
//! wallet does not fund that wallet's own pool transactions.
//!
//! The transactions are held shared ([`Arc`]), so that a published view of the pool
//! ([`crate::PoolView`]) holds the same ones and copies none.

use std::slice::Iter;
use std::sync::Arc;

use heartwood_crypto::identities::Address;
use heartwood_crypto::{Transaction, TransactionId};
use heartwood_state::undo::TxUndo;
use heartwood_state::wallets::{Wallet, WalletRepository};
use heartwood_transactions::PoolQuery;

use heartwood_kernel::utils::pluralise::pluralise;

use crate::errors::{PoolCode, Refusal};
use crate::options::Options;
use crate::sender_state::SenderState;
use crate::service::PoolContext;

/// One sender's pending transactions.
#[derive(Debug, Clone)]
pub struct SenderMempool {
    address: Address,
    transactions: Vec<Arc<Transaction>>,
    undo: Vec<TxUndo>,
    state: SenderState,
}

impl SenderMempool {
    /// An empty sender mempool for `address`, whose view starts as a copy of `wallets`.
    pub fn new(address: Address, wallets: &WalletRepository) -> SenderMempool {
        SenderMempool {
            address,
            transactions: Vec::new(),
            undo: Vec::new(),
            state: SenderState::new(wallets),
        }
    }

    /// The sender's address.
    pub fn address(&self) -> &Address {
        &self.address
    }

    /// The number of transactions.
    pub fn len(&self) -> usize {
        self.transactions.len()
    }

    /// Whether the sender mempool holds no transaction.
    pub fn is_empty(&self) -> bool {
        self.transactions.is_empty()
    }

    /// The transactions, the oldest first.
    pub fn from_earliest(
        &self,
    ) -> impl DoubleEndedIterator<Item = &Transaction> + ExactSizeIterator + Clone {
        self.transactions.iter().map(Arc::as_ref)
    }

    /// The transactions, the newest first.
    pub fn from_latest(
        &self,
    ) -> impl DoubleEndedIterator<Item = &Transaction> + ExactSizeIterator + Clone {
        self.from_earliest().rev()
    }

    /// The transactions as the pool holds them, shared, the oldest first.
    pub fn shared(&self) -> Iter<'_, Arc<Transaction>> {
        self.transactions.iter()
    }

    /// The sender's state: its view of the wallets.
    pub fn state(&self) -> &SenderState {
        &self.state
    }

    /// The sender's wallet in its view.
    pub fn wallet(&self) -> Option<&Wallet> {
        self.state.wallets().find_by_address(&self.address)
    }

    /// Add `transaction`: refused with `ERR_EXCEEDS_MAX_COUNT` when the sender already has
    /// `maxTransactionsPerSender` transactions and is not an allowed sender, then checked and
    /// applied on the sender's view ([`SenderState::apply`]), with `pending` as the pool's pending
    /// transactions for the pool-entry checks.
    pub fn add_transaction<P: PoolQuery>(
        &mut self,
        ctx: &PoolContext<'_>,
        options: &Options,
        pending: &P,
        transaction: Transaction,
    ) -> Result<(), Refusal> {
        self.check_count(ctx, options, &transaction)?;
        let undo = self.state.apply(ctx, options, pending, &transaction)?;
        self.push(transaction, undo);
        Ok(())
    }

    /// The per-sender limit: `ERR_EXCEEDS_MAX_COUNT` when the sender already has
    /// `maxTransactionsPerSender` transactions and is not an allowed sender.
    pub(crate) fn check_count(
        &self,
        ctx: &PoolContext<'_>,
        options: &Options,
        transaction: &Transaction,
    ) -> Result<(), Refusal> {
        let limit = options.max_transactions_per_sender;
        if self.transactions.len() < limit || options.allowed_senders.contains(&self.address) {
            return Ok(());
        }
        let count = i128::try_from(limit).unwrap_or(i128::MAX);
        Err(Refusal::new(
            PoolCode::ExceedsMaxCount,
            format!(
                "{} exceeds the sender's {} count limit",
                transaction.display(ctx.network.pub_key_hash),
                pluralise("transaction", count, true)
            ),
        ))
    }

    /// The view, taken out so that its checks can read the rest of the pool; an empty view holds
    /// its place until [`SenderMempool::put_state`].
    pub(crate) fn take_state(&mut self) -> SenderState {
        let network = self.state.wallets().network();
        std::mem::replace(&mut self.state, SenderState::empty(network))
    }

    /// Put back the view that [`SenderMempool::take_state`] took out.
    pub(crate) fn put_state(&mut self, state: SenderState) {
        self.state = state;
    }

    /// Append an applied transaction and its undo record.
    pub(crate) fn push(&mut self, transaction: Transaction, undo: TxUndo) {
        self.transactions.push(Arc::new(transaction));
        self.undo.push(undo);
    }

    /// Remove the transaction `id` and every later one, reverting them on the view newest first;
    /// when a revert fails, every transaction of the sender is removed and the view is marked
    /// inconsistent. Returns the removed transactions, newest first.
    pub fn remove_transaction(
        &mut self,
        ctx: &PoolContext<'_>,
        id: &TransactionId,
    ) -> Vec<Arc<Transaction>> {
        let Some(index) = self.position(id) else {
            return Vec::new();
        };
        let mut removed = self.transactions.split_off(index);
        let mut undo = self.undo.split_off(index);
        removed.reverse();
        undo.reverse();
        for (transaction, undo) in removed.iter().zip(&undo) {
            if self.state.revert(ctx, transaction, undo).is_err() {
                // The reference then empties the sender mempool without reverting the others.
                let mut others = std::mem::take(&mut self.transactions);
                self.undo.clear();
                others.reverse();
                removed.extend(others);
                break;
            }
        }
        removed
    }

    /// Remove the forged transaction `id` and every earlier one, without a revert (the state
    /// has applied them). Returns the removed transactions, oldest first; none when `id` is not
    /// here.
    pub fn remove_forged_transaction(&mut self, id: &TransactionId) -> Vec<Arc<Transaction>> {
        let Some(index) = self.position(id) else {
            return Vec::new();
        };
        let later = self.transactions.split_off(index.saturating_add(1));
        let later_undo = self.undo.split_off(index.saturating_add(1));
        self.undo = later_undo;
        std::mem::replace(&mut self.transactions, later)
    }

    /// The position of the transaction `id`.
    fn position(&self, id: &TransactionId) -> Option<usize> {
        self.transactions
            .iter()
            .position(|transaction| transaction.id() == id)
    }
}

#[cfg(test)]
mod tests {
    use heartwood_crypto::managers::{Milestones, Network};
    use heartwood_crypto::transactions::TransactionBuilder;
    use heartwood_crypto::transactions::types::TransferItem;
    use heartwood_crypto::{Aux, KeyPair};

    use super::*;
    use crate::online::DelegateVersions;

    #[test]
    fn a_failed_revert_empties_the_sender_mempool() {
        let network = Network {
            name: "devnet".into(),
            message_prefix: String::new(),
            address_character: None,
            bip32_public: 0,
            bip32_private: 0,
            pub_key_hash: 90,
            nethash: "00".repeat(32),
            wif: 0,
            slip44: 1,
            token: String::new(),
            symbol: "dRT".into(),
            explorer: String::new(),
        };
        let milestones = Milestones::load(
            r#"[{"height":1,"activeDelegates":1,"blockTime":8,"block":{"version":0},
            "epoch":"2026-01-01T00:00:00.000Z","burn":{"feeBasisPoints":0,"txAmount":0},
            "transfer":{}}]"#,
            &network,
        )
        .unwrap();
        let keys = KeyPair::from_passphrase("revert sender").unwrap();
        let address = Address::from_public_key(keys.public_key(), 90);
        let mut wallets = WalletRepository::new(90);
        wallets.get_or_create(&address).credit(1_000_000).unwrap();
        let online = DelegateVersions::new();
        let ctx = PoolContext {
            wallets: &wallets,
            tip: 10,
            milestones: &milestones,
            network: &network,
            online: &online,
        };
        let options = Options::defaults(&|_| None, std::path::Path::new("unused"), 1);
        let tx = |nonce| {
            TransactionBuilder::transfer(vec![TransferItem {
                amount: 1,
                recipient: address,
            }])
            .nonce(nonce)
            .fee(10)
            .build(&keys, Aux::fixed([1; 32]), ctx.params())
            .unwrap()
        };
        let empty = crate::mempool::Mempool::new();
        let pending = crate::query::Query::new(&empty);
        let mut sender = SenderMempool::new(address, &wallets);
        sender
            .add_transaction(&ctx, &options, &pending, tx(1))
            .unwrap();
        sender
            .add_transaction(&ctx, &options, &pending, tx(2))
            .unwrap();
        // A transaction that was never applied: its revert fails.
        sender.push(tx(3), TxUndo::None);
        let removed = sender.remove_transaction(&ctx, tx(2).id());
        let nonces: Vec<u64> = removed.iter().map(|tx| tx.data().nonce).collect();
        assert_eq!(nonces, [3, 2, 1]);
        assert!(sender.is_empty());
        assert!(sender.state().is_corrupt());
        // Not here: nothing removed.
        assert!(sender.remove_transaction(&ctx, tx(1).id()).is_empty());
        assert!(sender.remove_forged_transaction(tx(1).id()).is_empty());
    }
}
