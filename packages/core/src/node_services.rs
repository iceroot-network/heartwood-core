//! The node as the peer-to-peer server reads it and asks it for work: [`NodeServices`] over the
//! node thread's handle, its snapshots, the chain state, the store's read-only connections of the
//! server and the blockchain service ([`RelayServices`]), and [`PeerSide`] over the network
//! monitor, its repository and its acceptance ([`MonitorSide`]).
//!
//! The round and the slots are read as the reference's internal controller reads them: the tip
//! and the wallets of the state the node last published, the round's delegates in forging order
//! as `getActiveDelegates` gives them (the state's forging list when its round is the one asked,
//! else the stored round's delegates shuffled for the round), the tip's block data with its
//! forger's name, and the block time of the tip plus one. `allDelegates` lists every registered
//! delegate in name order, where the reference lists them in the order its username index was
//! filled, and each wallet is the API's shape, where the reference writes its wallet data (for
//! the round's delegates, the copies made at the round's start); the forger reads only each
//! wallet's key and name.

use std::net::IpAddr;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use futures_util::future::BoxFuture;
use heartwood_blockchain::chain_state::lock;
use heartwood_blockchain::state_machine::MachineState;
use heartwood_blockchain::{
    Blockchain, Channel, IncomingBlock, MachineStatus, Network, NodeHandle,
};
use heartwood_crypto::crypto::slots;
use heartwood_crypto::{Block, BlockHeader, BlockId, Milestones, PublicKeyBytes, TransactionId};
use heartwood_database::{BlockRow, Readers};
use heartwood_kernel::bus::EventBus;
use heartwood_kernel::{
    ChainLink, Clock, Event, RoundInfo, calculate_forging_info, calculate_round,
};
use heartwood_p2p::errors::LookupError;
use heartwood_p2p::monitor::CleanseOptions;
use heartwood_p2p::node_services::{Collation, ForgerEvent, NodeUnavailable, QueueState};
use heartwood_p2p::scoring::{self, Violation};
use heartwood_p2p::seams::{PeerAcceptor, PeerClient, PeerRules};
use heartwood_p2p::{
    AcceptOptions, BlockLookup, ChainView, DownloadedBlock, NetworkMonitor, NodeServices, Peer,
    PeerRepository, PeerSide, SharedPeer,
};
use heartwood_p2p_wire::codecs::blocks::BlockListing;
use heartwood_p2p_wire::internal::{CurrentRound, DelegateWallet, LastBlockData};
use heartwood_p2p_wire::replies::PeerAddress;
use heartwood_p2p_wire::status::PeerConfig;
use heartwood_pool::{Entry, PoolView};
use heartwood_state::StateView;
use heartwood_state::round_state::shuffle_delegates;
use heartwood_state::wallets::Wallet;
use serde_json::Value;
use tokio::runtime::Handle;
use tokio::sync::watch;
use tokio::time::timeout;

use crate::views::{Links, NodeView, header};

/// How often the queue's running state is looked at for AwaitBlock.
const QUEUE_POLL: Duration = Duration::from_millis(20);

/// The node's services for the server, over the blockchain service of network `N`.
pub struct RelayServices<N> {
    /// The node thread.
    pub handle: NodeHandle,
    /// The node as the client reads it.
    pub view: Arc<NodeView>,
    /// The server's own read-only connections to the store.
    pub history: Arc<Readers>,
    /// The monitor and the machine, the attestation session, the boot and the first sync.
    pub links: Arc<Links>,
    /// The blockchain service.
    pub blockchain: Arc<Blockchain<N>>,
    /// The machine's `newBlock` state and the queue's running state, kept current.
    pub queue: watch::Receiver<QueueState>,
    /// The node's peer configuration, which its status carries.
    pub peer_config: PeerConfig,
    /// The node's event bus, where the forger's events go.
    pub bus: EventBus,
    /// The wall clock.
    pub clock: Arc<dyn Clock>,
    /// Where each network state served is recorded (feature `difftest`).
    #[cfg(feature = "difftest")]
    pub record: Option<crate::harness::NetworkStateRecord>,
    /// Where each public request's AwaitBlock step is recorded (feature `difftest`).
    #[cfg(feature = "difftest")]
    pub await_record: Option<crate::harness::AwaitRecordFile>,
    /// Where each step of a status's attestation is recorded (feature `difftest`).
    #[cfg(feature = "difftest")]
    pub attestation_record: Option<crate::harness::AttestationRecordFile>,
}

/// The machine's `newBlock` state and the queue's running state of `blockchain`, kept current
/// (the queue is looked at every 20 ms) until the receiver is gone.
pub fn queue_state<N: Network>(blockchain: &Arc<Blockchain<N>>) -> watch::Receiver<QueueState> {
    let (blockchain, mut status) = (Arc::clone(blockchain), blockchain.status());
    let now = |status: &watch::Receiver<MachineStatus>, blockchain: &Blockchain<N>| QueueState {
        new_block: status.borrow().state == MachineState::NewBlock,
        running: blockchain.queue_running(),
    };
    let (sender, receiver) = watch::channel(now(&status, &blockchain));
    tokio::spawn(async move {
        while !sender.is_closed()
            && !matches!(timeout(QUEUE_POLL, status.changed()).await, Ok(Err(_)))
        {
            let state = now(&status, &blockchain);
            sender.send_if_modified(|was| std::mem::replace(was, state) != state);
        }
    });
    receiver
}

fn lookup<T, E: std::fmt::Display>(result: Result<T, E>) -> Result<T, LookupError> {
    result.map_err(|e| LookupError(e.to_string()))
}

impl<N> RelayServices<N> {
    fn state(&self) -> Arc<StateView> {
        Arc::clone(&self.handle.state().borrow())
    }
}

/// The block time at the tip `tip` plus one, and the network's epoch, in milliseconds.
fn slot_times(milestones: &Milestones, tip: u32) -> (u64, i64) {
    let params = milestones.at(tip.saturating_add(1));
    let epoch = slots::epoch_unix_ms(milestones.at(1).epoch()).unwrap_or(0);
    (params.block_time(), epoch)
}

/// The delegates of `round` in forging order, as `getActiveDelegates` gives them: the forging
/// list of the published `state` when its round is `round`, else the stored delegates that
/// `round_keys` gives (rank order) shuffled for the round.
pub fn forging_keys(
    state: &StateView,
    round: RoundInfo,
    round_keys: impl FnOnce(RoundInfo) -> Result<Vec<PublicKeyBytes>, LookupError>,
) -> Result<Vec<PublicKeyBytes>, LookupError> {
    if state.round.info().round == round.round {
        let forging = state.round.forging_delegates().iter();
        return Ok(forging.map(|delegate| delegate.public_key).collect());
    }
    Ok(shuffle_delegates(round.round, &round_keys(round)?))
}

/// getCurrentRound's answer at `now_ms` (milliseconds since the Unix epoch), as the internal
/// controller gives it: over the tip, the wallets and the round of the published `state`, with
/// `pool`'s view of the wallets, and the round of the tip plus one in forging order
/// ([`forging_keys`], with `round_keys`).
pub fn current_round(
    state: &StateView,
    pool: &PoolView,
    milestones: &Milestones,
    now_ms: i64,
    round_keys: impl FnOnce(RoundInfo) -> Result<Vec<PublicKeyBytes>, LookupError>,
) -> Result<CurrentRound, LookupError> {
    let tip = state.height();
    let (height, (_, epoch)) = (tip.saturating_add(1), slot_times(milestones, tip));
    let round = calculate_round(height, milestones);
    let wallet = |wallet: &&Wallet| {
        let fields = delegate_wallet(wallet, state, pool);
        lookup(DelegateWallet::new(
            fields.as_object().cloned().unwrap_or_default(),
        ))
    };
    let registered: Vec<&Wallet> = state.wallets.delegates().collect();
    let keys = forging_keys(state, round, round_keys)?;
    let active: Vec<&Wallet> = (keys.iter())
        .filter_map(|key| state.wallets.find_by_public_key(key))
        .collect();
    let all_delegates = registered.iter().map(wallet).collect::<Result<_, _>>()?;
    let delegates: Vec<_> = active.iter().map(wallet).collect::<Result<_, _>>()?;
    let info = calculate_forging_info(slots::epoch_time(now_ms, epoch), height, milestones);
    let index = |index: i64| usize::try_from(index).ok();
    let at = |i: i64| index(i).and_then(|i| delegates.get(i)).cloned();
    let current = index(info.current_forger).and_then(|i| active.get(i));
    let reward = match current.and_then(|wallet| wallet.delegate()) {
        Some(forger) => {
            let reward = state
                .round
                .reward_for_block_in_round(height, forger, milestones);
            lookup(reward)?.reward
        }
        None => milestones.at(tip).reward(),
    };
    let mut last_block = state.last_block.to_json(milestones);
    let fields = last_block.as_object_mut().map(std::mem::take);
    let mut fields = fields.unwrap_or_default();
    fields.shift_remove("transactions");
    // The name the reference gives an applied block's data when it enqueues it.
    let generator = &state.last_block.header().generator_public_key;
    let forger = state.wallets.find_by_public_key(generator);
    if let Some(delegate) = forger.and_then(Wallet::delegate).filter(|_| tip > 1) {
        fields.insert("username".into(), delegate.username().into());
    }
    Ok(CurrentRound {
        can_forge: info.can_forge,
        current: round.round,
        current_forger: at(info.current_forger),
        next_forger: at(info.next_forger),
        last_block: lookup(LastBlockData::new(fields))?,
        reward: reward.to_string(),
        timestamp: info.block_timestamp,
        all_delegates,
        delegates,
    })
}

/// getSlotNumber's answer: the slot of `timestamp` (seconds after the epoch), or of `now_ms`
/// when there is none, at the block time of the tip `tip` plus one.
pub fn slot_number(milestones: &Milestones, tip: u32, timestamp: Option<i64>, now_ms: i64) -> i64 {
    let (block_time, epoch) = slot_times(milestones, tip);
    let timestamp = timestamp.unwrap_or_else(|| slots::epoch_time(now_ms, epoch));
    slots::slot_number(timestamp, block_time)
}

/// A delegate's wallet as the round lists it: `{...wallet.getData(), delegate}`.
fn delegate_wallet(wallet: &Wallet, state: &StateView, pool: &heartwood_pool::PoolView) -> Value {
    let mut data = heartwood_api::resources::wallet(wallet, state, pool);
    let delegate = data.pointer("/attributes/delegate").cloned();
    if let Value::Object(fields) = &mut data {
        fields.shift_remove("votingFor");
        fields.insert("delegate".into(), delegate.unwrap_or(Value::Null));
    }
    data
}

/// A stored block's row as a getBlocks listing gives it.
fn listing(row: BlockRow, transactions: Option<Vec<Vec<u8>>>) -> BlockListing {
    BlockListing {
        id: row.id.to_hex(),
        version: row.version,
        timestamp: row.timestamp,
        previous_block: row.previous_block.map(|id| id.to_hex()),
        height: row.height,
        number_of_transactions: row.number_of_transactions,
        total_amount: row.total_amount.into(),
        total_fee: row.total_fee.into(),
        reward: row.reward.into(),
        payload_length: row.payload_length,
        payload_hash: row.payload_hash,
        generator_public_key: row.generator_public_key,
        block_signature: row.block_signature,
        transactions: transactions.unwrap_or_default(),
    }
}

impl<N: Network> NodeServices for RelayServices<N> {
    fn is_booted(&self) -> bool {
        self.links.booted.load(Ordering::Acquire)
    }

    fn first_synced_ms(&self) -> Option<i64> {
        self.links.first_synced.get().copied()
    }

    fn queue_state(&self) -> watch::Receiver<QueueState> {
        self.queue.clone()
    }

    fn chain(&self) -> Arc<dyn ChainView> {
        self.view.clone()
    }

    fn last_downloaded_block(&self) -> ChainLink {
        let chain = lock(&self.view.chain);
        let tip = || chain.last_block().map(|block| ChainLink::of(block));
        let link = chain.last_downloaded.or_else(tip);
        drop(chain);
        link.unwrap_or_else(|| ChainLink::of(&self.state().last_block))
    }

    fn ping_block(&self, header: &BlockHeader) -> bool {
        let id = heartwood_crypto::blocks::serialiser::block_id(header);
        self.blockchain.ping_block(&id, header.height)
    }

    fn handle_incoming_block(&self, block: Block, from_forger: bool, ip: IpAddr) {
        let (block, service) = (
            DownloadedBlock::from_block(&block, ip),
            self.blockchain.clone(),
        );
        let (name, fire) = (None, true);
        let incoming = IncomingBlock {
            block,
            from_forger,
            name,
        };
        let mut work = Box::pin(async move { service.handle_incoming_block(incoming, fire).await });
        // The reference enqueues the block before it answers; the service's work has no wait.
        if work.as_mut().poll(&mut Context::from_waker(Waker::noop())) == Poll::Pending
            && let Ok(runtime) = Handle::try_current()
        {
            runtime.spawn(work);
        }
    }

    fn delegate_rank(&self, username: &str) -> Option<u32> {
        self.state()
            .wallets
            .find_by_username(username)?
            .delegate()?
            .rank
    }

    fn already_forged(&self, height: u32, username: &str) -> Result<bool, LookupError> {
        let (state, milestones) = (self.state(), &self.view.milestones);
        let delegate = state
            .wallets
            .find_by_username(username)
            .and_then(Wallet::delegate);
        let delegate = delegate.ok_or_else(|| LookupError(format!("no delegate {username}")))?;
        let reward = state
            .round
            .reward_for_block_in_round(height, delegate, milestones);
        Ok(lookup(reward)?.already_forged)
    }

    fn active_keys_next_round(&self) -> Result<Vec<PublicKeyBytes>, LookupError> {
        let state = self.state();
        let round = calculate_round(state.height().saturating_add(1), &self.view.milestones);
        forging_keys(&state, round, |round| self.view.round_delegate_keys(round))
    }

    fn blocks_for_download(
        &self,
        from: u32,
        limit: u32,
        headers_only: bool,
    ) -> Result<Vec<BlockListing>, LookupError> {
        let reader = self.history.get();
        let blocks = lookup(reader.blocks_for_download(from, limit, headers_only))?;
        let listed = blocks.into_iter();
        Ok(listed.map(|b| listing(b.row, b.transactions)).collect())
    }

    fn common_blocks(
        &self,
        ids: &[BlockId],
        asked: usize,
    ) -> Result<Vec<BlockHeader>, LookupError> {
        let memory: Vec<BlockHeader> = lock(&self.view.chain)
            .last_blocks
            .iter()
            .filter(|block| ids.contains(block.id()))
            .map(|block| block.header().clone())
            .collect();
        if memory.len() >= asked {
            return Ok(memory);
        }
        let rows = lookup(self.history.get().blocks_by_ids(ids))?;
        rows.iter().map(header).collect()
    }

    fn pool_size(&self) -> usize {
        self.handle.pool().borrow().len()
    }

    fn collate(
        &self,
        validate: bool,
        exclude: Vec<TransactionId>,
    ) -> BoxFuture<'_, Result<Collation, NodeUnavailable>> {
        Box::pin(async move {
            let collation = self.handle.collate(validate, exclude).await;
            collation.map_err(|_| NodeUnavailable)
        })
    }

    fn post_transactions(
        &self,
        transactions: Vec<Vec<u8>>,
    ) -> BoxFuture<'_, Result<Vec<TransactionId>, NodeUnavailable>> {
        Box::pin(async move {
            let entries = transactions.into_iter().map(Entry::Bytes).collect();
            let result = self.handle.pool_entries(Channel::P2p, entries).await;
            let result = result.map_err(|_| NodeUnavailable)?;
            let accepted = result.accept.iter();
            Ok(accepted
                .filter_map(|id| TransactionId::from_hex(id).ok())
                .collect())
        })
    }

    fn current_round(&self, now_ms: i64) -> Result<CurrentRound, LookupError> {
        let (state, pool) = (self.state(), Arc::clone(&self.handle.pool().borrow()));
        let keys = |round| self.view.round_delegate_keys(round);
        current_round(&state, &pool, &self.view.milestones, now_ms, keys)
    }

    fn slot_number(&self, timestamp: Option<i64>, now_ms: i64) -> i64 {
        let tip = self.state().height();
        slot_number(&self.view.milestones, tip, timestamp, now_ms)
    }

    fn force_wakeup(&self) {
        self.blockchain.force_wakeup();
    }

    fn forger_event(&self, event: ForgerEvent) {
        self.bus.emit(match event {
            ForgerEvent::Started(_) => Event::ForgerStarted,
            ForgerEvent::Failed(_) => Event::ForgerFailed,
            ForgerEvent::BlockForged(_) => Event::BlockForged,
            ForgerEvent::TransactionForged(_) => Event::TransactionForged,
        });
    }

    fn peer_config(&self) -> PeerConfig {
        self.peer_config.clone()
    }

    #[cfg(feature = "difftest")]
    fn network_state_served(
        &self,
        caller: IpAddr,
        state: &heartwood_p2p_wire::internal::NetworkState,
    ) {
        if let Some(record) = &self.record {
            record.write(self.clock.now_ms(), caller, state);
        }
    }

    #[cfg(feature = "difftest")]
    fn await_block_step(&self, record: &heartwood_p2p::node_services::AwaitRecord) {
        if let Some(file) = &self.await_record {
            file.write(record);
        }
    }

    #[cfg(feature = "difftest")]
    fn attestation_step(&self, record: &heartwood_p2p::node_services::AttestationRecord) {
        if let Some(file) = &self.attestation_record {
            file.write(record);
        }
    }
}

/// The network monitor, its repository and its acceptance, as the server's peer side.
pub struct MonitorSide<C, A, R> {
    /// The network monitor.
    pub monitor: Arc<NetworkMonitor<C, A, R>>,
    /// The peers.
    pub repository: Arc<PeerRepository>,
    /// The acceptance of new peers.
    pub acceptor: Arc<A>,
    /// The wall clock.
    pub clock: Arc<dyn Clock>,
    /// The runtime of the acceptances.
    pub runtime: Handle,
}

impl<C, A, R> PeerSide for MonitorSide<C, A, R>
where
    C: PeerClient + 'static,
    A: PeerAcceptor + 'static,
    R: PeerRules + 'static,
{
    fn accept(&self, ip: IpAddr, port: u16) {
        let (acceptor, ip) = (Arc::clone(&self.acceptor), ip.to_string());
        let accepted = async move {
            let options = AcceptOptions::default();
            acceptor
                .validate_and_accept(PeerAddress { ip, port }, options)
                .await;
        };
        self.runtime.spawn(accepted);
    }

    fn is_peer(&self, ip: IpAddr) -> bool {
        self.repository.has(ip)
    }

    fn record_version(&self, ip: IpAddr, version: &str) {
        let peer = self.repository.get(ip);
        peer.inspect(|peer| peer.with(|peer| peer.version = Some(version.to_owned())));
    }

    fn mark_stale(&self, ip: IpAddr) {
        let peer = self.repository.get(ip);
        peer.inspect(|peer| peer.with(|peer| peer.stale = true));
    }

    fn record_violation(&self, ip: IpAddr, violation: Violation) {
        let (repository, now) = (&self.repository, self.clock.now_ms());
        match (repository.get(ip)).or_else(|| repository.get_pending(ip)) {
            Some(peer) => scoring::record(&peer, violation, now),
            // An address that is no peer yet keeps the violation in the repository, so it is not
            // accepted while the violation is in its window.
            None => scoring::record_address(repository, ip, violation, now),
        }
    }

    fn peers(&self) -> Vec<Peer> {
        self.repository
            .peers()
            .iter()
            .map(SharedPeer::snapshot)
            .collect()
    }

    fn remove_keys(&self, keys: &[PublicKeyBytes]) {
        for peer in self.repository.peers() {
            peer.with(|peer| peer.public_keys.retain(|key| !keys.contains(key)));
        }
    }

    fn is_cold_start(&self) -> bool {
        self.monitor.is_cold_start()
    }

    fn complete_cold_start(&self) {
        self.monitor.complete_cold_start();
    }

    fn cleanse(&self, options: CleanseOptions) -> BoxFuture<'_, ()> {
        Box::pin(self.monitor.cleanse_peers(options))
    }
}
