//! The process table of `status`: a running node's CPU use and resident memory, from `/proc` on
//! Linux (from `/bin/ps`, with an explicit argv, elsewhere), and the table's text.

use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use crate::errors::ProcessError;
use crate::seams::StatusRow;
use crate::supervisor::at;

/// The unit of the times in `/proc/<pid>/stat`: the kernel's `USER_HZ`, 100 on every
/// architecture a node runs on.
const USER_HZ: f64 = 100.0;

/// A running process's resource use.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sample {
    /// CPU use since the process started, in percent of one core.
    pub cpu_percent: f64,
    /// Resident memory, in bytes.
    pub rss_bytes: u64,
}

/// The resource use of `pid`.
pub fn sample(pid: u32) -> Result<Sample, ProcessError> {
    if cfg!(target_os = "linux") {
        from_proc(pid)
    } else {
        from_ps(pid)
    }
}

/// The resource use of `pid` from `/proc`: user and system time over the process's age, and
/// `VmRSS`.
pub fn from_proc(pid: u32) -> Result<Sample, ProcessError> {
    let read = |path: String| at(Path::new(&path), fs::read_to_string(&path));
    let stat = read(format!("/proc/{pid}/stat"))?;
    // The fields after the command name, which is in parentheses and may hold anything; the
    // first of them is field 3.
    let fields: Vec<f64> = stat
        .rsplit_once(") ")
        .map(|(_, rest)| rest.split(' ').map(|f| f.parse().unwrap_or(0.0)).collect())
        .unwrap_or_default();
    let field = |n: usize| fields.get(n - 3).copied().unwrap_or(0.0) / USER_HZ;
    let uptime = read("/proc/uptime".into())?;
    let uptime: f64 = uptime
        .split(' ')
        .next()
        .and_then(|up| up.parse().ok())
        .unwrap_or(0.0);
    let age = uptime - field(22);
    let status = read(format!("/proc/{pid}/status"))?;
    let rss = status.lines().find_map(|line| line.strip_prefix("VmRSS:"));
    let rss_kb: u64 = rss
        .and_then(|kb| kb.trim().trim_end_matches(" kB").parse().ok())
        .unwrap_or(0);
    Ok(Sample {
        cpu_percent: if age > 0.0 {
            (field(14) + field(15)) / age * 100.0
        } else {
            0.0
        },
        rss_bytes: rss_kb.saturating_mul(1024),
    })
}

/// The resource use of `pid` from `/bin/ps -o %cpu=,rss= -p <pid>`.
pub fn from_ps(pid: u32) -> Result<Sample, ProcessError> {
    let ps = Path::new("/bin/ps");
    let output = Command::new(ps)
        .args(["-o", "%cpu=,rss=", "-p", &pid.to_string()])
        .output();
    let output = String::from_utf8_lossy(&at(ps, output)?.stdout).into_owned();
    match output.split_whitespace().collect::<Vec<_>>().as_slice() {
        [cpu, kb] => Ok(Sample {
            cpu_percent: cpu.parse().unwrap_or(0.0),
            rss_bytes: kb.parse::<u64>().unwrap_or(0).saturating_mul(1024),
        }),
        _ => at(ps, Err(std::io::Error::other(format!("no process {pid}")))),
    }
}

/// The table of `rows`, with its header: ID, Name, Version, Status, Uptime, Restarts, CPU, RAM.
pub fn render(rows: &[StatusRow]) -> String {
    let header = [
        "ID", "Name", "Version", "Status", "Uptime", "Restarts", "CPU", "RAM",
    ];
    let mut table = vec![header.map(str::to_owned)];
    for row in rows {
        let or_dash = |cell: Option<String>| cell.unwrap_or_else(|| "-".into());
        table.push([
            or_dash(row.pid.map(|pid| pid.to_string())),
            row.name.clone(),
            row.version.clone(),
            row.status.name().to_owned(),
            or_dash(row.uptime.map(pretty_time)),
            row.restarts.to_string(),
            or_dash(row.cpu_percent.map(|cpu| format!("{cpu:.1}%"))),
            or_dash(row.rss_bytes.map(pretty_bytes)),
        ]);
    }
    let widths: [usize; 8] = std::array::from_fn(|column| {
        let cells = table.iter().filter_map(|row| row.get(column));
        cells.map(|cell| cell.chars().count()).max().unwrap_or(0)
    });
    let line = |row: &[String; 8]| {
        let cells = row.iter().zip(widths).map(|(cell, w)| format!("{cell:w$}"));
        format!("{}\n", cells.collect::<Vec<_>>().join("  ").trim_end())
    };
    table.iter().map(line).collect()
}

/// `1d 2h`, `2h 3m`, `3m 4s` or `5s`.
fn pretty_time(duration: Duration) -> String {
    let s = duration.as_secs();
    match (s / 86_400, s / 3_600 % 24, s / 60 % 60) {
        (0, 0, 0) => format!("{s}s"),
        (0, 0, m) => format!("{m}m {}s", s % 60),
        (0, h, m) => format!("{h}h {m}m"),
        (d, h, _) => format!("{d}d {h}h"),
    }
}

/// Bytes in decimal units, as `pretty-bytes` writes them: `512 B`, `52.3 MB`.
fn pretty_bytes(bytes: u64) -> String {
    let (mut value, mut unit) = (bytes as f64, "B");
    for next in ["kB", "MB", "GB", "TB"] {
        if value < 1000.0 {
            break;
        }
        (value, unit) = (value / 1000.0, next);
    }
    match unit {
        "B" => format!("{bytes} B"),
        _ => format!("{value:.1} {unit}"),
    }
}
