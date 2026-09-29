//! `heartwood pool clear` and `heartwood state clear`: the reference's two markers, written into the
//! relay's temp directory, where the relay reads them at its next start (`clear-pool.lock` clears
//! the pool's storage, `clear-saved-states.lock` restores the state by a replay).
//!
//! Each command asks the reference's question on a terminal, or goes on with `--force`; without
//! either it refuses with the reference's text. It resolves the relay's instance as the relay does
//! (the five `CORE_PATH_*` included), so the marker lands where the relay reads it; a missing temp
//! directory is created with mode 0700 (the relay does not create it); and it writes the empty
//! marker without following a symbolic link. A marker that cannot be written is an error, exit
//! status 1, where the reference writes nothing and exits 0.
//!
//! One trust rule serves the commands and the relay's start (core's `markers`): a path is trusted
//! when it is of its kind (a file for the marker, a directory for its directory; a symbolic link is
//! neither), belongs to the user who owns the relay's `<data>/run` directory, and is not writable by
//! every user. The group's write bit is allowed on the file, and on the directory only when the
//! group is the user's own (named as the user, with no other member), as a umask of 002 gives it: a
//! member of a shared group could rename or link a file of the user into a directory the group may
//! write. The marker must have no other hard link. The relay ignores a marker that fails the rule,
//! with a warning; the commands refuse a temp directory that fails it. [`judge`] is the rule itself,
//! over metadata the caller read, so the tests can give it any owner and mode.

use std::fs::{self, DirBuilder, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use nix::fcntl::OFlag;
use thiserror::Error;

use crate::errors::InstanceError;
use crate::instance::{InstanceFlags, resolve};
use crate::seams::{Instance, Role};

/// A marker of the reference, which the relay acts on at its next start.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Marker {
    /// `clear-pool.lock`: the pool's storage is cleared.
    ClearPool,
    /// `clear-saved-states.lock`: the state is restored by a replay from the genesis block.
    ClearSavedStates,
}

impl Marker {
    /// The marker's file name in the temp directory, the reference's.
    pub fn file_name(self) -> &'static str {
        match self {
            Marker::ClearPool => "clear-pool.lock",
            Marker::ClearSavedStates => "clear-saved-states.lock",
        }
    }

    /// The question the command asks before it writes the marker, the reference's word for word.
    #[rustfmt::skip]
    pub fn question(self) -> &'static str {
        match self {
            Marker::ClearPool => "Clear all unconfirmed transactions from your pool on next startup?",
            Marker::ClearSavedStates => "Clear all saved states so a fresh state will be generated on startup?",
        }
    }
}

/// Asks the user a yes-or-no question.
pub trait Confirm {
    /// The answer to `question`; `None` when there is no answer to read (no terminal).
    fn confirm(&mut self, question: &str) -> Option<bool>;
}

/// What kind of file a path is, its symbolic link not followed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    /// A regular file.
    File,
    /// A directory.
    Directory,
    /// Anything else, a symbolic link included.
    Other,
}

/// What the trust rule reads of a path: its own metadata, a symbolic link not followed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Meta {
    /// The kind of file.
    pub kind: Kind,
    /// The owner's user id.
    pub uid: u32,
    /// The permission bits.
    pub mode: u32,
    /// The group id.
    pub gid: u32,
    /// Whether the group is the owner's own ([`own_group`]).
    pub own_group: bool,
    /// The number of hard links.
    pub links: u64,
}

impl Meta {
    /// The metadata of `path` (`symlink_metadata`).
    pub fn of(path: &Path) -> io::Result<Meta> {
        let meta = fs::symlink_metadata(path)?;
        let kind = match (meta.is_file(), meta.is_dir()) {
            (true, _) => Kind::File,
            (_, true) => Kind::Directory,
            _ => Kind::Other,
        };
        let (uid, mode, gid) = (meta.uid(), meta.mode() & 0o7777, meta.gid());
        let (own_group, links) = (own_group(uid, gid), meta.nlink());
        Ok(Meta {
            kind,
            uid,
            mode,
            gid,
            own_group,
            links,
        })
    }
}

/// Whether the group `gid` is the user `uid`'s own: named as the user, with no other member.
pub fn own_group(uid: u32, gid: u32) -> bool {
    use nix::unistd::{Gid, Group, Uid, User};
    let user = User::from_uid(Uid::from_raw(uid)).ok().flatten();
    let group = Group::from_gid(Gid::from_raw(gid)).ok().flatten();
    user.zip(group).is_some_and(|(user, group)| {
        group.name == user.name && group.mem.iter().all(|member| *member == user.name)
    })
}

/// Why a path is not trusted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum Untrusted {
    /// It is not of its kind: a marker that is no regular file, or a directory that is none.
    #[error("it is not a {}", if *.expected == Kind::File { "file" } else { "directory" })]
    WrongKind {
        /// The kind it must be.
        expected: Kind,
    },
    /// Another user owns it.
    #[error("it belongs to user {uid}, not to user {owner}")]
    OtherOwner {
        /// Its owner.
        uid: u32,
        /// The user it must belong to.
        owner: u32,
    },
    /// Every user may write it.
    #[error("any user may write it (mode {mode:o})")]
    WorldWritable {
        /// Its permission bits.
        mode: u32,
    },
    /// A directory the members of a group other than its owner's own may write.
    #[error("the members of group {gid} may write it (mode {mode:o})")]
    GroupWritable {
        /// Its permission bits.
        mode: u32,
        /// Its group.
        gid: u32,
    },
    /// A marker with another hard link, which a user who may read and write it can make.
    #[error("it has {links} hard links")]
    Linked {
        /// Its hard links.
        links: u64,
    },
}

/// The trust rule's verdict on a path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Trust {
    /// The path may be relied on.
    Trusted,
    /// The path fails the rule.
    Untrusted {
        /// The path.
        path: PathBuf,
        /// Why.
        why: Untrusted,
    },
}

/// An error of the recovery commands.
#[derive(Debug, Error)]
pub enum RecoveryError {
    /// Neither `--force` nor a yes on the terminal: the reference's refusal.
    #[error("You'll need to confirm the input to continue")]
    NotConfirmed,
    /// The instance could not be resolved.
    #[error(transparent)]
    Instance(#[from] InstanceError),
    /// The temp directory, or a marker already in it, fails the trust rule.
    #[error("{}: {why}", path.display())]
    Untrusted {
        /// The directory.
        path: PathBuf,
        /// Why.
        why: Untrusted,
    },
    /// A file or directory could not be read, made or written.
    #[error("{}: {source}", path.display())]
    Io {
        /// The path.
        path: PathBuf,
        /// The error.
        source: io::Error,
    },
}

/// The trust rule on `path`, whose metadata is `meta`: it must be of `kind`, belong to the user
/// `owner` and not be writable by every user; a directory not by a group other than the user's
/// own either; a file must have no other hard link.
pub fn judge(path: &Path, meta: &Meta, kind: Kind, owner: u32) -> Trust {
    let Meta {
        uid,
        mode,
        gid,
        links,
        ..
    } = *meta;
    let why = if meta.kind != kind {
        Untrusted::WrongKind { expected: kind }
    } else if uid != owner {
        Untrusted::OtherOwner { uid, owner }
    } else if mode & 0o002 != 0 {
        Untrusted::WorldWritable { mode }
    } else if kind == Kind::Directory && mode & 0o020 != 0 && !meta.own_group {
        Untrusted::GroupWritable { mode, gid }
    } else if kind == Kind::File && links > 1 {
        Untrusted::Linked { links }
    } else {
        return Trust::Trusted;
    };
    let path = path.to_owned();
    Trust::Untrusted { path, why }
}

/// The relay's check of `marker` at its start, for the relay whose data directory is `data`: the
/// rule on the marker (a file) and on its directory, with the owner of `<data>/run` as the user.
/// `None` when the marker does not exist, or `<data>/run` does not (no user to compare with): then
/// nothing is honoured, and nothing is said.
pub fn marker_trust(marker: &Path, data: &Path) -> Option<Trust> {
    let owner = relay_user(data)?;
    let dir = marker.parent().unwrap_or(Path::new("."));
    let file = judge(marker, &Meta::of(marker).ok()?, Kind::File, owner);
    let dir = judge(dir, &Meta::of(dir).ok()?, Kind::Directory, owner);
    Some(if file == Trust::Trusted { dir } else { file })
}

/// The error of a failed file operation on `path`.
fn io_at(path: &Path) -> impl FnOnce(io::Error) -> RecoveryError {
    let path = path.to_owned();
    |source| RecoveryError::Io { path, source }
}

/// The commands' check of the temp directory `temp` before a marker is written into it: the rule
/// on a directory, with the owner of `<data>/run` as the user, or the process's own user when the
/// relay has never run.
pub fn temp_trust(temp: &Path, data: &Path) -> Result<Trust, RecoveryError> {
    let owner = relay_user(data).unwrap_or_else(|| nix::unistd::Uid::current().as_raw());
    let meta = Meta::of(temp).map_err(io_at(temp))?;
    Ok(judge(temp, &meta, Kind::Directory, owner))
}

/// The relay's user: the owner of `<data>/run`, if the relay has run.
fn relay_user(data: &Path) -> Option<u32> {
    Some(fs::metadata(data.join("run")).ok()?.uid())
}

/// Write `marker` for `instance` (a relay), asking `confirm` unless `force`: the temp directory
/// created with mode 0700 when it is missing, refused when it fails the trust rule, and the empty
/// marker created without following a symbolic link (one that exists is kept, and refused when the
/// relay would ignore it). The marker's path.
pub fn request(
    instance: &Instance,
    marker: Marker,
    force: bool,
    confirm: &mut dyn Confirm,
) -> Result<PathBuf, RecoveryError> {
    if !force && confirm.confirm(marker.question()) != Some(true) {
        return Err(RecoveryError::NotConfirmed);
    }
    let temp = &instance.paths.temp;
    let made = DirBuilder::new().recursive(true).mode(0o700).create(temp);
    made.map_err(io_at(temp))?;
    if let Trust::Untrusted { path, why } = temp_trust(temp, &instance.paths.data)? {
        return Err(RecoveryError::Untrusted { path, why });
    }
    let path = temp.join(marker.file_name());
    let mut file = OpenOptions::new();
    file.write(true).create(true).truncate(false).mode(0o600);
    file.custom_flags(OFlag::O_NOFOLLOW.bits());
    file.open(&path).map_err(io_at(&path))?;
    match marker_trust(&path, &instance.paths.data) {
        Some(Trust::Untrusted { path, why }) => Err(RecoveryError::Untrusted { path, why }),
        _ => Ok(path),
    }
}

/// The confirmation on standard input: the question and `(y/N)` on standard output, then a line
/// read from standard input (the reference's prompt reads a pipe as it reads a terminal). As the
/// reference's prompt, the first `y` or `n` typed decides, in either case, other characters are
/// ignored, and the end of the line before either is a no; no answer when nothing can be read.
impl Confirm for io::Stdin {
    fn confirm(&mut self, question: &str) -> Option<bool> {
        print!("{question} (y/N) ");
        io::stdout().flush().ok()?;
        let mut line = String::new();
        let read = self.read_line(&mut line).ok()?;
        let key = line.chars().find(|c| "yYnN\n\r".contains(*c));
        (read > 0).then_some(matches!(key, Some('y' | 'Y')))
    }
}

/// `pool clear` (`marker` [`Marker::ClearPool`]) or `state clear` ([`Marker::ClearSavedStates`]):
/// the relay's instance of `flags`, resolved with the process's environment, home and working
/// directory; the question on a terminal unless `force`; [`request`]; silence on success, the
/// error's message and exit status 1 otherwise.
pub fn clear(flags: &InstanceFlags, marker: Marker, force: bool) -> ExitCode {
    let env = |name: &str| std::env::var(name).ok();
    let home = std::env::home_dir().unwrap_or_default();
    let cwd = std::env::current_dir().unwrap_or_default();
    let done = resolve(Role::Relay, flags, &env, &home, &cwd)
        .map_err(RecoveryError::from)
        .and_then(|instance| request(&instance, marker, force, &mut io::stdin()))
        .inspect_err(|error| eprintln!("{error}"));
    ExitCode::from(u8::from(done.is_err()))
}
