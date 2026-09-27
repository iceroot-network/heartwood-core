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
//! Entering the syncing parent enters syncing / syncing. A sub-state's own events come first; the
//! parent's apply in every sub-state. No state is left for itself, so every transition runs the
//! entry actions of the state it enters.

use crate::state_machine::actions::Action;

named! {
    /// A sub-state of the syncing state (`syncWithNetwork`).
    SyncState {
        /// `syncing`.
        Syncing = "syncWithNetwork.syncing",
        /// `idle`, which no transition enters.
        Idle = "syncWithNetwork.idle",
        /// `downloadBlocks`.
        DownloadBlocks = "syncWithNetwork.downloadBlocks",
        /// `downloadFinished`.
        DownloadFinished = "syncWithNetwork.downloadFinished",
        /// `downloadPaused`.
        DownloadPaused = "syncWithNetwork.downloadPaused",
        /// `processFinished`.
        ProcessFinished = "syncWithNetwork.processFinished",
        /// `end`.
        End = "syncWithNetwork.end",
    }
}

/// A state of the machine; `uninitialised` by default.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum MachineState {
    /// `uninitialised`.
    #[default]
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

named! {
    /// An event of the machine, by the reference's names.
    MachineEvent {
        /// `START`.
        Start = "START",
        /// `STARTED`.
        Started = "STARTED",
        /// `NETWORKSTART`.
        NetworkStart = "NETWORKSTART",
        /// `ROLLBACK`.
        Rollback = "ROLLBACK",
        /// `FAILURE`.
        Failure = "FAILURE",
        /// `STOP`.
        Stop = "STOP",
        /// `SYNCED`.
        Synced = "SYNCED",
        /// `NOTSYNCED`.
        NotSynced = "NOTSYNCED",
        /// `PAUSED`.
        Paused = "PAUSED",
        /// `NETWORKHALTED`.
        NetworkHalted = "NETWORKHALTED",
        /// `DOWNLOADED`.
        Downloaded = "DOWNLOADED",
        /// `NOBLOCK`.
        NoBlock = "NOBLOCK",
        /// `PROCESSFINISHED`.
        ProcessFinished = "PROCESSFINISHED",
        /// `SYNCFINISHED`.
        SyncFinished = "SYNCFINISHED",
        /// `FORK`.
        Fork = "FORK",
        /// `TEST`.
        Test = "TEST",
        /// `WAKEUP`.
        WakeUp = "WAKEUP",
        /// `NEWBLOCK`.
        NewBlock = "NEWBLOCK",
        /// `SUCCESS`.
        Success = "SUCCESS",
    }
}

impl MachineState {
    /// The state's name as the reference's machine writes it (`idle`,
    /// `syncWithNetwork.downloadBlocks`, ...).
    pub fn name(self) -> &'static str {
        use MachineState as S;
        match self {
            S::Uninitialised => "uninitialised",
            S::Initialise => "initialise",
            S::SyncWithNetwork(sub) => sub.name(),
            S::Idle => "idle",
            S::NewBlock => "newBlock",
            S::Fork => "fork",
            S::Rollback => "rollback",
            S::Stopped => "stopped",
            S::Exit => "exit",
        }
    }

    /// The actions the state runs on entry, in order.
    pub fn entry_actions(self) -> Vec<Action> {
        use {Action as A, SyncState as Y};
        match self {
            MachineState::Uninitialised | MachineState::NewBlock => Vec::new(),
            MachineState::Initialise => vec![A::Initialise],
            MachineState::SyncWithNetwork(sub) => match sub {
                Y::Syncing => vec![A::CheckLastDownloadedBlockSynced],
                Y::Idle => Vec::new(),
                Y::DownloadBlocks => vec![A::DownloadBlocks],
                Y::DownloadFinished => vec![A::DownloadFinished],
                Y::DownloadPaused => vec![A::DownloadPaused],
                Y::ProcessFinished => vec![A::CheckLastBlockSynced],
                Y::End => vec![A::SyncingComplete],
            },
            MachineState::Idle => vec![A::CheckLater, A::BlockchainReady],
            MachineState::Fork => vec![A::StartForkRecovery],
            MachineState::Rollback => vec![A::RollbackDatabase],
            MachineState::Stopped => vec![A::Stopped],
            MachineState::Exit => vec![A::ExitApp],
        }
    }

    /// Whether the machine is idle.
    pub fn is_idle(self) -> bool {
        self == MachineState::Idle
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
    use MachineEvent as E;
    use MachineState as S;
    use SyncState as Y;
    let syncing = S::SyncWithNetwork(Y::Syncing);
    let next = match (state, event) {
        (S::Uninitialised, E::Start) => S::Initialise,
        (S::Initialise, E::NetworkStart) => S::Idle,
        (S::Initialise, E::Started) => syncing,
        (S::Initialise, E::Rollback) => S::Rollback,
        (S::SyncWithNetwork(sub), event) => match (sub, event) {
            (Y::Syncing, E::Synced) => S::SyncWithNetwork(Y::DownloadFinished),
            (Y::ProcessFinished, E::Synced) => S::SyncWithNetwork(Y::End),
            (Y::Syncing | Y::ProcessFinished, E::NotSynced) => {
                S::SyncWithNetwork(Y::DownloadBlocks)
            }
            (Y::Syncing, E::Paused) => S::SyncWithNetwork(Y::DownloadPaused),
            (Y::Syncing, E::NetworkHalted) => S::SyncWithNetwork(Y::End),
            (Y::Idle, E::Downloaded) => S::SyncWithNetwork(Y::DownloadBlocks),
            (Y::DownloadBlocks, E::Downloaded | E::NoBlock) => syncing,
            (Y::DownloadBlocks, E::ProcessFinished) => S::SyncWithNetwork(Y::DownloadFinished),
            (Y::DownloadFinished | Y::DownloadPaused, E::ProcessFinished) => {
                S::SyncWithNetwork(Y::ProcessFinished)
            }
            (_, E::Test | E::SyncFinished) => S::Idle,
            (_, E::Fork) => S::Fork,
            (_, E::Stop) => S::Stopped,
            _ => return None,
        },
        (S::Idle, E::WakeUp) | (S::Fork, E::Success) => syncing,
        (S::Idle, E::NewBlock) => S::NewBlock,
        (S::NewBlock, E::ProcessFinished) => S::Idle,
        (S::Idle | S::NewBlock, E::Fork) => S::Fork,
        (S::Rollback, E::Success) => S::Initialise,
        (S::Initialise | S::Fork | S::Rollback, E::Failure) => S::Exit,
        (
            S::Uninitialised | S::Initialise | S::Idle | S::NewBlock | S::Fork | S::Rollback,
            E::Stop,
        ) => S::Stopped,
        _ => return None,
    };
    Some(Transition {
        next,
        actions: next.entry_actions(),
    })
}
