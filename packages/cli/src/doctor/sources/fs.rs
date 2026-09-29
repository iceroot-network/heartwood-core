//! Files: [`Readable`], the only path [`DoctorFs::read`] and [`DoctorFs::tail`] accept;
//! [`KeyFile`], which yields metadata and nothing else; [`HostFs`], the real [`DoctorFs`]; and the
//! signing record's parser.
//!
//! [`Readable::new`] refuses a path that is not absolute, that has a `.` or `..` component, whose
//! file name is on [`KEY_FILE_NAMES`] in any letter case, or that is outside [`ReadableRoots`]:
//! the instance's five directories, `/proc`, `/sys`, and exactly two files the firewall and clock
//! checks read. A name check alone can be beaten by a link with an innocent name, so [`HostFs`]
//! opens every file with `O_NOFOLLOW`, takes the open descriptor's device and inode, and closes it
//! unread when they are a key file's ([`FsError::KeyFile`]).

use std::ffi::OsString;
use std::io;
use std::path::{Component, Path, PathBuf};

use heartwood_kernel::paths::Paths;

use crate::doctor::seams::{DoctorFs, FileFacts, FsError, FsStats, Tail};
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
    /// The roots of an instance with the directories `paths`: its five directories, `/proc`,
    /// `/sys` and [`READABLE_FILES`].
    pub fn for_paths(paths: &Paths) -> ReadableRoots {
        let own = [
            &paths.config,
            &paths.data,
            &paths.cache,
            &paths.log,
            &paths.temp,
        ];
        let mut dirs: Vec<PathBuf> = own.into_iter().cloned().collect();
        dirs.extend(["/proc", "/sys"].map(PathBuf::from));
        ReadableRoots {
            dirs,
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
}

impl DoctorFs for HostFs {
    fn facts(&self, path: &Path) -> io::Result<FileFacts> {
        let _ = path;
        Err(io::Error::other("not built"))
    }

    fn read(&self, file: &Readable, max: u64) -> Result<Vec<u8>, FsError> {
        let _ = (file, max);
        Err(FsError::NotBuilt)
    }

    fn tail(&self, file: &Readable, max: u64) -> Result<Tail, FsError> {
        let _ = (file, max);
        Err(FsError::NotBuilt)
    }

    fn list(&self, dir: &Readable) -> Result<Vec<OsString>, FsError> {
        let _ = dir;
        Err(FsError::NotBuilt)
    }

    fn stats(&self, path: &Path) -> Result<FsStats, FsError> {
        let _ = path;
        Err(FsError::NotBuilt)
    }
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

/// The signing record in `bytes`; the blocks' bytes are not kept.
pub fn parse_signing_record(bytes: &[u8]) -> Result<SigningRecordFacts, ParseError> {
    let _ = bytes;
    Err(ParseError::NotBuilt)
}
