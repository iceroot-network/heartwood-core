//! `/proc` and `/sys`: [`HostProc`], the real [`ProcView`], and the parsers of what it reads.
//! Each parser takes the file's bytes as the kernel writes them.

use std::ffi::OsString;
use std::net::SocketAddr;

use crate::doctor::seams::{FsError, ProcFile, ProcView};
use crate::doctor::sources::ParseError;

/// A TCP socket's state, as `/proc/net/tcp` numbers it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TcpState {
    /// `0A`: listening.
    Listen,
    /// `01`: established.
    Established,
    /// Any other state, by its number.
    Other(u8),
}

/// One row of `/proc/net/tcp` or `/proc/net/tcp6`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SocketRow {
    /// The local address and port; an IPv4-mapped IPv6 address stays IPv6.
    pub local: SocketAddr,
    /// The remote address and port (unspecified for a listener).
    pub remote: SocketAddr,
    /// The state.
    pub state: TcpState,
    /// The user id the socket belongs to.
    pub uid: u32,
    /// The socket's inode, which `/proc/<pid>/fd` links name as `socket:[<inode>]`.
    pub inode: u64,
}

/// `/proc/meminfo`, in bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemInfo {
    /// `MemTotal`.
    pub total: u64,
    /// `MemAvailable`.
    pub available: u64,
    /// `SwapTotal`.
    pub swap_total: u64,
    /// `SwapFree`.
    pub swap_free: u64,
}

/// What the doctor takes from `/proc/<pid>/status` (or `/proc/self/status`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcStatus {
    /// The real user id (`Uid:`'s first field).
    pub uid: u32,
    /// `VmRSS`, in bytes; `None` for a kernel thread.
    pub rss_bytes: Option<u64>,
    /// `VmSwap`, in bytes.
    pub swap_bytes: Option<u64>,
}

/// One row of `/proc/self/mountinfo`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MountRow {
    /// The device's major number.
    pub major: u32,
    /// The device's minor number.
    pub minor: u32,
    /// The mount point, with the kernel's octal escapes (`\040`) decoded.
    pub mount_point: std::path::PathBuf,
    /// The filesystem type (`ext4`, `nfs4`, `fuse.sshfs`, `tmpfs`).
    pub fs_type: String,
}

impl MountRow {
    /// The row of `rows` whose mount point holds `path`: the longest mount point it starts with.
    pub fn holding<'a>(rows: &'a [MountRow], path: &std::path::Path) -> Option<&'a MountRow> {
        rows.iter()
            .filter(|row| path.starts_with(&row.mount_point))
            .max_by_key(|row| row.mount_point.as_os_str().len())
    }
}

/// A block device's kind, from `/sys/dev/block`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockDevice {
    /// Whether it is a partition (then `rotational` is its parent's).
    pub partition: bool,
    /// `queue/rotational`: `Some(true)` for a spinning disk; `None` when the kernel does not say
    /// (a virtual device).
    pub rotational: Option<bool>,
}

/// The real [`ProcView`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HostProc;

impl ProcView for HostProc {
    fn read(&self, file: ProcFile) -> Result<Vec<u8>, FsError> {
        let _ = file;
        Err(FsError::NotBuilt)
    }

    fn fd_targets(&self, pid: u32) -> Result<Vec<OsString>, FsError> {
        let _ = pid;
        Err(FsError::NotBuilt)
    }

    fn pids(&self) -> Result<Vec<u32>, FsError> {
        Err(FsError::NotBuilt)
    }
}

/// The rows of `/proc/net/tcp` (`v6` false) or `/proc/net/tcp6` (`v6` true).
pub fn parse_net_tcp(bytes: &[u8], v6: bool) -> Result<Vec<SocketRow>, ParseError> {
    let _ = (bytes, v6);
    Err(ParseError::NotBuilt)
}

/// The socket inodes among `/proc/<pid>/fd` link targets (`socket:[<inode>]`), in order.
pub fn socket_inodes(targets: &[OsString]) -> Vec<u64> {
    let _ = targets;
    Vec::new()
}

/// `/proc/meminfo`.
pub fn parse_meminfo(bytes: &[u8]) -> Result<MemInfo, ParseError> {
    let _ = bytes;
    Err(ParseError::NotBuilt)
}

/// `/proc/<pid>/status`.
pub fn parse_status(bytes: &[u8]) -> Result<ProcStatus, ParseError> {
    let _ = bytes;
    Err(ParseError::NotBuilt)
}

/// The process's start, in clock ticks since boot, from `/proc/<pid>/stat` (field 22; the command
/// in field 2 may hold spaces and parentheses).
pub fn parse_stat_start(bytes: &[u8]) -> Result<u64, ParseError> {
    let _ = bytes;
    Err(ParseError::NotBuilt)
}

/// `/proc/self/mountinfo`.
pub fn parse_mountinfo(bytes: &[u8]) -> Result<Vec<MountRow>, ParseError> {
    let _ = bytes;
    Err(ParseError::NotBuilt)
}

/// `queue/rotational`: `0` or `1`.
pub fn parse_rotational(bytes: &[u8]) -> Result<bool, ParseError> {
    let _ = bytes;
    Err(ParseError::NotBuilt)
}

/// `/proc/sys/kernel/osrelease` as `<major>.<minor>`.
pub fn parse_os_release(bytes: &[u8]) -> Result<String, ParseError> {
    let _ = bytes;
    Err(ParseError::NotBuilt)
}
