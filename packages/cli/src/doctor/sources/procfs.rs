//! `/proc` and `/sys`: [`HostProc`], the real [`ProcView`], and the parsers of what it reads.
//! Each parser takes the file's bytes as the kernel writes them.

use std::ffi::OsString;
use std::net::{IpAddr, SocketAddr};
use std::os::unix::ffi::OsStringExt;
use std::path::PathBuf;

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
        crate::doctor::sources::fs::read_proc(file)
    }

    fn fd_targets(&self, pid: u32) -> Result<Vec<OsString>, FsError> {
        crate::doctor::sources::fs::proc_fd_targets(pid)
    }

    fn pids(&self) -> Result<Vec<u32>, FsError> {
        crate::doctor::sources::fs::proc_pids()
    }
}

/// `bytes` as text, or [`ParseError::Malformed`] naming `what`.
fn text<'a>(bytes: &'a [u8], what: &'static str) -> Result<&'a str, ParseError> {
    std::str::from_utf8(bytes).map_err(|_| ParseError::Malformed(what))
}

/// The value of the first `key: value` line of `text`.
fn value<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    let line = |line: &'a str| line.split_once(':').filter(|(name, _)| *name == key);
    text.lines().find_map(line).map(|(_, value)| value.trim())
}

/// A `key: <n> kB` line's value, in bytes.
fn kib(text: &str, key: &str) -> Option<u64> {
    let number = value(text, key)?.strip_suffix("kB")?.trim().parse::<u64>();
    number.ok()?.checked_mul(1024)
}

/// An address and port as `/proc/net/tcp` writes them: the address as 32-bit words in the host's
/// byte order, the port in network order, both in hexadecimal.
fn socket(field: &str, v6: bool) -> Option<SocketAddr> {
    let (address, port) = field.split_once(':')?;
    let word = |at: usize| u32::from_str_radix(address.get(at..at + 8)?, 16).ok();
    let words: Option<Vec<u32>> = (0..address.len()).step_by(8).map(word).collect();
    let bytes: Vec<u8> = words?.iter().flat_map(|w| w.to_ne_bytes()).collect();
    let ip = match v6 {
        false => IpAddr::from(<[u8; 4]>::try_from(bytes.as_slice()).ok()?),
        true => IpAddr::from(<[u8; 16]>::try_from(bytes.as_slice()).ok()?),
    };
    Some(SocketAddr::new(ip, u16::from_str_radix(port, 16).ok()?))
}

/// The rows of `/proc/net/tcp` (`v6` false) or `/proc/net/tcp6` (`v6` true).
pub fn parse_net_tcp(bytes: &[u8], v6: bool) -> Result<Vec<SocketRow>, ParseError> {
    let bad = ParseError::Malformed("tcp row");
    let rows = text(bytes, "tcp row")?.lines().map(str::trim);
    let rows = rows.filter(|line| !line.is_empty() && !line.starts_with("sl"));
    let row = |line: &str| {
        let fields: Vec<&str> = line.split_whitespace().collect();
        let field = |at: usize| fields.get(at).copied();
        Some(SocketRow {
            local: socket(field(1)?, v6)?,
            remote: socket(field(2)?, v6)?,
            state: match u8::from_str_radix(field(3)?, 16).ok()? {
                0x0A => TcpState::Listen,
                0x01 => TcpState::Established,
                other => TcpState::Other(other),
            },
            uid: field(7)?.parse().ok()?,
            inode: field(9)?.parse().ok()?,
        })
    };
    rows.map(|line| row(line).ok_or(bad)).collect()
}

/// The socket inodes among `/proc/<pid>/fd` link targets (`socket:[<inode>]`), in order.
pub fn socket_inodes(targets: &[OsString]) -> Vec<u64> {
    let inode = |target: &OsString| {
        let inode = target.to_str()?.strip_prefix("socket:[")?;
        inode.strip_suffix(']')?.parse().ok()
    };
    targets.iter().filter_map(inode).collect()
}

/// `/proc/meminfo`.
pub fn parse_meminfo(bytes: &[u8]) -> Result<MemInfo, ParseError> {
    let text = text(bytes, "meminfo")?;
    let get = |key: &str| kib(text, key).ok_or(ParseError::Malformed("meminfo"));
    Ok(MemInfo {
        total: get("MemTotal")?,
        available: get("MemAvailable")?,
        swap_total: get("SwapTotal")?,
        swap_free: get("SwapFree")?,
    })
}

/// `/proc/<pid>/status`.
pub fn parse_status(bytes: &[u8]) -> Result<ProcStatus, ParseError> {
    let text = text(bytes, "status")?;
    let uid = value(text, "Uid").and_then(|uids| uids.split_whitespace().next()?.parse().ok());
    Ok(ProcStatus {
        uid: uid.ok_or(ParseError::Malformed("status"))?,
        rss_bytes: kib(text, "VmRSS"),
        swap_bytes: kib(text, "VmSwap"),
    })
}

/// The process's start, in clock ticks since boot, from `/proc/<pid>/stat` (field 22; the command
/// in field 2 may hold spaces and parentheses).
pub fn parse_stat_start(bytes: &[u8]) -> Result<u64, ParseError> {
    let bad = ParseError::Malformed("stat");
    let (_, fields) = text(bytes, "stat")?.rsplit_once(')').ok_or(bad)?;
    let start = fields.split_whitespace().nth(19).ok_or(bad)?;
    start.parse().map_err(|_| bad)
}

/// A mountinfo field with the kernel's octal escapes (`\040` for a space) decoded.
fn unescape(field: &[u8]) -> Vec<u8> {
    let (mut out, mut at) = (Vec::with_capacity(field.len()), 0);
    while let Some(&byte) = field.get(at) {
        let digits = field.get(at + 1..at + 4).filter(|_| byte == b'\\');
        let code = digits.and_then(|d| u8::from_str_radix(std::str::from_utf8(d).ok()?, 8).ok());
        out.push(code.unwrap_or(byte));
        at += if code.is_some() { 4 } else { 1 };
    }
    out
}

/// `/proc/self/mountinfo`.
pub fn parse_mountinfo(bytes: &[u8]) -> Result<Vec<MountRow>, ParseError> {
    let row = |line: &[u8]| {
        let fields: Vec<&[u8]> = line.split(|b| *b == b' ').collect();
        let dash = 6 + fields.iter().skip(6).position(|field| *field == b"-")?;
        let device = std::str::from_utf8(fields.get(2)?).ok()?.split_once(':')?;
        Some(MountRow {
            major: device.0.parse().ok()?,
            minor: device.1.parse().ok()?,
            mount_point: PathBuf::from(OsString::from_vec(unescape(fields.get(4)?))),
            fs_type: std::str::from_utf8(fields.get(dash + 1)?).ok()?.to_owned(),
        })
    };
    let (bad, lines) = (
        ParseError::Malformed("mountinfo"),
        bytes.split(|b| *b == b'\n'),
    );
    let lines = lines.filter(|line| !line.is_empty());
    lines.map(|line| row(line).ok_or(bad)).collect()
}

/// `queue/rotational`: `0` or `1`.
pub fn parse_rotational(bytes: &[u8]) -> Result<bool, ParseError> {
    match bytes.trim_ascii() {
        b"0" => Ok(false),
        b"1" => Ok(true),
        _ => Err(ParseError::Malformed("rotational")),
    }
}

/// `/proc/sys/kernel/osrelease` as `<major>.<minor>`.
pub fn parse_os_release(bytes: &[u8]) -> Result<String, ParseError> {
    let number = |part: &&str| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit());
    let mut parts = text(bytes, "osrelease")?.trim().split(['.', '-']);
    let release = parts.next().filter(number).zip(parts.next().filter(number));
    let (major, minor) = release.ok_or(ParseError::Malformed("osrelease"))?;
    Ok(format!("{major}.{minor}"))
}

/// Whether a `/proc/<pid>/cmdline` is `heartwood` (by the program's file name) running `doctor
/// mcp` or `doctor:mcp`.
pub fn is_doctor_mcp(cmdline: &[u8]) -> bool {
    let args: Vec<&[u8]> = cmdline.split(|b| *b == 0).collect();
    let program = args.first().map(|arg| arg.rsplit(|b| *b == b'/').next());
    let doctor = |pair: &[&[u8]]| pair == [b"doctor".as_slice(), b"mcp"];
    let mcp = args.windows(2).any(doctor) || args.contains(&b"doctor:mcp".as_slice());
    mcp && program == Some(Some(b"heartwood".as_slice()))
}

/// The number of `heartwood doctor mcp` servers of the user `uid` running now ([`is_doctor_mcp`]),
/// this process included when it is one. Only the command lines of `uid`'s own processes are
/// read; a process that ends meanwhile is not counted.
pub fn doctor_mcp_servers(proc: &dyn ProcView, uid: u32) -> Result<usize, FsError> {
    let server = |pid: u32| {
        let status = proc.read(ProcFile::PidStatus(pid));
        let owned = matches!(status.map(|b| parse_status(&b)), Ok(Ok(s)) if s.uid == uid);
        owned && matches!(proc.read(ProcFile::PidCmdline(pid)), Ok(line) if is_doctor_mcp(&line))
    };
    Ok(proc.pids()?.into_iter().filter(|pid| server(*pid)).count())
}
