//! Files: [`Readable`], the only path [`DoctorFs::read`] and [`DoctorFs::tail`] accept;
//! [`KeyFile`], which yields metadata and nothing else; [`HostFs`], the real [`DoctorFs`]; and the
//! signing record's parser.
//!
//! [`Readable::new`] refuses a path that is not absolute, that has a `.` or `..` component, whose
//! file name is on [`KEY_FILE_NAMES`] in any letter case, or that is outside [`ReadableRoots`]:
//! the instance's five directories and exactly two files the firewall and clock checks read. The
//! fixed files of `/proc` and `/sys` are read through [`ProcFile`] alone, never as a `Readable`.
//! A name check alone can be beaten by a link with an innocent name, so [`HostFs`] opens every
//! file with `O_NOFOLLOW`, takes the open descriptor's device and inode, and closes it unread when
//! they are a key file's ([`FsError::KeyFile`]).

use std::ffi::OsString;
use std::fs::{File, Metadata, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Component, Path, PathBuf};

use heartwood_kernel::paths::Paths;
use nix::fcntl::OFlag;
use serde_json::Value;

use crate::doctor::report::{Id, IdKind};
use crate::doctor::seams::{DoctorFs, FileFacts, FileKind, FsError, FsStats, Meta, ProcFile, Tail};
use crate::doctor::sources::ParseError;

/// The file names the node loads as key files, never opened by the doctor. The forger's key file
/// is the first; the list grows with the node's key files.
pub const KEY_FILE_NAMES: &[&str] = &["delegates.json"];

/// The two files outside the instance's directories that the doctor reads.
pub const READABLE_FILES: [&str; 2] = ["/etc/ufw/ufw.conf", "/run/systemd/timesync/synchronized"];

/// Where the doctor may read: directories whose descendants it may read, and single files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadableRoots {
    dirs: Vec<PathBuf>,
    files: Vec<PathBuf>,
}

impl ReadableRoots {
    /// The roots of an instance with the directories `paths`: its five directories and
    /// [`READABLE_FILES`].
    pub fn for_paths(paths: &Paths) -> ReadableRoots {
        let own = [
            &paths.config,
            &paths.data,
            &paths.cache,
            &paths.log,
            &paths.temp,
        ];
        ReadableRoots {
            dirs: own.into_iter().cloned().collect(),
            files: READABLE_FILES.map(PathBuf::from).to_vec(),
        }
    }

    /// Whether `path` (absolute, without `.` or `..`) is under a root directory or is a root file.
    pub fn admits(&self, path: &Path) -> bool {
        self.files.iter().any(|file| file == path)
            || self.dirs.iter().any(|dir| path.starts_with(dir))
    }
}

/// Why [`Readable::new`] refused a path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RefusedPath {
    /// The path is relative.
    #[error("the path is not absolute")]
    NotAbsolute,
    /// The path has a `.` or `..` component.
    #[error("the path has a . or .. component")]
    DotComponent,
    /// The file name is a key file's.
    #[error("the path names a key file")]
    KeyFileName,
    /// The path is outside the readable roots.
    #[error("the path is outside the instance's directories")]
    OutsideRoots,
}

/// A path the doctor may open and read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Readable(PathBuf);

impl Readable {
    /// `path` as a readable path under `roots`, or why not.
    pub fn new(path: &Path, roots: &ReadableRoots) -> Result<Readable, RefusedPath> {
        if !path.is_absolute() {
            return Err(RefusedPath::NotAbsolute);
        }
        let plain = |c: Component<'_>| matches!(c, Component::RootDir | Component::Normal(_));
        if !path.components().all(plain) {
            return Err(RefusedPath::DotComponent);
        }
        if is_key_file_name(path) {
            return Err(RefusedPath::KeyFileName);
        }
        if !roots.admits(path) {
            return Err(RefusedPath::OutsideRoots);
        }
        Ok(Readable(path.to_path_buf()))
    }

    /// The path.
    pub fn path(&self) -> &Path {
        &self.0
    }
}

/// Whether the file name of `path` is on [`KEY_FILE_NAMES`], in any letter case.
pub fn is_key_file_name(path: &Path) -> bool {
    let name = path.file_name().and_then(|name| name.to_str());
    name.is_some_and(|name| {
        KEY_FILE_NAMES
            .iter()
            .any(|key| key.eq_ignore_ascii_case(name))
    })
}

/// A key file's path. It has no method that yields the file's bytes: only its metadata.
///
/// ```no_run
/// # use heartwood_cli::doctor::seams::{DoctorFs, KeyFile};
/// fn file_uid(key: &KeyFile, fs: &dyn DoctorFs) -> Option<u32> {
///     key.facts(fs).ok().map(|facts| facts.own.uid)
/// }
/// ```
///
/// Neither the doctor's reads nor the standard library's accept one:
///
/// ```compile_fail
/// # use heartwood_cli::doctor::seams::{DoctorFs, KeyFile};
/// fn bytes(key: &KeyFile, fs: &dyn DoctorFs) -> Vec<u8> {
///     fs.read(key, 1024).unwrap_or_default()
/// }
/// ```
///
/// ```compile_fail
/// # use heartwood_cli::doctor::seams::KeyFile;
/// fn bytes(key: &KeyFile) -> Vec<u8> {
///     std::fs::read(key).unwrap_or_default()
/// }
/// ```
///
/// ```compile_fail
/// # use heartwood_cli::doctor::seams::{KeyFile, Readable};
/// fn readable(key: KeyFile) -> Readable {
///     Readable::from(key)
/// }
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyFile(PathBuf);

impl KeyFile {
    /// The key file at `path`.
    pub fn new(path: PathBuf) -> KeyFile {
        KeyFile(path)
    }

    /// The path, to name it in a finding.
    pub fn path(&self) -> &Path {
        &self.0
    }

    /// The file's metadata (`lstat`, `stat`, `readlink`); never opens it.
    pub fn facts(&self, fs: &dyn DoctorFs) -> io::Result<FileFacts> {
        fs.facts(&self.0)
    }
}

/// The real [`DoctorFs`] over the host's filesystem, refusing every descriptor that is one of
/// `key_files`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostFs {
    key_files: Vec<KeyFile>,
}

impl HostFs {
    /// The filesystem, with the key files whose identity every open is checked against.
    pub fn new(key_files: Vec<KeyFile>) -> HostFs {
        HostFs { key_files }
    }

    /// The key files.
    pub fn key_files(&self) -> &[KeyFile] {
        &self.key_files
    }

    /// `file` opened with [`open_plain`], refused and closed unread when the open descriptor has
    /// the device and inode of a key file or of its link's target. A key file whose identity
    /// cannot be read (other than a missing one) refuses every open.
    fn open(&self, file: &Readable) -> Result<(File, Metadata), FsError> {
        let (handle, meta) = open_plain(file.path())?;
        let same = |key: &Meta| key.dev == meta.dev() && key.ino == meta.ino();
        for key in &self.key_files {
            match key.facts(self) {
                Ok(facts) if same(&facts.own) || facts.target.as_ref().is_some_and(same) => {
                    return Err(FsError::KeyFile);
                }
                Err(error) if error.kind() != io::ErrorKind::NotFound => {
                    return Err(io_error(error));
                }
                _ => {}
            }
        }
        Ok((handle, meta))
    }
}

impl DoctorFs for HostFs {
    fn facts(&self, path: &Path) -> io::Result<FileFacts> {
        let own = meta_of(&std::fs::symlink_metadata(path)?);
        let link = own.kind == FileKind::Symlink;
        let target = link.then(|| std::fs::metadata(path).ok()).flatten();
        Ok(FileFacts {
            path: path.to_path_buf(),
            own,
            target: target.as_ref().map(meta_of),
            link_target: link.then(|| std::fs::read_link(path).ok()).flatten(),
        })
    }

    fn read(&self, file: &Readable, max: u64) -> Result<Vec<u8>, FsError> {
        self.open(file)
            .and_then(|(handle, _)| read_bounded(handle, max))
    }

    fn tail(&self, file: &Readable, max: u64) -> Result<Tail, FsError> {
        let (mut handle, meta) = self.open(file)?;
        let (len, mut bytes) = (meta.len(), Vec::new());
        let start = len.saturating_sub(max);
        handle.seek(SeekFrom::Start(start)).map_err(io_error)?;
        let read = handle.take(len - start).read_to_end(&mut bytes);
        read.map_err(io_error)?;
        Ok(Tail { bytes, start, len })
    }

    fn list(&self, dir: &Readable) -> Result<Vec<OsString>, FsError> {
        let own = std::fs::symlink_metadata(dir.path()).map_err(io_error)?;
        match own.is_dir() {
            true => list_names(dir.path()),
            false => Err(FsError::Io(io::ErrorKind::NotADirectory)),
        }
    }

    fn stats(&self, path: &Path) -> Result<FsStats, FsError> {
        let stats = nix::sys::statvfs::statvfs(path);
        let stats = stats.map_err(|errno| io_error(errno.into()))?;
        let block = wide(stats.fragment_size());
        Ok(FsStats {
            total_bytes: wide(stats.blocks()).saturating_mul(block),
            available_bytes: wide(stats.blocks_available()).saturating_mul(block),
            total_inodes: wide(stats.files()),
            available_inodes: wide(stats.files_available()),
        })
    }
}

/// A count of any unsigned width as `u64`.
fn wide(count: impl Into<u64>) -> u64 {
    count.into()
}

fn io_error(error: io::Error) -> FsError {
    FsError::Io(error.kind())
}

/// The doctor's view of `meta`.
fn meta_of(meta: &Metadata) -> Meta {
    let kind = meta.file_type();
    let kind = match (kind.is_symlink(), kind.is_dir(), kind.is_file()) {
        (true, _, _) => FileKind::Symlink,
        (_, true, _) => FileKind::Dir,
        (_, _, true) => FileKind::File,
        _ => FileKind::Other,
    };
    let modified_ms = meta.mtime().saturating_mul(1000);
    Meta {
        kind,
        mode: meta.mode() & 0o7777,
        uid: meta.uid(),
        gid: meta.gid(),
        size: meta.size(),
        dev: meta.dev(),
        ino: meta.ino(),
        nlink: meta.nlink(),
        modified_ms: modified_ms.saturating_add(meta.mtime_nsec() / 1_000_000),
    }
}

/// `path` opened for reading without following a final symbolic link (a link fails to open),
/// without waiting on a pipe or becoming a terminal's, and only when the descriptor is a regular
/// file.
fn open_plain(path: &Path) -> Result<(File, Metadata), FsError> {
    let flags = OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK | OFlag::O_NOCTTY;
    let mut options = OpenOptions::new();
    let handle = options.read(true).custom_flags(flags.bits()).open(path);
    let handle = handle.map_err(io_error)?;
    let meta = handle.metadata().map_err(io_error)?;
    match meta.is_file() {
        true => Ok((handle, meta)),
        false => Err(FsError::Io(io::ErrorKind::InvalidInput)),
    }
}

/// The rest of `handle`, refused when longer than `max` bytes (a file of `/proc` says its size
/// is 0, so the length is what is read).
fn read_bounded(handle: File, max: u64) -> Result<Vec<u8>, FsError> {
    let mut bytes = Vec::new();
    let read = handle.take(max.saturating_add(1)).read_to_end(&mut bytes);
    read.map_err(io_error)?;
    match u64::try_from(bytes.len()).map_or(true, |len| len > max) {
        true => Err(FsError::TooLarge { max }),
        false => Ok(bytes),
    }
}

/// The names in the directory `path`, sorted.
fn list_names(path: &Path) -> Result<Vec<OsString>, FsError> {
    let entries = std::fs::read_dir(path).map_err(io_error)?;
    let names = entries.map(|entry| entry.map(|entry| entry.file_name()));
    let mut names = names.collect::<Result<Vec<_>, _>>().map_err(io_error)?;
    names.sort();
    Ok(names)
}

/// The numeric names in the directory `path`, in numeric order.
fn numbers(path: &Path) -> Result<Vec<u32>, FsError> {
    let names = list_names(path)?;
    let numbers = names.iter().filter_map(|n| n.to_str()?.parse().ok());
    let mut numbers: Vec<u32> = numbers.collect();
    numbers.sort_unstable();
    Ok(numbers)
}

/// The most a file of `/proc` or `/sys` gives.
pub const MAX_PROC: u64 = 4 * 1024 * 1024;

/// One of the fixed files of `/proc` and `/sys`, opened as [`HostFs`] opens a file; the only
/// path `crate::doctor::sources::procfs::HostProc` reads.
pub(crate) fn read_proc(file: ProcFile) -> Result<Vec<u8>, FsError> {
    let (handle, _) = open_plain(&file.path())?;
    read_bounded(handle, MAX_PROC)
}

/// The targets of `/proc/<pid>/fd/*` (`readlink` only), in descriptor order; a descriptor closed
/// meanwhile is left out.
pub(crate) fn proc_fd_targets(pid: u32) -> Result<Vec<OsString>, FsError> {
    let dir = PathBuf::from(format!("/proc/{pid}/fd"));
    let links = numbers(&dir)?.into_iter();
    let links = links.filter_map(|fd| std::fs::read_link(dir.join(fd.to_string())).ok());
    Ok(links.map(PathBuf::into_os_string).collect())
}

/// The pids in `/proc`, in order.
pub(crate) fn proc_pids() -> Result<Vec<u32>, FsError> {
    numbers(Path::new("/proc"))
}

/// The longest signing record read.
pub const MAX_SIGNING_RECORD: u64 = 16 * 1024 * 1024;

/// One key's entry of the signing record: public chain data only; the block's bytes are dropped
/// while parsing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedEntry {
    /// The delegate's public key, 66 lowercase hex digits.
    pub public_key: String,
    /// The height of the last block the forger signed with this key.
    pub height: u32,
    /// Its slot.
    pub slot: i64,
    /// Its id, 64 lowercase hex digits.
    pub block_id: String,
}

/// The forger's signing record (`heartwood-signing-record/1`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SigningRecordFacts {
    /// Whether `format` is `heartwood-signing-record/1`.
    pub known_format: bool,
    /// The network's nethash the record belongs to.
    pub nethash: String,
    /// The entries, by public key.
    pub entries: Vec<SignedEntry>,
}

/// The format a signing record names itself with.
pub const SIGNING_RECORD_FORMAT: &str = "heartwood-signing-record/1";

/// The signing record in `bytes`; the blocks' bytes are not kept. A record of another format
/// gives `known_format` false and nothing else; a nethash that is not 64 hex digits is kept as
/// empty (it matches no network, as the forger reads it); an entry the forger would refuse makes
/// the whole record [`ParseError::Malformed`], as it makes the forger refuse to start.
pub fn parse_signing_record(bytes: &[u8]) -> Result<SigningRecordFacts, ParseError> {
    let bad = ParseError::Malformed("signing record");
    let value: Value = serde_json::from_slice(bytes).map_err(|_| bad)?;
    let text = |value: &Value, key: &str| value.get(key).and_then(Value::as_str).map(str::to_owned);
    let id = |kind, text: &str| Some(Id::new(kind, &text.to_ascii_lowercase())?.as_str().into());
    let known_format = text(&value, "format").as_deref() == Some(SIGNING_RECORD_FORMAT);
    let mut facts = SigningRecordFacts {
        known_format,
        nethash: String::new(),
        entries: Vec::new(),
    };
    if !known_format {
        return Ok(facts);
    }
    let nethash = text(&value, "nethash").and_then(|hash| id(IdKind::Hex64, &hash));
    let keys = value.get("keys").and_then(Value::as_object).ok_or(bad)?;
    let entry = |(key, entry): (&String, &Value)| {
        text(entry, "bytes")?;
        Some(SignedEntry {
            public_key: id(IdKind::PublicKey, key)?,
            height: u32::try_from(entry.get("height")?.as_u64()?).ok()?,
            slot: entry.get("slot")?.as_i64()?,
            block_id: id(IdKind::Hex64, &text(entry, "id")?)?,
        })
    };
    let entries: Option<Vec<SignedEntry>> = keys.iter().map(entry).collect();
    facts.entries = entries.ok_or(bad)?;
    facts
        .entries
        .sort_by(|a, b| a.public_key.cmp(&b.public_key));
    facts.nethash = nethash.unwrap_or_default();
    Ok(facts)
}
