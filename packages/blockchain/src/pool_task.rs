//! The pool in the follower: two processors (the API's and the download's, each with its own
//! cooldown), the forged transactions removed and the pool cleaned up per applied block, the
//! online record expired at each round start before the clean-up, the re-add with its checks per
//! drain, the removed blocks' transactions re-added in chain order, and the online record saved to
//! the store.
//!
//! Each processor broadcasts the transactions it admitted that pass the broadcast fee rule to
//! every peer, so a follower forwards what it downloads, as a reference relay does. All of it runs
//! on the node thread, under the rules of the tip plus one.

use heartwood_crypto::{Block, Milestones, Network, Params, PublicKeyBytes, Transaction};
use heartwood_kernel::Clock;
use heartwood_pool::{
    DelegateVersions, Entry, Pool, PoolView, Processor, ProcessorResult, Readded, StorageError,
};
use heartwood_state::State;

use crate::commit::ChainStore;
use crate::errors::NodeError;
use crate::processor::PoolSteps;
use crate::work::Channel;

/// Where admitted transactions are broadcast: the network monitor in a node, a recorder in
/// tests.
pub trait TransactionBroadcast: Send + Sync {
    /// Broadcast `transactions` to every peer.
    fn broadcast_transactions(&self, transactions: &[Transaction]);
}

/// What the pool reads besides the state.
#[derive(Clone, Copy)]
pub struct PoolEnv<'a> {
    /// The milestones.
    pub milestones: &'a Milestones,
    /// The network.
    pub network: &'a Network,
    /// The clock of the processors' cooldowns.
    pub clock: &'a dyn Clock,
}

/// The pool, its processors and the online record, on the node thread.
pub struct PoolTask {
    pool: Pool,
    online: DelegateVersions,
    api: Processor,
    download: Processor,
    /// A storage failure of a pool step during a job, which stops the node after it.
    failure: Option<StorageError>,
}

impl PoolTask {
    /// The pool `pool` (opened, not yet re-added) with the restored online record `online`.
    pub fn new(pool: Pool, online: DelegateVersions) -> PoolTask {
        PoolTask {
            pool,
            online,
            api: Processor::new(),
            download: Processor::new(),
            failure: None,
        }
    }

    /// Process `entries` with `channel`'s processor, under the state `state`; the result, and
    /// the admitted transactions to broadcast.
    pub fn process(
        &mut self,
        channel: Channel,
        state: &State,
        env: &PoolEnv<'_>,
        entries: Vec<Entry>,
    ) -> Result<(ProcessorResult, Vec<Transaction>), StorageError> {
        let _ = (
            &mut self.api,
            &mut self.download,
            channel,
            state,
            env,
            entries,
        );
        todo!()
    }

    /// The pool steps after `block` was applied and committed: at a round start (`round_started`)
    /// the online record's expiry, then the clean-up.
    pub fn block_applied(
        &mut self,
        state: &State,
        env: &PoolEnv<'_>,
        block: &Block,
        round_started: Option<u64>,
    ) -> Result<(), StorageError> {
        let _ = (state, env, block, round_started);
        todo!()
    }

    /// The queue's drain: re-add every pool transaction with its checks, then save the online
    /// record to `store`.
    pub fn drain(
        &mut self,
        state: &State,
        env: &PoolEnv<'_>,
        store: &mut ChainStore,
    ) -> Result<Readded, NodeError> {
        let _ = (state, env, store);
        todo!()
    }

    /// Re-add the transactions of removed blocks, in chain order.
    pub fn readd_removed(
        &mut self,
        state: &State,
        env: &PoolEnv<'_>,
        removed: &[Transaction],
    ) -> Result<Readded, StorageError> {
        let _ = (state, env, removed);
        todo!()
    }

    /// Record a status attestation's version.
    pub fn set_version(&mut self, key: PublicKeyBytes, round: u64, version: String) {
        self.online.set(key, round, version);
    }

    /// Expire the online record's entries at the start of `round`.
    pub fn expire(&mut self, round: u64) {
        self.online.expire(round);
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

    /// Close the pool at the node's stop: its storage keeps the admitted transactions for the
    /// next start.
    pub fn close(&mut self) -> Result<(), StorageError> {
        todo!()
    }

    /// The storage failure of a pool step during the last job, if any.
    pub fn take_failure(&mut self) -> Option<StorageError> {
        self.failure.take()
    }
}

impl PoolSteps for PoolTask {
    fn remove_forged(&mut self, transaction: &Transaction, params: &Params) {
        let _ = (transaction, params, &mut self.failure);
        todo!()
    }
}
