//! The fixed probes: [`HostProbes`], the real [`Probes`], and the parsers of their output and of
//! the two files the clock and firewall checks read.
//!
//! A probe runs one program of [`Probe::programs`] by absolute path, with [`Probe::args`] and the
//! environment [`Probe::ENV`] only, reads at most [`Probe::MAX_OUTPUT`] bytes of its output, and
//! is killed after [`Probe::TIMEOUT`].

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
        let _ = probe;
        Err(ProbeError::NotBuilt)
    }
}

/// `timedatectl`'s output (systemd 249 to 255).
pub fn parse_timedatectl(stdout: &[u8]) -> Result<NtpState, ParseError> {
    let _ = stdout;
    Err(ParseError::NotBuilt)
}

/// `chronyc -n tracking`'s output (chrony 4.2 to 4.5).
pub fn parse_chrony_tracking(stdout: &[u8]) -> Result<ChronyTracking, ParseError> {
    let _ = stdout;
    Err(ParseError::NotBuilt)
}

/// `systemctl is-active ufw nftables firewalld`'s output.
pub fn parse_is_active(stdout: &[u8]) -> Result<FirewallServices, ParseError> {
    let _ = stdout;
    Err(ParseError::NotBuilt)
}

/// `/etc/ufw/ufw.conf`.
pub fn parse_ufw_conf(bytes: &[u8]) -> Result<UfwConf, ParseError> {
    let _ = bytes;
    Err(ParseError::NotBuilt)
}
