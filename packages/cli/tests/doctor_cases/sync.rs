//! The cases of the sync checks: each names the check, the view, the options and the findings it
//! must give. The registry walk requires a case for each status a built check declares.

use heartwood_cli::doctor::report::{Basis, Fact, Status};
use heartwood_cli::doctor::seams::Subject;
use heartwood_cli::doctor::sources::api::{ApiStatus, ApiSyncing};
use heartwood_cli::doctor::sources::logs::LineKind;
use heartwood_cli::doctor::view::{Reason, RolePresence, Source, Unknown};
use heartwood_cli::{Role, SupervisedStatus};
use heartwood_kernel::log::Level;

use super::Case;
use super::process::{case, log, run, view};
use crate::doctor_support::FakeView;

const RELAY: Subject = Subject::Relay;

/// A view whose relay runs and is `synced` or not, `blocks` behind by the status and
/// `syncing_blocks` by the syncing report, with `kinds` in its log.
fn lagging(synced: bool, blocks: i64, syncing_blocks: i64, kinds: Vec<LineKind>) -> FakeView {
    let mut view = view(SupervisedStatus::Online, None);
    view.api_status = Ok(ApiStatus {
        synced,
        now: 500,
        blocks_count: blocks,
    });
    let syncing = ApiSyncing {
        syncing: !synced,
        blocks: syncing_blocks,
        height: 500,
    };
    view.api_syncing = Ok(syncing);
    view.logs.insert(Role::Relay, Ok(log(kinds)));
    view
}

/// `sync.state` over `view`.
fn state(name: &'static str, view: FakeView, status: Status, advice: &'static str) -> Case {
    case(name, "sync.state", view, RELAY, status, advice)
}

/// `sync.forks` over a running relay whose log holds `kinds`.
fn forks(name: &'static str, kinds: Vec<LineKind>, status: Status, advice: &'static str) -> Case {
    let mut view = view(SupervisedStatus::Online, None);
    view.logs.insert(Role::Relay, Ok(log(kinds)));
    case(name, "sync.forks", view, RELAY, status, advice)
}

/// `n` fork recoveries.
fn recoveries(n: usize) -> Vec<LineKind> {
    vec![LineKind::ForkRecoveryStarted; n]
}

/// Every case of the area.
pub fn cases() -> Vec<Case> {
    let synced = vec![LineKind::RelayStarted, LineKind::InSync];
    let started = vec![LineKind::RelayStarted];
    let quiet = |blocks| {
        let mut view = lagging(false, blocks, blocks, Vec::new());
        let mut window = log(vec![LineKind::RelayStarted, LineKind::InSync]);
        window.file_level = Some(Level::Warning);
        view.logs.insert(Role::Relay, Ok(window));
        view
    };
    let no_log = |blocks| {
        let mut view = lagging(false, blocks, blocks, Vec::new());
        view.logs.clear();
        view
    };
    let mut not_here = FakeView::new();
    not_here.roles_present = Ok(RolePresence {
        relay: false,
        forger: true,
    });
    let mut no_status = lagging(true, 0, 0, Vec::new());
    no_status.api_status = Err(Unknown::new(Source::Api, Reason::NotBuilt));
    let mut corrupt_down = view(SupervisedStatus::Errored, None);
    corrupt_down
        .logs
        .insert(Role::Relay, Ok(log(vec![LineKind::StateCorrupted])));
    let mut corrupt_unread = FakeView::new();
    corrupt_unread.roles_present = Ok(RolePresence {
        relay: true,
        forger: false,
    });
    corrupt_unread
        .logs
        .insert(Role::Relay, Ok(log(vec![LineKind::StateMightBeCorrupted])));
    let mut no_forks_log = view(SupervisedStatus::Online, None);
    no_forks_log.logs.clear();
    let mut level_forks = view(SupervisedStatus::Online, None);
    let mut narrow = log(recoveries(3));
    narrow.file_level = Some(Level::Error);
    level_forks.logs.insert(Role::Relay, Ok(narrow));
    vec![
        state(
            "state: synced",
            lagging(true, 0, 0, Vec::new()),
            Status::Pass,
            "synced",
        ),
        state(
            "state: synced, whatever the log says",
            lagging(true, 500, 500, started.clone()),
            Status::Pass,
            "synced",
        ),
        state(
            "state: not synced since the start",
            lagging(false, 10, 10, started),
            Status::Warn,
            "first-sync",
        ),
        state(
            "state: 53 behind, synced before",
            lagging(false, 53, 53, synced.clone()),
            Status::Warn,
            "catching-up",
        ),
        state(
            "state: 54 behind, synced before",
            lagging(false, 54, 0, synced.clone()),
            Status::Fail,
            "fell-behind",
        ),
        state(
            "state: 54 behind by the syncing report",
            lagging(false, 0, 54, synced.clone()),
            Status::Fail,
            "fell-behind",
        ),
        state(
            "state: a negative count is no lag",
            lagging(false, -5, -5, synced),
            Status::Warn,
            "catching-up",
        ),
        state(
            "state: 54 behind, the log has no word on info",
            quiet(54),
            Status::Unknown,
            "unknown",
        ),
        state(
            "state: 10 behind, the log has no word on info",
            quiet(10),
            Status::Warn,
            "catching-up",
        ),
        state(
            "state: 54 behind, no log",
            no_log(54),
            Status::Unknown,
            "unknown",
        ),
        state(
            "state: 10 behind, no log",
            no_log(10),
            Status::Warn,
            "catching-up",
        ),
        state(
            "state: 54 behind, started before the window, never in sync in it",
            lagging(false, 54, 54, Vec::new()),
            Status::Unknown,
            "unknown",
        ),
        state(
            "state: 54 behind, started before the window, in sync in it",
            lagging(false, 54, 54, vec![LineKind::InSync]),
            Status::Fail,
            "fell-behind",
        ),
        state(
            "state: the status is unknown",
            no_status,
            Status::Unknown,
            "unknown",
        ),
        state(
            "state: no relay here",
            not_here,
            Status::Skipped,
            "skipped-absent",
        ),
        forks("forks: none", Vec::new(), Status::Pass, "none"),
        forks("forks: one recovery", recoveries(1), Status::Warn, "some"),
        forks("forks: two recoveries", recoveries(2), Status::Warn, "some"),
        forks(
            "forks: three recoveries",
            recoveries(3),
            Status::Fail,
            "many",
        ),
        forks(
            "forks: one failed download",
            vec![LineKind::DownloadFailed],
            Status::Warn,
            "some",
        ),
        forks(
            "forks: no peers to download from",
            vec![LineKind::NoPeersToDownload],
            Status::Warn,
            "some",
        ),
        forks(
            "forks: a block not accepted",
            vec![LineKind::DownloadedBlockNotAccepted],
            Status::Warn,
            "some",
        ),
        forks(
            "forks: no blocks from any peer",
            vec![LineKind::NoBlocksFromAnyPeer { from: 5 }],
            Status::Warn,
            "some",
        ),
        forks(
            "forks: corruption logged, the relay runs",
            vec![LineKind::StateCorrupted],
            Status::Warn,
            "corruption-logged",
        ),
        forks(
            "forks: possible corruption logged",
            vec![LineKind::StateMightBeCorrupted],
            Status::Warn,
            "corruption-logged",
        ),
        case(
            "forks: corruption logged, the relay is down",
            "sync.forks",
            corrupt_down,
            RELAY,
            Status::Fail,
            "corrupted",
        ),
        case(
            "forks: corruption logged, the process unknown",
            "sync.forks",
            corrupt_unread,
            RELAY,
            Status::Warn,
            "corruption-logged",
        ),
        case(
            "forks: no log",
            "sync.forks",
            no_forks_log,
            RELAY,
            Status::Unknown,
            "unknown",
        ),
        case(
            "forks: the log skips info",
            "sync.forks",
            level_forks,
            RELAY,
            Status::Unknown,
            "unknown",
        ),
        case(
            "forks: no relay here",
            "sync.forks",
            {
                let mut view = FakeView::new();
                view.roles_present = Ok(RolePresence {
                    relay: false,
                    forger: true,
                });
                view
            },
            RELAY,
            Status::Skipped,
            "skipped-absent",
        ),
    ]
}

#[test]
fn forks_are_counted_over_the_minutes_of_the_window() {
    let mut view = view(SupervisedStatus::Online, None);
    view.logs.insert(Role::Relay, Ok(log(recoveries(2))));
    let findings = run("sync.forks", &view);
    let [finding] = findings.as_slice() else {
        panic!("one finding")
    };
    assert!(finding.facts.contains(&("forks", Fact::Int(2))));
    assert!(finding.facts.contains(&("minutes", Fact::Int(60))));
    assert_eq!(finding.basis, Basis::Log);
}

#[test]
fn a_corruption_fail_rests_on_the_process_table() {
    let mut view = view(SupervisedStatus::Errored, None);
    view.logs
        .insert(Role::Relay, Ok(log(vec![LineKind::StateCorrupted])));
    let findings = run("sync.forks", &view);
    assert!(
        findings
            .iter()
            .all(|finding| finding.basis == Basis::Process)
    );
}
