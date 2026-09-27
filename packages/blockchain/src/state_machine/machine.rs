//! The states, the events and the transitions of the reference's machine.
//!
//! | State | On entry | Events |
//! |---|---|---|
//! | uninitialised | | `START` initialise, `STOP` stopped |
//! | initialise | `initialise` | `STARTED` syncing, `NETWORKSTART` idle, `ROLLBACK` rollback, `FAILURE` exit, `STOP` stopped |
//! | syncing (parent) | | `SYNCFINISHED` idle, `FORK` fork, `STOP` stopped, `TEST` idle, in every sub-state |
//! | syncing / syncing | `checkLastDownloadedBlockSynced` | `SYNCED` downloadFinished, `NOTSYNCED` downloadBlocks, `PAUSED` downloadPaused, `NETWORKHALTED` end |
//! | syncing / idle | | `DOWNLOADED` downloadBlocks |
//! | syncing / downloadBlocks | `downloadBlocks` | `DOWNLOADED` and `NOBLOCK` syncing, `PROCESSFINISHED` downloadFinished |
//! | syncing / downloadFinished | `downloadFinished` | `PROCESSFINISHED` processFinished |
//! | syncing / downloadPaused | `downloadPaused` | `PROCESSFINISHED` processFinished |
//! | syncing / processFinished | `checkLastBlockSynced` | `SYNCED` end, `NOTSYNCED` downloadBlocks |
//! | syncing / end | `syncingComplete` | |
//! | idle | `checkLater`, `blockchainReady` | `WAKEUP` syncing, `NEWBLOCK` newBlock, `FORK` fork, `STOP` stopped |
//! | newBlock | | `PROCESSFINISHED` idle, `FORK` fork, `STOP` stopped |
//! | fork | `startForkRecovery` | `SUCCESS` syncing, `FAILURE` exit, `STOP` stopped |
//! | rollback | `rollbackDatabase` | `SUCCESS` initialise, `FAILURE` exit, `STOP` stopped |
//! | stopped | `stopped` | |
//! | exit | `exitApp` | |
//!
//! Entering the syncing parent enters syncing / syncing.

use crate::state_machine::actions::Action;

/// A sub-state of the syncing state (`syncWithNetwork`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncState {
    /// `syncing`.
    Syncing,
    /// `idle`.
    Idle,
    /// `downloadBlocks`.
    DownloadBlocks,
    /// `downloadFinished`.
    DownloadFinished,
    /// `downloadPaused`.
    DownloadPaused,
    /// `processFinished`.
    ProcessFinished,
    /// `end`.
    End,
}

/// A state of the machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MachineState {
    /// `uninitialised`.
    Uninitialised,
    /// `initialise`.
    Initialise,
    /// `syncWithNetwork`, in one of its sub-states.
    SyncWithNetwork(SyncState),
    /// `idle`.
    Idle,
    /// `newBlock`.
    NewBlock,
    /// `fork`.
    Fork,
    /// `rollback`.
    Rollback,
    /// `stopped`.
    Stopped,
    /// `exit`.
    Exit,
}

/// An event of the machine, by the reference's names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MachineEvent {
    /// `START`.
    Start,
    /// `STARTED`.
    Started,
    /// `NETWORKSTART`.
    NetworkStart,
    /// `ROLLBACK`.
    Rollback,
    /// `FAILURE`.
    Failure,
    /// `STOP`.
    Stop,
    /// `SYNCED`.
    Synced,
    /// `NOTSYNCED`.
    NotSynced,
    /// `PAUSED`.
    Paused,
    /// `NETWORKHALTED`.
    NetworkHalted,
    /// `DOWNLOADED`.
    Downloaded,
    /// `NOBLOCK`.
    NoBlock,
    /// `PROCESSFINISHED`.
    ProcessFinished,
    /// `SYNCFINISHED`.
    SyncFinished,
    /// `FORK`.
    Fork,
    /// `TEST`.
    Test,
    /// `WAKEUP`.
    WakeUp,
    /// `NEWBLOCK`.
    NewBlock,
    /// `SUCCESS`.
    Success,
}

impl MachineState {
    /// The state's name as the reference's machine writes it (`idle`,
    /// `syncWithNetwork.downloadBlocks`, ...).
    pub fn name(self) -> &'static str {
        todo!()
    }

    /// Whether the machine is idle.
    pub fn is_idle(self) -> bool {
        self == MachineState::Idle
    }
}

impl MachineEvent {
    /// The event's name, as the reference's machine writes it (`START`, `NEWBLOCK`, ...).
    pub fn name(self) -> &'static str {
        todo!()
    }
}

/// A transition: the next state and the actions its entry runs, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transition {
    /// The next state.
    pub next: MachineState,
    /// The entry actions of the next state.
    pub actions: Vec<Action>,
}

/// The transition of `event` in `state`; `None` when neither the state nor its parent lists the
/// event, which is then ignored.
pub fn transition(state: MachineState, event: MachineEvent) -> Option<Transition> {
    let _ = (state, event);
    todo!()
}
