//! The blockchain machine: every event in every state against the reference's table (the ignored
//! events included), the entry actions, the names, and the service's start, failure and stop.

mod common;
mod service;

use std::time::Duration;

use heartwood_blockchain::state_machine::{
    Action, MachineEvent as E, MachineState as S, SyncState as Y, transition,
};
use service::{Harness, Setup};

const SYNC: [Y; 7] = [
    Y::Syncing,
    Y::Idle,
    Y::DownloadBlocks,
    Y::DownloadFinished,
    Y::DownloadPaused,
    Y::ProcessFinished,
    Y::End,
];

const EVENTS: [E; 19] = [
    E::Start,
    E::Started,
    E::NetworkStart,
    E::Rollback,
    E::Failure,
    E::Stop,
    E::Synced,
    E::NotSynced,
    E::Paused,
    E::NetworkHalted,
    E::Downloaded,
    E::NoBlock,
    E::ProcessFinished,
    E::SyncFinished,
    E::Fork,
    E::Test,
    E::WakeUp,
    E::NewBlock,
    E::Success,
];

fn states() -> Vec<S> {
    let mut states = vec![S::Uninitialised, S::Initialise];
    states.extend(SYNC.map(S::SyncWithNetwork));
    states.extend([
        S::Idle,
        S::NewBlock,
        S::Fork,
        S::Rollback,
        S::Stopped,
        S::Exit,
    ]);
    states
}

/// The reference's machine, transition by transition.
fn table() -> Vec<(S, E, S)> {
    let sync = S::SyncWithNetwork;
    let mut table = vec![
        (S::Uninitialised, E::Start, S::Initialise),
        (S::Uninitialised, E::Stop, S::Stopped),
        (S::Initialise, E::NetworkStart, S::Idle),
        (S::Initialise, E::Started, sync(Y::Syncing)),
        (S::Initialise, E::Rollback, S::Rollback),
        (S::Initialise, E::Failure, S::Exit),
        (S::Initialise, E::Stop, S::Stopped),
        (sync(Y::Syncing), E::Synced, sync(Y::DownloadFinished)),
        (sync(Y::Syncing), E::NotSynced, sync(Y::DownloadBlocks)),
        (sync(Y::Syncing), E::Paused, sync(Y::DownloadPaused)),
        (sync(Y::Syncing), E::NetworkHalted, sync(Y::End)),
        (sync(Y::Idle), E::Downloaded, sync(Y::DownloadBlocks)),
        (sync(Y::DownloadBlocks), E::Downloaded, sync(Y::Syncing)),
        (sync(Y::DownloadBlocks), E::NoBlock, sync(Y::Syncing)),
        (
            sync(Y::DownloadBlocks),
            E::ProcessFinished,
            sync(Y::DownloadFinished),
        ),
        (
            sync(Y::DownloadFinished),
            E::ProcessFinished,
            sync(Y::ProcessFinished),
        ),
        (
            sync(Y::DownloadPaused),
            E::ProcessFinished,
            sync(Y::ProcessFinished),
        ),
        (sync(Y::ProcessFinished), E::Synced, sync(Y::End)),
        (
            sync(Y::ProcessFinished),
            E::NotSynced,
            sync(Y::DownloadBlocks),
        ),
        (S::Idle, E::WakeUp, sync(Y::Syncing)),
        (S::Idle, E::NewBlock, S::NewBlock),
        (S::Idle, E::Stop, S::Stopped),
        (S::Idle, E::Fork, S::Fork),
        (S::NewBlock, E::ProcessFinished, S::Idle),
        (S::NewBlock, E::Fork, S::Fork),
        (S::NewBlock, E::Stop, S::Stopped),
        (S::Fork, E::Success, sync(Y::Syncing)),
        (S::Fork, E::Failure, S::Exit),
        (S::Fork, E::Stop, S::Stopped),
        (S::Rollback, E::Success, S::Initialise),
        (S::Rollback, E::Failure, S::Exit),
        (S::Rollback, E::Stop, S::Stopped),
    ];
    // The parent's events, in every sub-state.
    for sub in SYNC {
        table.push((sync(sub), E::Test, S::Idle));
        table.push((sync(sub), E::SyncFinished, S::Idle));
        table.push((sync(sub), E::Fork, S::Fork));
        table.push((sync(sub), E::Stop, S::Stopped));
    }
    table
}

#[test]
fn every_event_in_every_state_follows_the_reference_table() {
    let table = table();
    assert_eq!(table.len(), 60);
    let mut taken = 0;
    for state in states() {
        for event in EVENTS {
            let expected = table
                .iter()
                .find(|(from, on, _)| *from == state && *on == event)
                .map(|(_, _, to)| *to);
            let got = transition(state, event);
            let context = format!("{} on {}", state.name(), event.name());
            assert_eq!(got.as_ref().map(|t| t.next), expected, "{context}");
            if let Some(got) = got {
                taken += 1;
                assert_eq!(got.actions, got.next.entry_actions(), "{context}");
            }
        }
    }
    assert_eq!(taken, 60);
    assert_eq!(states().len() * EVENTS.len() - taken, 225, "ignored events");
}

#[test]
fn each_state_runs_the_reference_s_entry_actions() {
    let expected: [(S, &[Action]); 15] = [
        (S::Uninitialised, &[]),
        (S::Initialise, &[Action::Initialise]),
        (
            S::SyncWithNetwork(Y::Syncing),
            &[Action::CheckLastDownloadedBlockSynced],
        ),
        (S::SyncWithNetwork(Y::Idle), &[]),
        (
            S::SyncWithNetwork(Y::DownloadBlocks),
            &[Action::DownloadBlocks],
        ),
        (
            S::SyncWithNetwork(Y::DownloadFinished),
            &[Action::DownloadFinished],
        ),
        (
            S::SyncWithNetwork(Y::DownloadPaused),
            &[Action::DownloadPaused],
        ),
        (
            S::SyncWithNetwork(Y::ProcessFinished),
            &[Action::CheckLastBlockSynced],
        ),
        (S::SyncWithNetwork(Y::End), &[Action::SyncingComplete]),
        (S::Idle, &[Action::CheckLater, Action::BlockchainReady]),
        (S::NewBlock, &[]),
        (S::Fork, &[Action::StartForkRecovery]),
        (S::Rollback, &[Action::RollbackDatabase]),
        (S::Stopped, &[Action::Stopped]),
        (S::Exit, &[Action::ExitApp]),
    ];
    for (state, actions) in expected {
        assert_eq!(state.entry_actions(), actions, "{}", state.name());
    }
}

#[test]
fn names_are_the_reference_s() {
    let names: Vec<_> = states().into_iter().map(S::name).collect();
    assert_eq!(
        names,
        [
            "uninitialised",
            "initialise",
            "syncWithNetwork.syncing",
            "syncWithNetwork.idle",
            "syncWithNetwork.downloadBlocks",
            "syncWithNetwork.downloadFinished",
            "syncWithNetwork.downloadPaused",
            "syncWithNetwork.processFinished",
            "syncWithNetwork.end",
            "idle",
            "newBlock",
            "fork",
            "rollback",
            "stopped",
            "exit",
        ]
    );
    let events: Vec<_> = EVENTS.into_iter().map(E::name).collect();
    assert_eq!(
        events.join(" "),
        "START STARTED NETWORKSTART ROLLBACK FAILURE STOP SYNCED NOTSYNCED PAUSED NETWORKHALTED \
         DOWNLOADED NOBLOCK PROCESSFINISHED SYNCFINISHED FORK TEST WAKEUP NEWBLOCK SUCCESS"
    );
    assert_eq!(
        Action::CheckLastDownloadedBlockSynced.name(),
        "checkLastDownloadedBlockSynced"
    );
    assert_eq!(Action::ExitApp.name(), "exitApp");
    assert!(S::Idle.is_idle() && !S::NewBlock.is_idle());
}

#[tokio::test(start_paused = true)]
async fn an_ignored_event_changes_nothing() {
    let h = Harness::new(Setup::default());
    for event in [
        E::ProcessFinished,
        E::WakeUp,
        E::NewBlock,
        E::Success,
        E::Synced,
    ] {
        h.service.dispatch(event);
    }
    h.settle(Duration::from_secs(5)).await;
    assert_eq!(h.state(), "uninitialised");
    assert!(h.network.calls().is_empty());
    assert!(h.node.log().is_empty());
    assert!(!h.status.has_changed().unwrap());
}

#[tokio::test(start_paused = true)]
async fn the_start_restores_the_pool_boots_the_network_and_syncs_to_idle() {
    let mut h = Harness::new(Setup {
        peers: 0,
        ..Setup::default()
    });
    h.service.boot().await.unwrap();
    h.until("idle", Duration::from_secs(1)).await;
    assert!(h.status.borrow().started && h.chain().started);
    // The pool's re-add and the online record's save come before the network's boot.
    assert_eq!(h.node.log()[..2], ["drain", "expire 1"]);
    let calls = h.network.calls();
    assert_eq!(calls[0], "boot");
    assert!(calls.contains(&"synced".to_owned()));
    // The boot pings 10 random peers once the blockchain has started, and the first idle entry
    // pings every peer fast, without common blocks.
    assert!(calls.contains(&"cleanse fast=false force=true count=10 skip=false".to_owned()));
    assert!(calls.contains(&"cleanse fast=true force=true count=all skip=true".to_owned()));
}

#[tokio::test(start_paused = true)]
async fn a_failed_start_exits_and_fails_the_boot() {
    let mut h = Harness::new(Setup::default());
    h.network.state().boot_fails = true;
    let booted = h.service.boot().await;
    assert_eq!(booted.unwrap_err().package, "@solar-network/blockchain");
    h.until("exit", Duration::from_secs(1)).await;
    assert!(!h.chain().started);
}

#[tokio::test(start_paused = true)]
async fn the_network_start_mode_finishes_the_sync_at_once() {
    let options = heartwood_blockchain::BlockchainOptions {
        network_start: true,
        ..Default::default()
    };
    let mut h = Harness::new(Setup {
        options,
        now_ms: service::time_of(265) + 600_000,
        ..Setup::default()
    });
    h.service.boot().await.unwrap();
    h.until("idle", Duration::from_secs(1)).await;
    // `downloadFinished` turns the flag off for the next sync, and nothing was downloaded.
    assert!(!h.chain().network_start);
    let syncs = h
        .network
        .calls()
        .iter()
        .filter(|c| c.contains("silent=false"))
        .count();
    assert_eq!(syncs, 0, "no sync download");
    assert_eq!(
        h.network.count("synced"),
        0,
        "no syncingComplete in this mode"
    );
}

#[tokio::test(start_paused = true)]
async fn the_stop_ends_the_machine_the_queue_and_the_loops() {
    let mut h = Harness::new(Setup {
        peers: 0,
        ..Setup::default()
    });
    h.service.boot().await.unwrap();
    h.until("idle", Duration::from_secs(1)).await;
    h.service.dispose().await;
    h.until("stopped", Duration::from_secs(1)).await;
    let polls = h.network.count("download ");
    h.settle(Duration::from_secs(120)).await;
    assert_eq!(h.network.count("download "), polls, "the idle loops ended");
    assert_eq!(h.state(), "stopped", "no wake-up after the stop");
    // A second stop does nothing.
    h.service.dispose().await;
}
