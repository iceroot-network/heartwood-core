//! The cases of the disk checks: free space and how long it lasts, the other directories' disks,
//! inodes, the kind of disk under the store, and the forger's slow writes.

use std::path::PathBuf;

use heartwood_cli::Role;
use heartwood_cli::doctor::registry::{CheckInput, ENTRIES};
use heartwood_cli::doctor::report::{Fact, Status};
use heartwood_cli::doctor::seams::{FsStats, RoleState, Subject};
use heartwood_cli::doctor::sources::api::ApiBlock;
use heartwood_cli::doctor::sources::logs::{Covered, LineKind, LogEvent, LogWindow};
use heartwood_cli::doctor::sources::procfs::{BlockDevice, MountRow};
use heartwood_cli::doctor::view::{NodeDir, NodeFile, Reason, Source, Unknown};
use heartwood_kernel::log::Level;

use super::Case;
use super::net::{base, case, denied, ex, file_of, presence, relay, resolve};
use super::slots::process;
use crate::doctor_support::FakeView;

const HOST: Subject = Subject::Host;
const FORGER: Subject = Subject::Forger;
const GB: u64 = 1_000_000_000;
const GIB: u64 = 1 << 30;
const MIB: u64 = 1 << 20;

/// A view whose data disk has `available` of `total` bytes free, and 1,000,000 file entries of
/// which `inodes` are free.
fn disk_view(total: u64, available: u64, inodes: u64) -> FakeView {
    let mut view = base();
    view.fs_stats.insert(
        NodeDir::Data,
        Ok(FsStats {
            total_bytes: total,
            available_bytes: available,
            total_inodes: 1_000_000,
            available_inodes: inodes,
        }),
    );
    view
}

/// A data disk with `available` of 1 TB free, on the planning growth.
fn free(available: u64) -> FakeView {
    disk_view(1_000 * GB, available, 900_000)
}

/// A data disk with `available` of 1 TB free, and a chain of `height` blocks of 1 MB each in the
/// store (a growth of 10.8 GB a day at a block time of 8 s, when it is 10,000 blocks or more).
fn chain_of(height: u32, available: u64) -> FakeView {
    let mut view = free(available);
    view.relay_config = Ok(relay("0.0.0.0:4001", None, Vec::new()));
    view.files.insert(
        NodeFile::Store,
        Ok(Some(file_of(u64::from(height) * 1_000_000))),
    );
    view.api_last_block = Ok(ApiBlock {
        id: "1".repeat(64),
        height,
        timestamp: None,
        generator_public_key: None,
        username: None,
    });
    view
}

/// The data disk's space.
fn space() -> Vec<Case> {
    let id = "disk.space";
    let mut cases = Vec::new();
    let mut add = |name, view, status, key| {
        cases.push(case(name, id, view, vec![ex(HOST, status, key)]));
    };
    add("plenty of space", free(400 * GB), Status::Pass, "ok");
    add("15 per cent free", free(150 * GB), Status::Pass, "ok");
    add(
        "just under 15 per cent free",
        free(150 * GB - 1),
        Status::Warn,
        "low",
    );
    add("5 per cent free", free(50 * GB), Status::Warn, "low");
    add(
        "just under 5 per cent free",
        free(50 * GB - 1),
        Status::Fail,
        "low",
    );
    let small = |available| disk_view(20 * GB, available, 900_000);
    add("2 GiB of a small disk", small(2 * GIB), Status::Warn, "low");
    add(
        "just under 2 GiB of a small disk",
        small(2 * GIB - 1),
        Status::Fail,
        "low",
    );
    add(
        "4 GiB of a large disk, on the planning growth",
        disk_view(30 * GB, 4 * GIB, 900_000),
        Status::Warn,
        "low",
    );
    let mut view = chain_of(20_000, 5_000 * GB);
    let stats = FsStats {
        total_bytes: 10_000 * GB,
        available_bytes: 5_000 * GB,
        total_inodes: 1_000_000,
        available_inodes: 900_000,
    };
    view.fs_stats.insert(NodeDir::Data, Ok(stats));
    add("many days left, measured", view, Status::Pass, "ok");
    add(
        "14 days left, measured",
        chain_of(20_000, 151_200_000_000),
        Status::Warn,
        "low",
    );
    add(
        "just under 14 days left, measured",
        chain_of(20_000, 151_199_999_999),
        Status::Fail,
        "low",
    );
    add(
        "planning growth just below 10,000 blocks",
        chain_of(9_999, 400 * GB),
        Status::Pass,
        "ok",
    );
    add(
        "measured growth at 10,000 blocks",
        chain_of(10_000, 400 * GB),
        Status::Warn,
        "low",
    );
    let mut view = chain_of(20_000, 500 * GB);
    view.api_last_block = Err(denied(Source::Api));
    add(
        "height unknown falls back to the planning growth",
        view,
        Status::Pass,
        "ok",
    );
    let mut view = free(400 * GB);
    view.fs_stats.insert(
        NodeDir::Data,
        Ok(FsStats {
            total_bytes: 1_000 * GB,
            available_bytes: 400 * GB,
            total_inodes: 0,
            available_inodes: 0,
        }),
    );
    add("a filesystem with no entry count", view, Status::Pass, "ok");
    add("data disk unreadable", base(), Status::Unknown, "unknown");
    add(
        "data disk of no size",
        disk_view(0, 0, 900_000),
        Status::Unknown,
        "unknown",
    );
    let mut view = base();
    view.fs_stats
        .insert(NodeDir::Data, Err(denied(Source::Files)));
    add("data disk refused", view, Status::Unknown, "unknown");
    cases
}

/// The growth facts of a run of `space` over `view`.
fn growth_of(view: &FakeView) -> (Fact, Fact) {
    let entry = ENTRIES
        .iter()
        .find(|entry| entry.id == "disk.space")
        .unwrap();
    let options = Default::default();
    let input = CheckInput {
        entry,
        view,
        options: &options,
    };
    let findings = (entry.run)(&input);
    let fact = |name: &str| {
        let found = findings
            .iter()
            .flat_map(|f| f.facts.iter())
            .find(|(key, _)| *key == name);
        found.map(|(_, fact)| fact.clone()).unwrap()
    };
    (fact("growth_bytes_per_day"), fact("growth"))
}

#[test]
fn growth_is_the_planning_figure_before_10000_blocks_and_measured_from_then() {
    let planning = Fact::Bytes(95_890_410);
    let measured = Fact::Bytes(10_800_000_000);
    let growth = |view| growth_of(&view);
    assert_eq!(
        growth(chain_of(9_999, 400 * GB)),
        (planning.clone(), Fact::Word("planning"))
    );
    assert_eq!(
        growth(chain_of(10_000, 400 * GB)),
        (measured.clone(), Fact::Word("measured"))
    );
    assert_eq!(
        growth(chain_of(20_000, 400 * GB)),
        (measured, Fact::Word("measured"))
    );
    let mut view = chain_of(20_000, 400 * GB);
    view.api_last_block = Err(denied(Source::Api));
    assert_eq!(growth(view), (planning, Fact::Word("planning-no-height")));
}

/// A mount of `major:minor` at `at`.
fn mount(major: u32, minor: u32, at: &str, fs_type: &str) -> MountRow {
    MountRow {
        major,
        minor,
        mount_point: PathBuf::from(at),
        fs_type: fs_type.into(),
    }
}

/// A view whose disks are as `mounts` say, with `stats` free bytes for the directories named.
fn mounts_view(mounts: Vec<MountRow>, stats: &[(NodeDir, u64)]) -> FakeView {
    let mut view = free(400 * GB);
    view.mounts = Ok(mounts);
    for (dir, available) in stats {
        let stat = FsStats {
            total_bytes: 100 * GB,
            available_bytes: *available,
            total_inodes: 1_000_000,
            available_inodes: 900_000,
        };
        view.fs_stats.insert(*dir, Ok(stat));
    }
    view
}

/// The other directories' disks.
fn other() -> Vec<Case> {
    let id = "disk.other";
    let root = || mount(8, 1, "/", "ext4");
    let logs = || mount(8, 2, "/nodes/node1/log", "ext4");
    let temp = || mount(8, 3, "/nodes/node1/temp", "ext4");
    let mut cases = Vec::new();
    let mut add = |name, view, status, key| {
        cases.push(case(name, id, view, vec![ex(HOST, status, key)]));
    };
    add(
        "one disk",
        mounts_view(vec![root()], &[]),
        Status::Skipped,
        "same-disk",
    );
    let log = |available| mounts_view(vec![root(), logs()], &[(NodeDir::Log, available)]);
    add("log disk with room", log(5 * GIB), Status::Pass, "ok");
    add("log disk at 1 GiB", log(GIB), Status::Pass, "ok");
    add(
        "log disk just under 1 GiB",
        log(GIB - 1),
        Status::Warn,
        "low",
    );
    add("log disk at 100 MiB", log(100 * MIB), Status::Warn, "low");
    add(
        "log disk just under 100 MiB",
        log(100 * MIB - 1),
        Status::Fail,
        "low",
    );
    let view = mounts_view(
        vec![root(), logs(), temp()],
        &[(NodeDir::Log, 5 * GIB), (NodeDir::Temp, 500 * MIB)],
    );
    add("the lowest of two other disks", view, Status::Warn, "low");
    let mut view = mounts_view(vec![root(), logs()], &[]);
    view.fs_stats.insert(
        NodeDir::Log,
        Err(Unknown::new(Source::Files, Reason::Missing)),
    );
    add(
        "a missing directory is left out",
        view,
        Status::Skipped,
        "same-disk",
    );
    let mut view = mounts_view(vec![root(), logs()], &[]);
    view.fs_stats
        .insert(NodeDir::Log, Err(denied(Source::Files)));
    add("another disk unreadable", view, Status::Unknown, "unknown");
    add("mount table unreadable", base(), Status::Unknown, "unknown");
    add(
        "data directory on no mount",
        mounts_view(vec![mount(8, 2, "/other", "ext4")], &[]),
        Status::Unknown,
        "unknown",
    );
    cases
}

/// The data disk's file entries.
fn inodes() -> Vec<Case> {
    let id = "disk.inodes";
    let mut cases = Vec::new();
    let mut add = |name, view, status, key| {
        cases.push(case(name, id, view, vec![ex(HOST, status, key)]));
    };
    let entries = |free| disk_view(GB, GB / 2, free);
    add("plenty of entries", entries(900_000), Status::Pass, "ok");
    add("5 per cent of entries", entries(50_000), Status::Pass, "ok");
    add(
        "just under 5 per cent",
        entries(49_999),
        Status::Warn,
        "low",
    );
    add(
        "1 per cent of entries",
        entries(10_000),
        Status::Warn,
        "low",
    );
    add("just under 1 per cent", entries(9_999), Status::Fail, "low");
    add("no entries left", entries(0), Status::Fail, "low");
    let mut view = disk_view(GB, GB / 2, 900_000);
    view.fs_stats.insert(
        NodeDir::Data,
        Ok(FsStats {
            total_bytes: GB,
            available_bytes: GB / 2,
            total_inodes: 0,
            available_inodes: 0,
        }),
    );
    add(
        "a filesystem with no entry count",
        view,
        Status::Pass,
        "no-count",
    );
    add("data disk unreadable", base(), Status::Unknown, "unknown");
    cases
}

/// A view whose data directory is on a filesystem of `fs_type`, with the block device 8:1 as
/// `device` says.
fn type_view(fs_type: &str, device: Option<BlockDevice>) -> FakeView {
    let mut view = base();
    view.mounts = Ok(vec![mount(8, 1, "/", fs_type)]);
    if let Some(device) = device {
        view.block_devices.insert((8, 1), Ok(device));
    }
    view
}

/// The kind of disk under the store.
fn disk_type() -> Vec<Case> {
    let id = "disk.type";
    let disk = |rotational| {
        Some(BlockDevice {
            partition: false,
            rotational,
        })
    };
    let partition = |rotational| {
        Some(BlockDevice {
            partition: true,
            rotational,
        })
    };
    let mut cases = Vec::new();
    let mut add = |name, view, status, key| {
        cases.push(case(name, id, view, vec![ex(HOST, status, key)]));
    };
    add(
        "a solid-state disk",
        type_view("ext4", disk(Some(false))),
        Status::Pass,
        "local",
    );
    add(
        "a solid-state partition",
        type_view("xfs", partition(Some(false))),
        Status::Pass,
        "local",
    );
    add(
        "a rotational disk",
        type_view("ext4", disk(Some(true))),
        Status::Warn,
        "spinning",
    );
    add(
        "a partition of a rotational disk",
        type_view("ext4", partition(Some(true))),
        Status::Warn,
        "spinning",
    );
    add(
        "a virtual disk the kernel does not describe",
        type_view("ext4", disk(None)),
        Status::Warn,
        "unreported",
    );
    add(
        "a partition of a virtual disk",
        type_view("ext4", partition(None)),
        Status::Warn,
        "unreported",
    );
    add("nfs", type_view("nfs", None), Status::Fail, "network");
    add(
        "nfs version 4",
        type_view("nfs4", None),
        Status::Fail,
        "network",
    );
    add("cifs", type_view("cifs", None), Status::Fail, "network");
    add("smb", type_view("smb3", None), Status::Fail, "network");
    add(
        "sshfs",
        type_view("fuse.sshfs", None),
        Status::Fail,
        "network",
    );
    add(
        "a network filesystem on a rotational disk",
        type_view("nfs4", disk(Some(true))),
        Status::Fail,
        "network",
    );
    add("tmpfs", type_view("tmpfs", None), Status::Warn, "tmpfs");
    add("ramfs", type_view("ramfs", None), Status::Warn, "tmpfs");
    add(
        "a device nothing is known of",
        type_view("ext4", None),
        Status::Unknown,
        "unknown",
    );
    let mut view = type_view("ext4", None);
    view.block_devices.insert((8, 1), Err(denied(Source::Sys)));
    add(
        "a device that cannot be read",
        view,
        Status::Unknown,
        "unknown",
    );
    let mut view = type_view("tmpfs", None);
    view.mounts = Ok(vec![
        mount(8, 1, "/", "ext4"),
        mount(0, 40, "/nodes/node1/data", "tmpfs"),
    ]);
    add(
        "the mount that holds the data directory",
        view,
        Status::Warn,
        "tmpfs",
    );
    add("mount table unreadable", base(), Status::Unknown, "unknown");
    add(
        "data directory on no mount",
        {
            let mut v = base();
            v.mounts = Ok(Vec::new());
            v
        },
        Status::Unknown,
        "unknown",
    );
    cases
}

/// A view whose forger log holds slow-write lines of `millis`.
fn slow_view(millis: &[u64]) -> FakeView {
    let mut view = base();
    let events: Vec<LogEvent> = millis
        .iter()
        .zip(0_u64..)
        .map(|(millis, offset)| LogEvent {
            kind: LineKind::SigningRecordSlow { millis: *millis },
            level: Level::Warning,
            at_ms: 1_700_000_000_000 + i64::try_from(offset).unwrap() * 1000,
            offset,
            rotated: false,
        })
        .collect();
    let window = LogWindow {
        role: Role::Forger,
        events,
        other_lines: 0,
        untrusted_lines: 0,
        covered: Covered::default(),
        file_level: None,
        excerpts: Vec::new(),
        capped: false,
        roster: Vec::new(),
    };
    view.logs.insert(Role::Forger, Ok(window));
    view
}

/// The forger's slow writes.
fn slow_writes() -> Vec<Case> {
    let id = "disk.slow-writes";
    let mut cases = Vec::new();
    let mut add = |name, view, status, key| {
        cases.push(case(name, id, view, vec![ex(FORGER, status, key)]));
    };
    add("no slow write", slow_view(&[]), Status::Pass, "none");
    add("a write of 249 ms", slow_view(&[249]), Status::Pass, "none");
    add("a write of 250 ms", slow_view(&[250]), Status::Warn, "slow");
    add("a write of 999 ms", slow_view(&[999]), Status::Warn, "slow");
    add(
        "a write of 1000 ms",
        slow_view(&[1000]),
        Status::Fail,
        "slow",
    );
    add(
        "the slowest of several",
        slow_view(&[260, 1_500, 300]),
        Status::Fail,
        "slow",
    );
    add(
        "the slowest of several is mild",
        slow_view(&[10, 260, 300]),
        Status::Warn,
        "slow",
    );
    let mut view = base();
    view.logs
        .insert(Role::Forger, Err(denied(Source::ForgerLog)));
    add("forger log unreadable", view, Status::Unknown, "unknown");
    let mut view = slow_view(&[]);
    if let Some(Ok(window)) = view.logs.get_mut(&Role::Forger) {
        window.file_level = Some(Level::Error);
    }
    add(
        "forger log level hides the warnings",
        view,
        Status::Unknown,
        "unknown",
    );
    let mut view = slow_view(&[]);
    view.roles_present = Ok(presence(true, false));
    add("no forger here", view, Status::Skipped, "skipped-absent");
    let stopped = |millis: &[u64]| {
        let mut view = slow_view(millis);
        let facts = process(RoleState::Stopped, 1_600_000_000_000);
        view.processes.insert(Role::Forger, Ok(facts));
        view
    };
    let (quiet, slow) = (stopped(&[]), stopped(&[300]));
    add(
        "the forger does not run",
        quiet,
        Status::Skipped,
        "skipped-absent",
    );
    add(
        "a slow write before the forger stopped",
        slow,
        Status::Warn,
        "slow",
    );
    cases
}

/// Every case of the area.
pub fn cases() -> Vec<Case> {
    [space(), other(), inodes(), disk_type(), slow_writes()]
        .into_iter()
        .flatten()
        .collect()
}

#[test]
fn every_disk_advice_has_its_template_and_its_values() {
    resolve("disk", cases());
}
