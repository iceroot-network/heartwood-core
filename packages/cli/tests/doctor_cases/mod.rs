//! The checks' case tables, one file per area. A case runs one check over a [`FakeView`] with
//! options and names the findings it must give: subject, status and advice key. Every finding the
//! check gives must have a status its registry entry declares.

#![allow(dead_code, reason = "each test file uses part of the cases")]

use heartwood_cli::doctor::DoctorOptions;
use heartwood_cli::doctor::report::Status;
use heartwood_cli::doctor::seams::Subject;

use crate::doctor_support::FakeView;

pub mod clock;
pub mod config;
pub mod disk;
pub mod forger;
pub mod host;
pub mod net;
pub mod onboarding;
pub mod peers;
pub mod process;
pub mod quorum;
pub mod slots;
pub mod sync;
pub mod version;

/// One finding a case requires.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Expect {
    /// What it is about.
    pub subject: Subject,
    /// Its status.
    pub status: Status,
    /// Its advice key.
    pub advice: &'static str,
}

/// One case of one check.
pub struct Case {
    /// The case's name.
    pub name: &'static str,
    /// The check's id.
    pub check: &'static str,
    /// The view it runs over.
    pub view: FakeView,
    /// The run's options.
    pub options: DoctorOptions,
    /// The findings it must give (it may give more).
    pub expect: Vec<Expect>,
}

/// Every case of every area.
pub fn all() -> Vec<Case> {
    [
        process::cases(),
        peers::cases(),
        sync::cases(),
        slots::cases(),
        forger::cases(),
        quorum::cases(),
        net::cases(),
        clock::cases(),
        disk::cases(),
        config::cases(),
        version::cases(),
        host::cases(),
        onboarding::cases(),
    ]
    .into_iter()
    .flatten()
    .collect()
}
