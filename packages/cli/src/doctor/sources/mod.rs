//! The sources: the real implementations of the host's seams, and the parsers that turn what a
//! source gives into the typed facts of [`crate::doctor::view::View`]. Every parser takes bytes
//! or text from outside the doctor and must never panic on it: it gives a [`ParseError`] instead.
//!
//! - [`fs`]: the real [`crate::doctor::seams::DoctorFs`], [`fs::Readable`] with the key-file
//!   names and the readable roots, [`fs::KeyFile`], and the signing record's parser.
//! - [`procfs`]: the real [`crate::doctor::seams::ProcView`] and the parsers of `/proc` and
//!   `/sys`.
//! - [`probes`]: the real [`crate::doctor::seams::Probes`] and the probes' parsers.
//! - [`logs`]: the log window of a role, its line kinds, line integrity and the error capture.
//! - [`patterns`]: each line kind's template, as the ordered literal pieces of its log call.
//! - [`api`]: the API's address rule, the request budget and the answers' parsers.

pub mod api;
pub mod fs;
pub mod logs;
pub mod patterns;
pub mod probes;
pub mod procfs;

/// Why a parser refused its input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ParseError {
    /// The parser is not built yet.
    #[error("not built")]
    NotBuilt,
    /// The input is not what the source writes; the word says which part.
    #[error("malformed: {0}")]
    Malformed(&'static str),
}
