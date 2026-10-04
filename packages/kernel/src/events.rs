//! The event names.
//!
//! [`Event`] has one variant per event name of the reference that the port keeps, and
//! [`Event::name`] returns the reference's string, which the API's event topics and the webhooks
//! use. The reference's dropped names have no variant: the `cache.*` and `schedule.*` names of
//! services that are not ported, and `kernel.booting`, `kernel.booted`, `database.postConnect`,
//! `peer.disconnecting` and `peer.disconnected`. Emitting events, and their payloads, belong to
//! the crates that own the transitions.

/// An event, by the reference's name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Event {
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
    /// `queue.finished`: a queue job finished.
    QueueFinished,
    /// `queue.failed`: a queue job failed.
    QueueFailed,
}

impl Event {
    /// Every event, in the order of the reference's event enums.
    pub const ALL: [Event; 40] = [
        Event::KernelBootstrapped,
        Event::KernelBootstrapping,
        Event::ServiceProviderBooted,
        Event::ServiceProviderDisposed,
        Event::ServiceProviderRegistered,
        Event::MilestoneChanged,
        Event::BlockchainSynced,
        Event::BlockApplied,
        Event::BlockDisregarded,
        Event::BlockForged,
        Event::BlockReceived,
        Event::BlockReverted,
        Event::DatabasePreConnect,
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
        Event::QueueFinished,
        Event::QueueFailed,
    ];

    /// The reference's name of the event.
    pub const fn name(self) -> &'static str {
        match self {
            Event::KernelBootstrapped => "kernel.bootstrapper.bootstrapped",
            Event::KernelBootstrapping => "kernel.bootstrapper.bootstrapping",
            Event::ServiceProviderBooted => "kernel.serviceProvider.booted",
            Event::ServiceProviderDisposed => "kernel.serviceProvider.disposed",
            Event::ServiceProviderRegistered => "kernel.serviceProvider.registered",
            Event::MilestoneChanged => "crypto.milestone.changed",
            Event::BlockchainSynced => "blockchain.synced",
            Event::BlockApplied => "block.applied",
            Event::BlockDisregarded => "block.disregarded",
            Event::BlockForged => "block.forged",
            Event::BlockReceived => "block.received",
            Event::BlockReverted => "block.reverted",
            Event::DatabasePreConnect => "database.preConnect",
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

    /// The reference's dropped names have no variant.
    #[test]
    fn unused_reference_names_have_no_variant() {
        for name in [
            "kernel.booted",
            "kernel.booting",
            "cache.flushed",
            "cache.forgotten",
            "cache.hit",
            "cache.missed",
            "cache.written",
            "database.postConnect",
            "peer.disconnected",
            "peer.disconnecting",
            "schedule.blockJob.finished",
            "schedule.cronJob.finished",
        ] {
            assert_eq!(Event::from_name(name), None, "{name}");
        }
    }
}
