//! The fixed probes: [`HostProbes`], the real [`Probes`], and the parsers of their output and of
//! the two files the clock and firewall checks read.
//!
//! A probe runs one program of [`Probe::programs`] by absolute path, with [`Probe::args`] and the
//! environment [`Probe::ENV`] only, reads at most [`Probe::MAX_OUTPUT`] bytes of its output, and
//! is killed after [`Probe::TIMEOUT`].

use std::io::{ErrorKind, Read};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use nix::fcntl::{FcntlArg, OFlag, fcntl};

use crate::doctor::seams::{Probe, ProbeError, ProbeOutput, Probes};
use crate::doctor::sources::ParseError;

/// `timedatectl show --property=NTPSynchronized --property=NTP`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NtpState {
    /// `NTPSynchronized`: the clock is synchronized.
    pub synchronized: Option<bool>,
    /// `NTP`: a time service is enabled.
    pub ntp: Option<bool>,
}

/// chrony's leap status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeapStatus {
    /// `Normal`.
    Normal,
    /// `Insert second` or `Delete second`.
    Pending,
    /// `Not synchronised`.
    NotSynchronised,
}

/// `chronyc -n tracking`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChronyTracking {
    /// `System time`: how far the clock is from NTP time, in microseconds, fast positive.
    pub offset_us: i64,
    /// `Leap status`.
    pub leap: LeapStatus,
}

/// One unit's answer to `systemctl is-active`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceState {
    /// `active`.
    Active,
    /// `activating` or `reloading`.
    Activating,
    /// `inactive`, or a unit that does not exist.
    Inactive,
    /// `failed`.
    Failed,
    /// Another answer.
    Other,
}

/// `systemctl is-active ufw nftables firewalld`, one line per unit in that order; its exit code
/// is non-zero whenever a unit is not active, which is not an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FirewallServices {
    /// `ufw`.
    pub ufw: ServiceState,
    /// `nftables`.
    pub nftables: ServiceState,
    /// `firewalld`.
    pub firewalld: ServiceState,
}

/// `/etc/ufw/ufw.conf`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UfwConf {
    /// `ENABLED=yes`.
    pub enabled: bool,
}

/// The real [`Probes`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HostProbes;

impl Probes for HostProbes {
    fn run(&self, probe: Probe) -> Result<ProbeOutput, ProbeError> {
        for program in probe.programs() {
            let limits = (Probe::TIMEOUT, Probe::MAX_OUTPUT);
            match run_program(Path::new(program), probe.args(), limits) {
                Err(ProbeError::Missing) => continue,
                result => return result,
            }
        }
        Err(ProbeError::Missing)
    }
}

/// Run `program` (an absolute path, never searched for) with `args`, the environment
/// [`Probe::ENV`] alone, standard input and error on the null device, reading at most `limits.1`
/// bytes of its standard output (the rest is read and dropped). It is killed and reaped when it
/// runs longer than `limits.0`. The doctor calls it only through [`HostProbes`], with a probe's
/// fixed program and arguments; it is public for the probes' tests.
pub fn run_program(
    program: &Path,
    args: &[&str],
    limits: (Duration, usize),
) -> Result<ProbeOutput, ProbeError> {
    let failed = |error: std::io::Error| ProbeError::Failed(error.kind());
    let mut command = Command::new(program);
    command.args(args).env_clear().envs(Probe::ENV);
    command.current_dir("/").stdin(Stdio::null());
    command.stdout(Stdio::piped()).stderr(Stdio::null());
    let mut child = match command.spawn() {
        Err(error) if error.kind() == ErrorKind::NotFound => return Err(ProbeError::Missing),
        spawned => spawned.map_err(failed)?,
    };
    let stdout = collect(&mut child, limits);
    if stdout.is_err() {
        let _ = child.kill();
    }
    let status = child.wait().map_err(failed)?.code();
    stdout.map(|stdout| ProbeOutput { status, stdout })
}

/// The child's standard output once it has exited, read without blocking so that a program that
/// never exits, or never closes its output, is found at the deadline.
fn collect(child: &mut Child, (timeout, max): (Duration, usize)) -> Result<Vec<u8>, ProbeError> {
    let failed = |error: std::io::Error| ProbeError::Failed(error.kind());
    let broken = ProbeError::Failed(ErrorKind::BrokenPipe);
    let mut pipe = child.stdout.take().ok_or(broken)?;
    let nonblocking = fcntl(&pipe, FcntlArg::F_SETFL(OFlag::O_NONBLOCK));
    nonblocking.map_err(|errno| failed(errno.into()))?;
    let (deadline, mut out, mut chunk, mut open) =
        (Instant::now() + timeout, Vec::new(), [0; 8192], true);
    loop {
        let exited = child.try_wait().map_err(failed)?.is_some();
        while open {
            match pipe.read(&mut chunk) {
                Ok(0) => open = false,
                Ok(n) => out.extend(chunk.iter().take(n.min(max.saturating_sub(out.len())))),
                Err(error) if error.kind() == ErrorKind::Interrupted => {}
                Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                Err(error) => return Err(failed(error)),
            }
        }
        if exited {
            return Ok(out);
        }
        if Instant::now() >= deadline {
            return Err(ProbeError::TimedOut);
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// `stdout` as text, or [`ParseError::Malformed`] naming `what`.
fn text<'a>(stdout: &'a [u8], what: &'static str) -> Result<&'a str, ParseError> {
    std::str::from_utf8(stdout).map_err(|_| ParseError::Malformed(what))
}

/// `timedatectl`'s output (systemd 249 to 255): `NTP=yes|no` and `NTPSynchronized=yes|no`
/// lines, in either order; an output with neither is malformed.
pub fn parse_timedatectl(stdout: &[u8]) -> Result<NtpState, ParseError> {
    let text = text(stdout, "timedatectl")?;
    let flag = |key: &str| {
        let value = text
            .lines()
            .find_map(|line| line.strip_prefix(key)?.strip_prefix('='));
        value.and_then(|value| ["no", "yes"].iter().position(|word| *word == value.trim()))
    };
    let (synchronized, ntp) = (flag("NTPSynchronized"), flag("NTP"));
    match (synchronized, ntp) {
        (None, None) => Err(ParseError::Malformed("timedatectl")),
        _ => Ok(NtpState {
            synchronized: synchronized.map(|at| at == 1),
            ntp: ntp.map(|at| at == 1),
        }),
    }
}

/// `chronyc -n tracking`'s output (chrony 4.2 to 4.5): `System time : <s> seconds fast|slow of
/// NTP time` and `Leap status : <status>`.
pub fn parse_chrony_tracking(stdout: &[u8]) -> Result<ChronyTracking, ParseError> {
    let text = text(stdout, "chronyc")?;
    let field = |key: &str| {
        let pairs = text.lines().filter_map(|line| line.split_once(':'));
        pairs.fold(None, |found, (name, value)| {
            found.or((name.trim() == key).then(|| value.trim()))
        })
    };
    let offset = field("System time").and_then(|value| {
        let words: Vec<&str> = value.split_whitespace().collect();
        let seconds = words.first()?.parse::<f64>().ok()?;
        let sign = match words.get(1..3)? {
            ["seconds", "fast"] => 1.0,
            ["seconds", "slow"] => -1.0,
            _ => return None,
        };
        seconds
            .is_finite()
            .then(|| (seconds * sign * 1e6).round() as i64)
    });
    let leap = match field("Leap status") {
        Some("Normal") => Some(LeapStatus::Normal),
        Some("Insert second" | "Delete second") => Some(LeapStatus::Pending),
        Some("Not synchronised") => Some(LeapStatus::NotSynchronised),
        _ => None,
    };
    match (offset, leap) {
        (Some(offset_us), Some(leap)) => Ok(ChronyTracking { offset_us, leap }),
        _ => Err(ParseError::Malformed("chronyc")),
    }
}

/// `systemctl is-active ufw nftables firewalld`'s output: one state per unit, in that order.
pub fn parse_is_active(stdout: &[u8]) -> Result<FirewallServices, ParseError> {
    let text = text(stdout, "systemctl")?;
    let states = text.lines().map(|line| match line.trim() {
        "active" => ServiceState::Active,
        "activating" | "reloading" | "refreshing" => ServiceState::Activating,
        "inactive" | "unknown" => ServiceState::Inactive,
        "failed" => ServiceState::Failed,
        _ => ServiceState::Other,
    });
    match states.collect::<Vec<_>>().as_slice() {
        &[ufw, nftables, firewalld] => Ok(FirewallServices {
            ufw,
            nftables,
            firewalld,
        }),
        _ => Err(ParseError::Malformed("systemctl")),
    }
}

/// `/etc/ufw/ufw.conf`, read as the shell reads it: the last `ENABLED=` assignment wins; none
/// means not enabled.
pub fn parse_ufw_conf(bytes: &[u8]) -> Result<UfwConf, ParseError> {
    let text = String::from_utf8_lossy(bytes);
    let mut lines = text.lines().rev();
    let value = lines.find_map(|line| line.trim().strip_prefix("ENABLED="));
    let value = value.map(|value| value.trim().trim_matches(['"', '\'']));
    let enabled = value.is_some_and(|value| value.eq_ignore_ascii_case("yes"));
    Ok(UfwConf { enabled })
}
