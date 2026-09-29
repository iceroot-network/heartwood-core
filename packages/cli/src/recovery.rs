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
//! every user. The group's write bit is allowed, as a umask of 002 gives it to the user's own group.
//! The relay ignores a marker that fails the rule, with a warning; the commands refuse a temp
//! directory that fails it. [`judge`] is the rule itself, over metadata the caller read, so the tests
//! can give it any owner and mode.

use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use thiserror::Error;

use crate::errors::InstanceError;
use crate::instance::InstanceFlags;
use crate::seams::Instance;

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
        todo!("the reference's file names")
    }

    /// The question the command asks before it writes the marker, the reference's word for word.
    pub fn question(self) -> &'static str {
        todo!("the reference's prompts")
    }
}

/// Asks the user a yes-or-no question.
pub trait Confirm {
    /// The answer to `question`; `None` when there is no terminal to ask on.
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
}

impl Meta {
    /// The metadata of `path` (`symlink_metadata`).
    pub fn of(path: &Path) -> io::Result<Meta> {
        let _ = path;
        todo!("the path's own metadata")
    }
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
    /// The temp directory fails the trust rule.
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
/// `owner` and not be writable by every user.
pub fn judge(path: &Path, meta: &Meta, kind: Kind, owner: u32) -> Trust {
    let _ = (path, meta, kind, owner);
    todo!("the trust rule")
}

/// The relay's check of `marker` at its start, for the relay whose data directory is `data`: the
/// rule on the marker (a file) and on its directory, with the owner of `<data>/run` as the user.
/// `None` when the marker does not exist, or `<data>/run` does not (no user to compare with): then
/// nothing is honoured, and nothing is said.
pub fn marker_trust(marker: &Path, data: &Path) -> Option<Trust> {
    let _ = (marker, data);
    todo!("the relay's side of the rule")
}

/// The commands' check of the temp directory `temp` before a marker is written into it: the rule
/// on a directory, with the owner of `<data>/run` as the user, or the process's own user when the
/// relay has never run.
pub fn temp_trust(temp: &Path, data: &Path) -> Result<Trust, RecoveryError> {
    let _ = (temp, data);
    todo!("the commands' side of the rule")
}

/// Write `marker` for `instance` (a relay), asking `confirm` unless `force`: the temp directory
/// created with mode 0700 when it is missing, refused when it fails the trust rule, and the empty
/// marker created without following a symbolic link. The marker's path.
pub fn request(
    instance: &Instance,
    marker: Marker,
    force: bool,
    confirm: &mut dyn Confirm,
) -> Result<PathBuf, RecoveryError> {
    let _ = (instance, marker, force, confirm);
    todo!("the marker's request")
}

/// `pool clear` (`marker` [`Marker::ClearPool`]) or `state clear` ([`Marker::ClearSavedStates`]):
/// the relay's instance of `flags`, resolved with the process's environment, home and working
/// directory; the question on a terminal unless `force`; [`request`]; silence on success, the
/// error's message and exit status 1 otherwise.
pub fn clear(flags: &InstanceFlags, marker: Marker, force: bool) -> ExitCode {
    let _ = (flags, marker, force);
    todo!("the command")
}
