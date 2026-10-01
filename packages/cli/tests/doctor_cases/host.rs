//! The cases of the host checks: each names the check, the view, the options and the findings it
//! must give. The registry walk requires a case for each status a built check declares.

use heartwood_cli::Role;
use heartwood_cli::doctor::report::{Fact, HostFacts, Status};
use heartwood_cli::doctor::seams::{FileFacts, FileKind, Meta, Subject};
use heartwood_cli::doctor::sources::procfs::{MemInfo, ProcStatus};
use heartwood_cli::doctor::view::{NodeDir, NodeFile, Reason, RolePresence, Source, Unknown};

use super::Case;
use super::process::{case, run};
use crate::doctor_support::FakeView;

const HOST: Subject = Subject::Host;
const GIB: u64 = 1 << 30;

/// A host of `cpus` and `memory` bytes that runs a relay, a forger or both.
fn sized(cpus: Option<u32>, memory: u64, relay: bool, forger: bool) -> FakeView {
    let mut view = FakeView::new();
    view.host = HostFacts {
        cpus,
        ..HostFacts::default()
    };
    view.meminfo = Ok(MemInfo {
        total: memory,
        available: memory,
        swap_total: 0,
        swap_free: 0,
    });
    view.roles_present = Ok(RolePresence { relay, forger });
    view
}

/// `host.resources` over `view`.
fn resources(name: &'static str, view: FakeView, status: Status, advice: &'static str) -> Case {
    case(name, "host.resources", view, HOST, status, advice)
}

/// A host of 1000 bytes with `available` of them free, whose roles do not run.
fn free(available: u64) -> FakeView {
    let mut view = FakeView::new();
    view.meminfo = Ok(MemInfo {
        total: 1_000,
        available,
        swap_total: 0,
        swap_free: 0,
    });
    for role in Role::SPLIT {
        view.role_statuses
            .insert(role, Err(Unknown::new(Source::Process, Reason::NotRunning)));
    }
    view
}

/// A host with plenty of memory whose relay holds `swap` bytes in swap.
fn swapping(swap: Option<u64>) -> FakeView {
    let mut view = free(500);
    let status = ProcStatus {
        uid: 1000,
        rss_bytes: Some(1),
        swap_bytes: swap,
    };
    view.role_statuses.insert(Role::Relay, Ok(status));
    view
}

/// `host.memory` over `view`.
fn memory(name: &'static str, view: FakeView, status: Status, advice: &'static str) -> Case {
    case(name, "host.memory", view, HOST, status, advice)
}

/// A view whose doctor runs as `uid` and whose data directory belongs to `holder`.
fn owned(uid: u32, holder: Option<u32>) -> FakeView {
    let mut view = FakeView::new();
    view.own_uid = Ok(uid);
    let meta = |holder| Meta {
        kind: FileKind::Dir,
        mode: 0o700,
        uid: holder,
        gid: holder,
        size: 4096,
        dev: 1,
        ino: 2,
        nlink: 2,
        modified_ms: 0,
    };
    let facts = holder.map(|holder| FileFacts {
        path: "/nodes/node1/data".into(),
        own: meta(holder),
        target: None,
        link_target: None,
    });
    view.files.insert(NodeFile::Dir(NodeDir::Data), Ok(facts));
    view
}

/// `host.user` over `view`.
fn user(name: &'static str, view: FakeView, status: Status, advice: &'static str) -> Case {
    case(name, "host.user", view, HOST, status, advice)
}

/// Every case of the area.
pub fn cases() -> Vec<Case> {
    let mut no_memory = sized(Some(4), 8 * GIB, true, false);
    no_memory.meminfo = Err(Unknown::new(Source::Proc, Reason::NotBuilt));
    let mut no_total = free(0);
    no_total.meminfo = Ok(MemInfo {
        total: 0,
        available: 0,
        swap_total: 0,
        swap_free: 0,
    });
    let mut denied = free(500);
    denied.role_statuses.insert(
        Role::Relay,
        Err(Unknown::new(Source::Process, Reason::Denied)),
    );
    let mut no_file = owned(1000, Some(1000));
    no_file.files.clear();
    let mut unowned = FakeView::new();
    unowned.own_uid = Ok(1000);
    unowned.files.insert(NodeFile::Dir(NodeDir::Data), Ok(None));
    vec![
        resources(
            "resources: a relay on 4 CPUs and 8 GiB",
            sized(Some(4), 8 * GIB, true, false),
            Status::Pass,
            "enough",
        ),
        resources(
            "resources: a relay on 3 CPUs",
            sized(Some(3), 8 * GIB, true, false),
            Status::Warn,
            "below-estimate",
        ),
        resources(
            "resources: a relay on 90% of the memory",
            sized(Some(4), 7_730_941_133, true, false),
            Status::Pass,
            "enough",
        ),
        resources(
            "resources: a relay just under 90% of the memory",
            sized(Some(4), 7_730_941_132, true, false),
            Status::Warn,
            "below-estimate",
        ),
        resources(
            "resources: a forger on 2 CPUs and 2 GiB",
            sized(Some(2), 2 * GIB, false, true),
            Status::Pass,
            "enough",
        ),
        resources(
            "resources: a forger on 1 CPU",
            sized(Some(1), 2 * GIB, false, true),
            Status::Warn,
            "below-estimate",
        ),
        resources(
            "resources: both roles on 6 CPUs and 10 GiB",
            sized(Some(6), 10 * GIB, true, true),
            Status::Pass,
            "enough",
        ),
        resources(
            "resources: both roles on 5 CPUs",
            sized(Some(5), 10 * GIB, true, true),
            Status::Warn,
            "below-estimate",
        ),
        resources(
            "resources: both roles on 8 GiB",
            sized(Some(6), 8 * GIB, true, true),
            Status::Warn,
            "below-estimate",
        ),
        resources(
            "resources: the CPUs are unknown",
            sized(None, 8 * GIB, true, false),
            Status::Unknown,
            "unknown",
        ),
        resources(
            "resources: the memory is unknown",
            no_memory,
            Status::Unknown,
            "unknown",
        ),
        resources(
            "resources: the roles are unknown",
            {
                let mut view = sized(Some(4), 8 * GIB, true, false);
                view.roles_present = FakeView::new().roles_present;
                view
            },
            Status::Unknown,
            "unknown",
        ),
        resources(
            "resources: no role runs here",
            sized(Some(4), 8 * GIB, false, false),
            Status::Unknown,
            "unknown",
        ),
        memory(
            "memory: 1.9% available",
            free(19),
            Status::Fail,
            "exhausted",
        ),
        memory("memory: 2% available", free(20), Status::Warn, "low"),
        memory("memory: 10% available", free(100), Status::Warn, "low"),
        memory("memory: 10.1% available", free(101), Status::Pass, "enough"),
        memory(
            "memory: a role in swap",
            swapping(Some(1)),
            Status::Warn,
            "swap",
        ),
        memory(
            "memory: a role with none in swap",
            swapping(Some(0)),
            Status::Pass,
            "enough",
        ),
        memory(
            "memory: a role whose swap is not shown",
            swapping(None),
            Status::Pass,
            "enough",
        ),
        memory(
            "memory: a role's status is denied",
            denied,
            Status::Unknown,
            "unknown",
        ),
        memory(
            "memory: no memory total",
            no_total,
            Status::Unknown,
            "unknown",
        ),
        memory(
            "memory: the memory is unknown",
            FakeView::new(),
            Status::Unknown,
            "unknown",
        ),
        user(
            "user: the node's own user",
            owned(1000, Some(1000)),
            Status::Pass,
            "node-user",
        ),
        user("user: root", owned(0, Some(1000)), Status::Warn, "root"),
        user(
            "user: root holds the data",
            owned(0, Some(0)),
            Status::Warn,
            "root",
        ),
        user(
            "user: another user holds the data",
            owned(1000, Some(1001)),
            Status::Warn,
            "other-user",
        ),
        user(
            "user: no data directory",
            unowned,
            Status::Unknown,
            "unknown",
        ),
        user(
            "user: the data directory is not read",
            no_file,
            Status::Unknown,
            "unknown",
        ),
        user(
            "user: the user is unknown",
            FakeView::new(),
            Status::Unknown,
            "unknown",
        ),
    ]
}

#[test]
fn the_resources_check_never_fails() {
    let sizes = [(Some(1), 1), (Some(0), 0), (Some(64), 1 << 40), (None, 1)];
    for (cpus, memory) in sizes.into_iter().chain([(Some(u32::MAX), u64::MAX)]) {
        let findings = run("host.resources", &sized(cpus, memory, true, true));
        assert!(findings.iter().all(|f| f.status != Status::Fail));
    }
}

#[test]
fn the_resources_say_what_they_compare() {
    let findings = run("host.resources", &sized(Some(4), 8 * GIB, true, true));
    let [finding] = findings.as_slice() else {
        panic!("one finding")
    };
    assert!(finding.facts.contains(&("need_cpus", Fact::Int(6))));
    let need = ("need_memory_bytes", Fact::Bytes(10 * GIB));
    assert!(finding.facts.contains(&need), "{finding:#?}");
    let memory = ("memory_bytes", Fact::Bytes(8 * GIB));
    assert!(finding.facts.contains(&memory), "{finding:#?}");
    assert!(
        finding
            .facts
            .contains(&("roles", Fact::Word("relay-and-forger")))
    );
}

#[test]
fn the_swap_finding_names_the_role() {
    let findings = run("host.memory", &swapping(Some(4096)));
    let [finding] = findings.as_slice() else {
        panic!("one finding")
    };
    assert!(finding.facts.contains(&("role", Fact::Word("relay"))));
}

#[test]
fn memory_figures_at_the_limits_of_their_type_give_a_finding_and_no_panic() {
    for (total, available) in [(u64::MAX, u64::MAX), (u64::MAX, 0), (1, u64::MAX)] {
        let mut view = free(0);
        view.meminfo = Ok(MemInfo {
            total,
            available,
            swap_total: u64::MAX,
            swap_free: 0,
        });
        let findings = run("host.memory", &view);
        assert_eq!(findings.len(), 1);
    }
}
