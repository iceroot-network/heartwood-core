//! The pool in the follower: three processors (the API's, the download's and the peer-to-peer
//! server's, each with its own cooldown), the collation for getUnconfirmedTransactions, the
//! forged transactions removed during a job, the online record expired at each round start, then
//! the re-add when a milestone started, then the clean-up, after the job's commit; the re-add
//! with its checks per drain, the removed blocks' transactions re-added in chain order, and the
//! online record saved to the store.
//!
//! The delegates on this node ([`OwnKeys`], the forger's attested keys) that are registered
//! delegates take the node's own version in the online record at each round start and at the
//! sync's end, before each expiry, as the reference sets `delegate.version` for the keys of its
//! forger.
//!
//! Each processor broadcasts the transactions it admitted that pass the broadcast fee rule to
//! every peer, as they were admitted (one that a later entry of the batch evicts included), so a
//! follower forwards what it downloads, as a reference relay does. All of it runs on the node
//! thread, under the rules of the tip plus one.
//!
//! Every step that runs the handlers on the pool's views or on a collation's copy leaves the
//! parties of their transactions for the state to re-index ([`PoolTask::take_reindexed`]), as
//! the reference's handlers re-index them in the state's wallets too.
//!
//! The reference cleans the pool up after each applied block, in a listener that runs while the
//! job goes on, and at a milestone re-adds the pool before that block's clean-up; here the steps
//! run once after the job's commit, at the committed tip, in that order. That removes the same
//! transactions: the age and expiry rules only widen as the tip rises, and the lowest-priority
//! rule finds nothing to remove, since every addition already keeps the pool within
//! `maxTransactionsInPool`.

use std::sync::Arc;

use heartwood_crypto::identities::Address;
use heartwood_crypto::{Milestones, Network, Params, Transaction, TransactionId};
use heartwood_kernel::Clock;
use heartwood_p2p::node_services::Collation;
use heartwood_pool::{
    DelegateVersions, Entry, Pool, PoolContext, PoolView, Processor, ProcessorResult, Readded,
    Reindexed, StorageError,
};
use heartwood_state::State;

use crate::commit::ChainStore;
use crate::errors::NodeError;
use crate::processor::PoolSteps;
use crate::work::{Channel, OnlineWork, OwnKeys};

/// Where admitted transactions are broadcast: the network monitor in a node, a recorder in
/// tests.
pub trait TransactionBroadcast: Send + Sync {
    /// Broadcast `transactions` to every peer.
    fn broadcast_transactions(&self, transactions: &[Transaction]);
}

/// What the pool reads: the state at the tip, the rules and the time.
#[derive(Clone, Copy)]
pub struct PoolEnv<'a> {
    /// The state at the tip.
    pub state: &'a State,
    /// The milestones.
    pub milestones: &'a Milestones,
    /// The network.
    pub network: &'a Network,
    /// The clock of the processors' cooldowns.
    pub clock: &'a dyn Clock,
}

impl<'a> PoolEnv<'a> {
    /// The pool's context at the tip, with the online record `online`, leaving the parties of its
    /// handler runs in `reindexed`.
    fn context(&self, online: &'a DelegateVersions, reindexed: &'a Reindexed) -> PoolContext<'a> {
        PoolContext {
            wallets: self.state.wallets(),
            tip: self.state.height(),
            milestones: self.milestones,
            network: self.network,
            online,
            reindexed,
        }
    }
}

/// The pool, its processors and the online record, on the node thread.
pub struct PoolTask {
    pool: Pool,
    online: DelegateVersions,
    api: Processor,
    download: Processor,
    p2p: Processor,
    /// A storage failure of a pool step during a job, which stops the node after it.
    failure: Option<StorageError>,
    /// The delegates on this node and the node's version.
    own: Option<(Arc<dyn OwnKeys>, String)>,
    /// The parties the handler runs left for the state to re-index.
    reindexed: Reindexed,
}

impl PoolTask {
    /// The pool `pool` (opened, not yet re-added) with the restored online record `online`.
    pub fn new(pool: Pool, online: DelegateVersions) -> PoolTask {
        let (api, download, p2p) = (Processor::new(), Processor::new(), Processor::new());
        PoolTask {
            pool,
            online,
            api,
            download,
            p2p,
            failure: None,
            own: None,
            reindexed: Reindexed::default(),
        }
    }

    /// Give the delegates on this node, `keys`, the node's `version` in the online record at each
    /// round start and at the sync's end.
    pub fn set_own_keys(&mut self, keys: Arc<dyn OwnKeys>, version: String) {
        self.own = Some((keys, version));
    }

    /// Set the delegates on this node that are registered delegates in the state of `env` to the
    /// node's version in `round`: before the expiry at a round start or at the sync's end.
    pub fn set_own(&mut self, env: &PoolEnv<'_>, round: u64) {
        if let Some((keys, version)) = &self.own {
            let wallets = env.state.wallets();
            let delegate = |key: &_| {
                wallets
                    .find_by_public_key(key)
                    .is_some_and(|w| w.is_delegate())
            };
            for key in keys.attested().into_iter().filter(delegate) {
                self.online.set(key, round, version.clone());
            }
        }
    }

    /// Process `entries` with `channel`'s processor; the result, and the admitted transactions
    /// to broadcast, in the result's order.
    pub fn process(
        &mut self,
        channel: Channel,
        env: &PoolEnv<'_>,
        entries: Vec<Entry>,
    ) -> Result<(ProcessorResult, Vec<Transaction>), StorageError> {
        let processor = match channel {
            Channel::Api => &mut self.api,
            Channel::Download => &mut self.download,
            Channel::P2p => &mut self.p2p,
        };
        let ctx = env.context(&self.online, &self.reindexed);
        processor.process_to_broadcast(&mut self.pool, &ctx, env.clock, entries)
    }

    /// The collator's candidates at the tip plus one, validated when `validate`, without the
    /// transactions of `exclude`, with the pool's size; the transactions the collation found
    /// invalid are removed from the pool, as the reference's collator removes them.
    pub fn collate(
        &mut self,
        env: &PoolEnv<'_>,
        validate: bool,
        exclude: &[TransactionId],
    ) -> Result<Collation, StorageError> {
        let ctx = env.context(&self.online, &self.reindexed);
        let collated = heartwood_pool::collate(&mut self.pool, &ctx, validate, exclude)?;
        let transactions = collated.candidates.iter();
        Ok(Collation {
            pool_size: self.pool.size(),
            transactions: transactions.map(|t| t.bytes().to_vec()).collect(),
        })
    }

    /// The pool steps after a job's commit: the online record's expiry at the start of each round
    /// in `rounds`, in order, the re-add with its checks when a `milestone` started, then the
    /// clean-up.
    pub fn committed(
        &mut self,
        env: &PoolEnv<'_>,
        rounds: &[u64],
        milestone: bool,
    ) -> Result<(), StorageError> {
        for round in rounds {
            self.set_own(env, *round);
            self.online.expire(*round);
        }
        if milestone {
            self.pool
                .readd(&env.context(&self.online, &self.reindexed), &[])?;
        }
        self.pool
            .clean_up(&env.context(&self.online, &self.reindexed))?;
        Ok(())
    }

    /// Re-add every pool transaction with its checks, first the previously `forged` ones (a
    /// removal's transactions, in chain order): at the queue's drain and after a removal (and
    /// when a milestone starts, [`PoolTask::committed`]).
    pub fn readd(
        &mut self,
        env: &PoolEnv<'_>,
        forged: &[Transaction],
    ) -> Result<Readded, StorageError> {
        self.pool
            .readd(&env.context(&self.online, &self.reindexed), forged)
    }

    /// Change the online record: a status attestation's version, or the expiry at the start of a
    /// round.
    pub fn online(&mut self, change: OnlineWork) {
        match change {
            OnlineWork::Set {
                key,
                round,
                version,
            } => self.online.set(key, round, version),
            OnlineWork::Expire { round } => self.online.expire(round),
        }
    }

    /// Save the online record to `store`.
    pub fn save_online(&self, store: &mut ChainStore) -> Result<(), NodeError> {
        store.save_online(&self.online)?;
        Ok(())
    }

    /// The view of the pool and the online record, to publish.
    pub fn view(&self) -> PoolView {
        self.pool.view(&self.online)
    }

    /// The view of the pool and the online record, sharing with `previous` the transactions the
    /// pool still holds.
    pub fn view_after(&self, previous: &PoolView) -> PoolView {
        self.pool.view_after(&self.online, previous)
    }

    /// `previous` with the online record as it is now: the view after a change of the record
    /// alone, which shares the pool's transactions with `previous`.
    pub fn view_online(&self, previous: &PoolView) -> PoolView {
        let online = self.online.entries();
        PoolView {
            online,
            ..previous.clone()
        }
    }

    /// Close the pool at the node's stop. Every admitted transaction is already committed to the
    /// pool's storage, which the next start re-adds.
    pub fn close(self) {
        drop(self.pool);
    }

    /// The storage failure of a pool step during the last job, if any.
    pub fn take_failure(&mut self) -> Option<StorageError> {
        self.failure.take()
    }

    /// The parties the handler runs of the pool's steps left since the last call, in order, for
    /// the state to re-index (`State::reindex_parties`).
    pub fn take_reindexed(&mut self) -> Vec<Address> {
        self.reindexed.take()
    }
}

impl PoolSteps for PoolTask {
    fn remove_forged(&mut self, transaction: &Transaction, params: &Params) {
        let (sender, pool) = (transaction.data().sender_address(params), &mut self.pool);
        if let Err(error) = pool.remove_forged_transaction(&sender, transaction.id()) {
            self.failure.get_or_insert(error);
        }
    }
}
