//! The pool in the follower: three processors (the API's, the download's and the peer-to-peer
//! server's, each with its own cooldown), the collation for getUnconfirmedTransactions, the
//! forged transactions removed during a job, the online record expired at each round start, then
//! the re-add when a milestone started, then the clean-up, after the job's commit; the re-add
//! with its checks per drain, the removed blocks' transactions re-added in chain order, and the
//! online record saved to the store.
//!
//! Each processor broadcasts the transactions it admitted that pass the broadcast fee rule to
//! every peer, as they were admitted (one that a later entry of the batch evicts included), so a
//! follower forwards what it downloads, as a reference relay does. All of it runs on the node
//! thread, under the rules of the tip plus one.
//!
//! The reference cleans the pool up after each applied block, in a listener that runs while the
//! job goes on, and at a milestone re-adds the pool before that block's clean-up; here the steps
//! run once after the job's commit, at the committed tip, in that order. That removes the same
//! transactions: the age and expiry rules only widen as the tip rises, and the lowest-priority
//! rule finds nothing to remove, since every addition already keeps the pool within
//! `maxTransactionsInPool`.

use heartwood_crypto::{Milestones, Network, Params, Transaction, TransactionId};
use heartwood_kernel::Clock;
use heartwood_p2p::node_services::Collation;
use heartwood_pool::{
    DelegateVersions, Entry, Pool, PoolContext, PoolView, Processor, ProcessorResult, Readded,
    StorageError,
};
use heartwood_state::State;

use crate::commit::ChainStore;
use crate::errors::NodeError;
use crate::processor::PoolSteps;
use crate::work::{Channel, OnlineWork};

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
    /// The pool's context at the tip, with the online record `online`.
    fn context(&self, online: &'a DelegateVersions) -> PoolContext<'a> {
        PoolContext {
            wallets: self.state.wallets(),
            tip: self.state.height(),
            milestones: self.milestones,
            network: self.network,
            online,
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
        let ctx = env.context(&self.online);
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
        let _ = (env, validate, exclude);
        todo!("the collation on the node thread")
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
            self.online.expire(*round);
        }
        if milestone {
            self.pool.readd(&env.context(&self.online), &[])?;
        }
        self.pool.clean_up(&env.context(&self.online))?;
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
        self.pool.readd(&env.context(&self.online), forged)
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
}

impl PoolSteps for PoolTask {
    fn remove_forged(&mut self, transaction: &Transaction, params: &Params) {
        let (sender, pool) = (transaction.data().sender_address(params), &mut self.pool);
        if let Err(error) = pool.remove_forged_transaction(&sender, transaction.id()) {
            self.failure.get_or_insert(error);
        }
    }
}
