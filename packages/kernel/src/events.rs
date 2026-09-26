//! The event names.
//!
//! [`Event`] has one variant per event name of the reference, and [`Event::name`] returns the
//! reference's string, which the API's event topics and the webhooks use. Emitting events, and
//! their payloads, belong to the crates that own the transitions.

/// An event, by the reference's name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Event {
    /// `kernel.booted`: the kernel booted.
    KernelBooted,
    /// `kernel.booting`: the kernel is booting.
    KernelBooting,
    /// `kernel.bootstrapper.bootstrapped`: a bootstrap step finished.
    KernelBootstrapped,
    /// `kernel.bootstrapper.bootstrapping`: a bootstrap step started.
    KernelBootstrapping,
    /// `kernel.serviceProvider.booted`: a component booted.
    ServiceProviderBooted,
    /// `kernel.serviceProvider.disposed`: a component was disposed.
    ServiceProviderDisposed,
    /// `kernel.serviceProvider.registered`: a component was registered.
    ServiceProviderRegistered,
    /// `cache.flushed`: a cache was flushed.
    CacheFlushed,
    /// `cache.forgotten`: a cache entry was forgotten.
    CacheForgotten,
    /// `cache.hit`: a cache lookup hit.
    CacheHit,
    /// `cache.missed`: a cache lookup missed.
    CacheMissed,
    /// `cache.written`: a cache entry was written.
    CacheWritten,
    /// `crypto.milestone.changed`: a new milestone came into force.
    MilestoneChanged,
    /// `blockchain.synced`: the node caught up with the network.
    BlockchainSynced,
    /// `block.applied`: a block was applied.
    BlockApplied,
    /// `block.disregarded`: a received block was disregarded.
    BlockDisregarded,
    /// `block.forged`: a block was forged.
    BlockForged,
    /// `block.received`: a block was received.
    BlockReceived,
    /// `block.reverted`: a block was reverted.
    BlockReverted,
    /// `database.preConnect`: the database is about to connect.
    DatabasePreConnect,
    /// `database.postConnect`: the database connected.
    DatabasePostConnect,
    /// `database.preDisconnect`: the database is about to disconnect.
    DatabasePreDisconnect,
    /// `database.postDisconnect`: the database disconnected.
    DatabasePostDisconnect,
    /// `delegate.productivityChanged`: a delegate's productivity changed.
    DelegateProductivityChanged,
    /// `delegate.registered`: a delegate registered.
    DelegateRegistered,
    /// `delegate.resigned`: a delegate resigned or revoked its resignation.
    DelegateResigned,
    /// `wallet.vote`: a wallet voted.
    WalletVote,
    /// `forger.failed`: the forger failed to forge.
    ForgerFailed,
    /// `forger.missing`: a delegate missed its slot.
    ForgerMissing,
    /// `forger.started`: the forger started.
    ForgerStarted,
    /// `peer.added`: a peer was added.
    PeerAdded,
    /// `peer.disconnect`: a peer is to be disconnected.
    PeerDisconnect,
    /// `peer.disconnected`: a peer was disconnected.
    PeerDisconnected,
    /// `peer.disconnecting`: a peer is being disconnected.
    PeerDisconnecting,
    /// `peer.removed`: a peer was removed.
    PeerRemoved,
    /// `round.applied`: a round started.
    RoundApplied,
    /// `round.created`: a round was created.
    RoundCreated,
    /// `round.missed`: a delegate forged no block in a round.
    RoundMissed,
    /// `state.builder.finished`: the state was built at start.
    StateBuilderFinished,
    /// `state.started`: the state started.
    StateStarted,
    /// `state.starting`: the state is starting.
    StateStarting,
    /// `transaction.pool.added`: a transaction entered the pool.
    TransactionAddedToPool,
    /// `transaction.applied`: a transaction was applied.
    TransactionApplied,
    /// `transaction.expired`: a pool transaction expired.
    TransactionExpired,
    /// `transaction.forged`: a transaction was forged.
    TransactionForged,
    /// `transaction.pool.rejected`: the pool rejected a transaction.
    TransactionRejectedByPool,
    /// `transaction.pool.removed`: a transaction left the pool.
    TransactionRemovedFromPool,
    /// `transaction.reverted`: a transaction was reverted.
    TransactionReverted,
    /// `schedule.blockJob.finished`: a scheduled block job finished.
    ScheduleBlockJobFinished,
    /// `schedule.cronJob.finished`: a scheduled cron job finished.
    ScheduleCronJobFinished,
    /// `queue.finished`: a queue job finished.
    QueueFinished,
    /// `queue.failed`: a queue job failed.
    QueueFailed,
}

impl Event {
    /// Every event, in the order of the reference's event enums.
    pub const ALL: [Event; 52] = [
        Event::KernelBooted,
        Event::KernelBooting,
        Event::KernelBootstrapped,
        Event::KernelBootstrapping,
        Event::ServiceProviderBooted,
        Event::ServiceProviderDisposed,
        Event::ServiceProviderRegistered,
        Event::CacheFlushed,
        Event::CacheForgotten,
        Event::CacheHit,
        Event::CacheMissed,
        Event::CacheWritten,
        Event::MilestoneChanged,
        Event::BlockchainSynced,
        Event::BlockApplied,
        Event::BlockDisregarded,
        Event::BlockForged,
        Event::BlockReceived,
        Event::BlockReverted,
        Event::DatabasePreConnect,
        Event::DatabasePostConnect,
        Event::DatabasePreDisconnect,
        Event::DatabasePostDisconnect,
        Event::DelegateProductivityChanged,
        Event::DelegateRegistered,
        Event::DelegateResigned,
        Event::WalletVote,
        Event::ForgerFailed,
        Event::ForgerMissing,
        Event::ForgerStarted,
        Event::PeerAdded,
        Event::PeerDisconnect,
        Event::PeerDisconnected,
        Event::PeerDisconnecting,
        Event::PeerRemoved,
        Event::RoundApplied,
        Event::RoundCreated,
        Event::RoundMissed,
        Event::StateBuilderFinished,
        Event::StateStarted,
        Event::StateStarting,
        Event::TransactionAddedToPool,
        Event::TransactionApplied,
        Event::TransactionExpired,
        Event::TransactionForged,
        Event::TransactionRejectedByPool,
        Event::TransactionRemovedFromPool,
        Event::TransactionReverted,
        Event::ScheduleBlockJobFinished,
        Event::ScheduleCronJobFinished,
        Event::QueueFinished,
        Event::QueueFailed,
    ];

    /// The reference's name of the event.
    pub const fn name(self) -> &'static str {
        match self {
            Event::KernelBooted => "kernel.booted",
            Event::KernelBooting => "kernel.booting",
            Event::KernelBootstrapped => "kernel.bootstrapper.bootstrapped",
            Event::KernelBootstrapping => "kernel.bootstrapper.bootstrapping",
            Event::ServiceProviderBooted => "kernel.serviceProvider.booted",
            Event::ServiceProviderDisposed => "kernel.serviceProvider.disposed",
            Event::ServiceProviderRegistered => "kernel.serviceProvider.registered",
            Event::CacheFlushed => "cache.flushed",
            Event::CacheForgotten => "cache.forgotten",
            Event::CacheHit => "cache.hit",
            Event::CacheMissed => "cache.missed",
            Event::CacheWritten => "cache.written",
            Event::MilestoneChanged => "crypto.milestone.changed",
            Event::BlockchainSynced => "blockchain.synced",
            Event::BlockApplied => "block.applied",
            Event::BlockDisregarded => "block.disregarded",
            Event::BlockForged => "block.forged",
            Event::BlockReceived => "block.received",
            Event::BlockReverted => "block.reverted",
            Event::DatabasePreConnect => "database.preConnect",
            Event::DatabasePostConnect => "database.postConnect",
            Event::DatabasePreDisconnect => "database.preDisconnect",
            Event::DatabasePostDisconnect => "database.postDisconnect",
            Event::DelegateProductivityChanged => "delegate.productivityChanged",
            Event::DelegateRegistered => "delegate.registered",
            Event::DelegateResigned => "delegate.resigned",
            Event::WalletVote => "wallet.vote",
            Event::ForgerFailed => "forger.failed",
            Event::ForgerMissing => "forger.missing",
            Event::ForgerStarted => "forger.started",
            Event::PeerAdded => "peer.added",
            Event::PeerDisconnect => "peer.disconnect",
            Event::PeerDisconnected => "peer.disconnected",
            Event::PeerDisconnecting => "peer.disconnecting",
            Event::PeerRemoved => "peer.removed",
            Event::RoundApplied => "round.applied",
            Event::RoundCreated => "round.created",
            Event::RoundMissed => "round.missed",
            Event::StateBuilderFinished => "state.builder.finished",
            Event::StateStarted => "state.started",
            Event::StateStarting => "state.starting",
            Event::TransactionAddedToPool => "transaction.pool.added",
            Event::TransactionApplied => "transaction.applied",
            Event::TransactionExpired => "transaction.expired",
            Event::TransactionForged => "transaction.forged",
            Event::TransactionRejectedByPool => "transaction.pool.rejected",
            Event::TransactionRemovedFromPool => "transaction.pool.removed",
            Event::TransactionReverted => "transaction.reverted",
            Event::ScheduleBlockJobFinished => "schedule.blockJob.finished",
            Event::ScheduleCronJobFinished => "schedule.cronJob.finished",
            Event::QueueFinished => "queue.finished",
            Event::QueueFailed => "queue.failed",
        }
    }

    /// The event with the reference's name `name`.
    pub fn from_name(name: &str) -> Option<Event> {
        Event::ALL.into_iter().find(|event| event.name() == name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_unique_and_round_trip() {
        for (i, event) in Event::ALL.iter().enumerate() {
            assert_eq!(Event::from_name(event.name()), Some(*event));
            assert!(
                Event::ALL[..i]
                    .iter()
                    .all(|other| other.name() != event.name())
            );
        }
    }
}
