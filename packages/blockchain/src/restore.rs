//! A node's start: the state restored from the store.
//!
//! 1. An empty store is a first start: the state after the configured genesis block, written
//!    with the chain's identity in one transaction.
//! 2. Otherwise the boot checks run on every start: the store names the configured nethash and
//!    genesis block, the configured genesis block passes its checks and is the stored one, and
//!    the store's own checks pass ([`heartwood_database::Database::integrity`]). A failed check
//!    refuses the start and names the check; nothing is rewound.
//! 3. The applied milestone entries must be unchanged ([`crate::milestones`]).
//! 4. With [`RestoreMode::Auto`] and a state row of the current format, the state is read back
//!    from the state tables and checked ([`heartwood_state::State::restore`]). A missing,
//!    older-format or inconsistent state is no error: the start replays instead.
//! 5. The replay: the state after the genesis block, then every stored block applied again
//!    through the one apply path; then the state tables are written again. A replay that would
//!    take a balance below zero refuses the start with the reference's message.
//! 6. After either, a store that holds no missed-slot row gets the rows of the slots missed
//!    before block 2, as the reference's start writes them (its productivity backfill).
//! 7. Every start also loads the node-local online record ([`crate::online`]).

use heartwood_crypto::blocks::factory;
use heartwood_crypto::{Block, Milestones};
use heartwood_database::{ChainMeta, Database, MilestoneEntryRow, SCHEMA_VERSION};
use heartwood_kernel::config::crypto::CryptoFiles;
use heartwood_pool::DelegateVersions;
use heartwood_state::errors::{ApplyError, GenesisError, WalletError};
use heartwood_state::genesis;
use heartwood_state::persist::{RestoreError, STATE_FORMAT, StateRecord, UndoRecord, WalletRecord};
use heartwood_state::round_state::MissedBlock;
use heartwood_state::{Rejection, State, TransactionHandlers, Wallet, WalletRepository};

use crate::commit::ChainStore;
use crate::errors::{StartError, StoreError};
use crate::milestones::{entry_digests, first_changed};
use crate::rows::stored_block;

/// How the state is to be restored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestoreMode {
    /// From the state tables when they hold a consistent state, else by a replay.
    Auto,
    /// By a replay from the genesis block, as a `state clear` asks.
    Replay,
}

/// Why a start replayed the chain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplayReason {
    /// [`RestoreMode::Replay`] asked for it.
    Requested,
    /// The store has no state row.
    NoState,
    /// The state row's format is not the current one.
    Format {
        /// The stored format.
        found: u32,
    },
    /// The stored state failed a check.
    Inconsistent(RestoreError),
}

/// How the state was restored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestorePath {
    /// A first start, from the genesis block.
    FirstStart,
    /// From the state tables.
    Table,
    /// By a replay from the genesis block.
    Replay(ReplayReason),
}

/// A restored state, how it was restored, and the node-local online record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Restored {
    /// The state at the stored tip.
    pub state: State,
    /// How it was restored.
    pub path: RestorePath,
    /// The stored online record, for the pool (empty at a first start).
    pub online: DelegateVersions,
}

/// Restore the state of `store` under the configuration `files`, with `handlers` for the genesis
/// block and a replay, and load the online record (see the module documentation for the steps).
pub fn restore<H: TransactionHandlers>(
    store: &mut ChainStore,
    files: &CryptoFiles,
    handlers: &H,
    mode: RestoreMode,
) -> Result<Restored, StartError<H::Error>> {
    let (state, path) = restore_state(store, files, handlers, mode)?;
    let online = store.online()?;
    Ok(Restored {
        state,
        path,
        online,
    })
}

/// The state of `store` at its tip, and how it was restored.
fn restore_state<H: TransactionHandlers>(
    store: &mut ChainStore,
    files: &CryptoFiles,
    handlers: &H,
    mode: RestoreMode,
) -> Result<(State, RestorePath), StartError<H::Error>> {
    let milestones = &files.milestones;
    let genesis = factory::from_json(&files.genesis, milestones).map_err(|error| {
        let detail = error.to_string();
        StartError::GenesisDecode { detail }
    })?;
    genesis::validate(&genesis, milestones).map_err(StartError::Genesis)?;
    genesis::issued(&genesis).ok_or(StartError::Genesis(GenesisError::Issuance))?;
    store.entries = entry_digests(&files.milestones_text)?;
    let database = store.database();
    let incomplete = |detail| Err(StartError::Identity { detail });
    let (meta, tip) = match (database.meta()?, database.last_block()?) {
        (None, None) => return first_start(store, files, &genesis, handlers),
        (Some(meta), Some(last)) => (meta, last.row.height),
        (None, Some(_)) => return incomplete("no identity"),
        (Some(_), None) => return incomplete("no block"),
    };
    let meta = boot_checks(database, files, &store.entries, &genesis, meta, tip)?;
    let stored = if tip >= 2 {
        database.blocks(2, tip)?
    } else {
        Vec::new()
    };
    let blocks = (stored.iter().map(|block| stored_block(block, milestones)))
        .collect::<Result<Vec<Block>, StoreError>>()?;
    let backfill = backfill(database, &genesis, &blocks, milestones, handlers)?;
    store.meta = Some(meta);
    let reason = match mode {
        RestoreMode::Replay => ReplayReason::Requested,
        RestoreMode::Auto => match table(store, &genesis, &blocks, milestones)? {
            Ok(state) => {
                store.committed = Some(state.wallets().clone());
                let mut chunk = store.chunk()?;
                chunk.save_missed(&backfill)?;
                chunk.next_round_at_start(&state, milestones)?;
                chunk.commit()?;
                return Ok((state, RestorePath::Table));
            }
            Err(reason) => reason,
        },
    };
    let mut state = State::from_genesis(&genesis, milestones, handlers).map_err(genesis_error)?;
    for block in &blocks {
        let applied = state.apply_block(block, milestones, handlers);
        applied.map_err(|error| replay_error(block.height(), error))?;
    }
    let mut chunk = store.chunk()?;
    chunk.rewrite_state(&state)?;
    chunk.save_missed(&backfill)?;
    chunk.next_round_at_start(&state, milestones)?;
    chunk.commit()?;
    Ok((state, RestorePath::Replay(reason)))
}

/// The missed-slot rows the reference's start writes when its store holds none
/// (`calculateMissedBlocks`): its walk over the stored blocks counts again the slots each block's
/// apply counted, none in such a store, and also the slots between the genesis block and block 2,
/// which no apply counts (the count after a block starts after the genesis block). Those are round
/// 1's slots, with round 1's forging list, and their rows have the height 2.
fn backfill<H: TransactionHandlers>(
    database: &Database,
    genesis: &Block,
    blocks: &[Block],
    milestones: &Milestones,
    handlers: &H,
) -> Result<Vec<MissedBlock>, StartError<H::Error>> {
    let Some(second) = blocks.first() else {
        return Ok(Vec::new());
    };
    if database.has_missed_blocks()? {
        return Ok(Vec::new());
    }
    let first = State::from_genesis(genesis, milestones, handlers).map_err(genesis_error)?;
    let round = first.round();
    Ok(round.missed_slots(genesis.header(), second.header(), milestones))
}

/// The first start: the state after the genesis block, written with the chain's identity.
fn first_start<H: TransactionHandlers>(
    store: &mut ChainStore,
    files: &CryptoFiles,
    genesis: &Block,
    handlers: &H,
) -> Result<(State, RestorePath), StartError<H::Error>> {
    let milestones = &files.milestones;
    let state = State::from_genesis(genesis, milestones, handlers).map_err(genesis_error)?;
    let meta = ChainMeta {
        schema_version: SCHEMA_VERSION,
        nethash: files.network.nethash.clone(),
        genesis_id: *genesis.id(),
        state_format: STATE_FORMAT,
    };
    store.meta = Some(meta.clone());
    let mut chunk = store.chunk()?;
    chunk.first_start(&state, &meta, milestones)?;
    chunk.commit()?;
    Ok((state, RestorePath::FirstStart))
}

/// The checks of every start after the first: the store is the configured chain's, the store's
/// own checks pass, and no applied milestone entry changed. Returns the chain's identity.
fn boot_checks<E>(
    database: &Database,
    files: &CryptoFiles,
    configured: &[MilestoneEntryRow],
    genesis: &Block,
    meta: ChainMeta,
    tip: u32,
) -> Result<ChainMeta, StartError<E>> {
    let other = |what, stored: String, configured: String| StartError::OtherChain {
        what,
        stored,
        configured,
    };
    if meta.nethash != files.network.nethash {
        return Err(other(
            "nethash",
            meta.nethash,
            files.network.nethash.clone(),
        ));
    }
    let first = database
        .blocks(1, 1)?
        .into_iter()
        .next()
        .map(|block| block.row.id);
    for stored in [Some(meta.genesis_id), first] {
        if stored != Some(*genesis.id()) {
            let stored = stored.map_or_else(|| "none".into(), |id| id.to_hex());
            return Err(other("genesis block", stored, genesis.id().to_hex()));
        }
    }
    let report = database.integrity(&files.milestones)?;
    if let Some(problem) = report.problems.into_iter().next() {
        return Err(StartError::Integrity(problem));
    }
    let stored = database.milestone_entries()?;
    if let Some(height) = first_changed(&stored, configured, tip) {
        return Err(StartError::MilestoneChanged { height });
    }
    Ok(meta)
}

/// The state read back from the state tables, or why it is to be replayed instead.
fn table(
    store: &ChainStore,
    genesis: &Block,
    blocks: &[Block],
    milestones: &Milestones,
) -> Result<Result<State, ReplayReason>, StoreError> {
    let database = store.database();
    let Some(row) = database.state_row()? else {
        return Ok(Err(ReplayReason::NoState));
    };
    let found = store
        .meta
        .as_ref()
        .map_or(STATE_FORMAT, |meta| meta.state_format);
    if found != STATE_FORMAT {
        return Ok(Err(ReplayReason::Format { found }));
    }
    let (wallet_rows, undo_rows) = (database.wallets()?, database.undo_records()?);
    let network = milestones.at(1).network();
    let read = || -> Result<State, RestoreError> {
        let state: StateRecord =
            serde_json::from_str(&row.record).map_err(|e| unreadable("state", e))?;
        let mut wallets = Vec::with_capacity(wallet_rows.len());
        for row in &wallet_rows {
            let record: WalletRecord =
                serde_json::from_str(&row.record).map_err(|e| unreadable("wallet", e))?;
            if record.a != row.address {
                return Err(unreadable("wallet", "a row holds another address's record"));
            }
            wallets.push(Wallet::from_record(record, network)?);
        }
        let wallets = WalletRepository::from_wallets(network, wallets)?;
        let mut journal = Vec::with_capacity(blocks.len());
        for row in &undo_rows {
            let record: UndoRecord =
                serde_json::from_str(&row.record).map_err(|e| unreadable("undo", e))?;
            let index = usize::try_from(row.height.saturating_sub(2)).ok();
            let block = index.and_then(|index| blocks.get(index)).cloned();
            let block = block.ok_or_else(|| unreadable("undo", "no block at its height"))?;
            journal.push(record.into_undo(block, network)?);
        }
        let last = blocks.last().unwrap_or(genesis).clone();
        State::restore(genesis.clone(), last, wallets, state, journal, milestones)
    };
    Ok(read().map_err(ReplayReason::Inconsistent))
}

/// The error of a record of `what` that does not read.
fn unreadable(what: &'static str, detail: impl std::fmt::Display) -> RestoreError {
    let detail = detail.to_string();
    RestoreError::Record { what, detail }
}

/// The start's refusal when the genesis block's state cannot be built: the reference's text when
/// a balance would be negative, as the reference's start builds that state with the same check.
fn genesis_error<E>(error: GenesisError<E>) -> StartError<E> {
    match error {
        GenesisError::NegativeBalance { .. } => StartError::NegativeBalance { height: 1 },
        error => StartError::Genesis(error),
    }
}

/// The start's refusal when replaying the block at `height` failed: the reference's text when a
/// balance would go below zero (a failed debit, or the handlers' insufficient balance), else the
/// reference's prefix and the error.
fn replay_error<E: Rejection>(height: u32, error: ApplyError<E>) -> StartError<E> {
    let negative = match &error {
        ApplyError::Wallet(WalletError::NegativeBalance { .. }) => true,
        ApplyError::Transaction { rejected, .. } => {
            matches!(
                rejected.class(),
                "InsufficientBalanceError" | "AssertionError"
            )
        }
        _ => false,
    };
    if negative {
        StartError::NegativeBalance { height }
    } else {
        StartError::Replay { height, error }
    }
}

#[cfg(test)]
mod tests {
    use heartwood_crypto::TransactionId;
    use heartwood_crypto::identities::Address;
    use heartwood_transactions::TransactionError;

    use super::*;

    const NEGATIVE: &str = "State Generation failed: Wallet with negative balance";

    fn refused(rejected: TransactionError) -> StartError<TransactionError> {
        let id = TransactionId::from_array([0; 32]);
        let error = ApplyError::Transaction {
            index: 0,
            id,
            rejected,
        };
        replay_error(7, error)
    }

    #[test]
    fn a_balance_below_zero_refuses_with_the_reference_text() {
        let address = Address::from_bytes([30; 21]);
        let debit = WalletError::NegativeBalance { address };
        for error in [
            replay_error(7, ApplyError::Wallet(debit.clone())),
            refused(TransactionError::from(debit)),
            refused(TransactionError::InsufficientBalance {
                needed: 2,
                balance: 1,
            }),
        ] {
            assert_eq!(error, StartError::NegativeBalance { height: 7 });
            assert_eq!(error.to_string(), NEGATIVE);
        }
        let genesis = genesis_error::<TransactionError>(GenesisError::NegativeBalance { address });
        assert_eq!(genesis.to_string(), NEGATIVE);
        let other = refused(TransactionError::UnexpectedSecondSignature);
        assert!(matches!(other, StartError::Replay { height: 7, .. }));
        assert!(
            other
                .to_string()
                .starts_with("State Generation failed: transaction 0 ")
        );
        let other = genesis_error::<TransactionError>(GenesisError::Issuance);
        assert_eq!(other, StartError::Genesis(GenesisError::Issuance));
    }
}
