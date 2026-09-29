//! The disk checks: free space and how long it lasts, the other directories' disks, inodes, the
//! kind of disk under the store, and the forger's slow writes.

use crate::doctor::registry::CheckInput;
use crate::doctor::report::{Basis, Fact, Finding, Status};
use crate::doctor::seams::FsStats;
use crate::doctor::sources::logs::{LineKind, LogEvent};
use crate::doctor::sources::procfs::MountRow;
use crate::doctor::view::{NodeDir, NodeFile, Reason, Source, Unknown, View};
use crate::seams::Role;
use heartwood_kernel::log::Level;

use super::net::{graded, int, run};

const MIB: u64 = 1 << 20;
const GIB: u64 = 1 << 30;
/// About 35 GB a year, the planning figure until the chain has enough blocks to measure.
const PLANNING_PER_DAY: u64 = 95_890_410;
/// The chain's height from which the store's own growth is measured.
const MEASURED_FROM: u64 = 10_000;

/// Whether `free` is under `percent` per cent of `total`.
fn under(free: u64, total: u64, percent: u64) -> bool {
    u128::from(free) * 100 < u128::from(total) * u128::from(percent)
}

/// `free` as per cent of `total`.
fn percent_of(free: u64, total: u64) -> u64 {
    u64::try_from(u128::from(free) * 100 / u128::from(total.max(1))).unwrap_or(100)
}

/// The data directory's `statvfs`, when it says how big the filesystem is. Some filesystems
/// (btrfs among them) have no fixed count of file entries and report none.
fn data_stats(view: &dyn View) -> Result<FsStats, Unknown> {
    let stats = view.fs_stats(NodeDir::Data)?;
    let unread = Unknown::new(Source::Files, Reason::Unparsed);
    (stats.total_bytes > 0).then_some(stats).ok_or(unread)
}

/// The mount that holds one of the node's directories.
fn holding<'a>(view: &dyn View, mounts: &'a [MountRow], dir: NodeDir) -> Option<&'a MountRow> {
    MountRow::holding(mounts, &NodeFile::Dir(dir).path(view.scope())?)
}

/// The mounts, and the one that holds the data directory.
fn data_mount(view: &dyn View) -> Result<(&[MountRow], &MountRow), Unknown> {
    let mounts = view.mounts()?;
    let unread = Unknown::new(Source::Proc, Reason::Unparsed);
    Ok((mounts, holding(view, mounts, NodeDir::Data).ok_or(unread)?))
}

/// The bytes a day the store grows by, and how that was worked out: the store's size per block
/// once the chain has 10,000 blocks, the planning figure before.
fn growth(view: &dyn View) -> (u64, &'static str) {
    let measured = || {
        let block_time = u64::from(view.relay_config().ok()?.chain.block_time_s);
        let height = u64::from(view.api_last_block().ok()?.height);
        let file = view.file(NodeFile::Store).ok()??;
        let size = u128::from(file.target.unwrap_or(file.own).size);
        let a_day = 86_400_u64.checked_div(block_time)?;
        let day = u64::try_from(size * u128::from(a_day) / u128::from(height.max(1))).ok();
        day.filter(|day| *day > 0 && height >= MEASURED_FROM)
    };
    let planning = match view.api_last_block() {
        Ok(_) => {
            "the planning figure of about 35 GB a year, used until the chain has 10,000 blocks"
        }
        Err(_) => "the planning figure of about 35 GB a year: the height could not be read",
    };
    measured().map_or((PLANNING_PER_DAY, planning), |day| {
        (day, "measured from the store's size")
    })
}

/// The data disk's free space, and the days it lasts at the store's growth.
pub fn space(input: &CheckInput<'_>) -> Vec<Finding> {
    run(input, (None, Basis::System), || {
        let stats = data_stats(input.view)?;
        let (free, total) = (stats.available_bytes, stats.total_bytes);
        let (per_day, how) = growth(input.view);
        let lasts_under = |days: u128| u128::from(free) < u128::from(per_day) * days;
        let fail = under(free, total, 5) || free < 2 * GIB || lasts_under(14);
        let (status, key) = graded(fail, under(free, total, 15) || lasts_under(180));
        let values = vec![
            ("free_mb", int(free / 1_000_000)),
            ("percent", int(percent_of(free, total))),
            ("days", int(free / per_day.max(1))),
            ("growth_mb", int(per_day / 1_000_000)),
            ("growth", Fact::Word(how)),
        ];
        Ok((status, key, values))
    })
}

/// The free space of the log, temporary and configuration directories that sit on other disks.
pub fn other(input: &CheckInput<'_>) -> Vec<Finding> {
    run(input, (None, Basis::System), || {
        let (mounts, data) = data_mount(input.view)?;
        let mut free = Vec::new();
        let dirs = [
            (NodeDir::Log, "log"),
            (NodeDir::Temp, "temp"),
            (NodeDir::Config, "configuration"),
        ];
        for (dir, name) in dirs {
            let here = holding(input.view, mounts, dir);
            if here.is_some_and(|row| row.mount_point == data.mount_point) {
                continue;
            }
            match input.view.fs_stats(dir) {
                Err(unknown) if unknown.reason == Reason::Missing => {}
                stats => free.push((stats?.available_bytes, name)),
            }
        }
        let Some(&(least, name)) = free.iter().min() else {
            return Ok((Status::Skipped, "same-disk", Vec::new()));
        };
        let (status, key) = graded(least < 100 * MIB, least < GIB);
        let values = vec![("disk", Fact::Word(name)), ("free_mb", int(least / MIB))];
        Ok((status, key, values))
    })
}

/// The data filesystem's free file entries.
pub fn inodes(input: &CheckInput<'_>) -> Vec<Finding> {
    run(input, (None, Basis::System), || {
        let stats = data_stats(input.view)?;
        let (free, total) = (stats.available_inodes, stats.total_inodes);
        if total == 0 {
            return Ok((Status::Pass, "no-count", Vec::new()));
        }
        let (status, key) = graded(under(free, total, 1), under(free, total, 5));
        let values = vec![
            ("percent", int(percent_of(free, total))),
            ("free", int(free)),
        ];
        Ok((status, key, values))
    })
}

/// The kind of disk under the store: local and non-rotational passes.
pub fn disk_type(input: &CheckInput<'_>) -> Vec<Finding> {
    run(input, (None, Basis::System), || {
        let row = data_mount(input.view)?.1;
        let kind = row.fs_type.as_str();
        let network = kind.starts_with("nfs") || matches!(kind, "cifs" | "smb3" | "fuse.sshfs");
        let (status, key) = if network {
            (Status::Fail, "network")
        } else if matches!(kind, "tmpfs" | "ramfs") {
            (Status::Warn, "tmpfs")
        } else {
            match input.view.block_device(row.major, row.minor)?.rotational {
                Some(true) => (Status::Warn, "spinning"),
                Some(false) => (Status::Pass, "local"),
                None => (Status::Warn, "unreported"),
            }
        };
        Ok((status, key, Vec::new()))
    })
}

/// The slowest write of the signing record the forger logged.
pub fn slow_writes(input: &CheckInput<'_>) -> Vec<Finding> {
    run(input, (Some(Role::Forger), Basis::Log), || {
        let log = input.view.log(Role::Forger)?;
        log.shows(Level::Warning)?;
        let slow = |event: &LogEvent| match event.kind {
            LineKind::SigningRecordSlow { millis } => Some(millis),
            _ => None,
        };
        let slowest = log.events.iter().filter_map(slow).max().unwrap_or(0);
        let (status, key) = match slowest {
            1000.. => (Status::Fail, "slow"),
            250..=999 => (Status::Warn, "slow"),
            _ => {
                super::running(input.view, Role::Forger)?;
                (Status::Pass, "none")
            }
        };
        let millis = Fact::Millis(i64::try_from(slowest).unwrap_or(i64::MAX));
        Ok((status, key, vec![("millis", millis)]))
    })
}
