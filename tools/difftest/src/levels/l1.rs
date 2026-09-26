//! L1: random and mutated cases, one oracle call each, compared with the same subjects as L0.
//!
//! Each property draws its inputs from [`crate::strategies`], sends each case to
//! `oracle-server.js`, runs the case's subject, and compares the two results; a failing case is
//! shrunk to a minimal one.

use std::fmt;
use std::str::FromStr;

use crate::errors::{Error, Result};
use crate::oracle::OracleServer;
use crate::report::Report;
use crate::subject::Registry;

/// A property of L1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Property {
    /// The dynamic fee floor (`fee.minimum`).
    FeeFloor,
    /// Rounds (`round.calculate`, `round.isNew`).
    Rounds,
    /// Forging slots and chaining (`forging.info`, `block.isChained`).
    Slots,
    /// The forging shuffle (`state.shuffle`).
    Shuffle,
    /// The delegate ranking (`state.rank`).
    Ranking,
    /// A voter's shares (`wallet.voteAmounts`).
    VoteAmounts,
    /// One transaction applied to a fixture and reverted (`state.applyTransaction`).
    Handlers,
    /// The kernel's text formats (`kernel.stringify`, `kernel.dotenv`, `kernel.ip`).
    KernelText,
    /// The state digest of a fixture (`state.digest`).
    Digest,
}

impl Property {
    /// Every property.
    pub const ALL: [Property; 9] = [
        Property::FeeFloor,
        Property::Rounds,
        Property::Slots,
        Property::Shuffle,
        Property::Ranking,
        Property::VoteAmounts,
        Property::Handlers,
        Property::KernelText,
        Property::Digest,
    ];

    /// The property's name on the command line.
    pub const fn name(self) -> &'static str {
        match self {
            Property::FeeFloor => "fee-floor",
            Property::Rounds => "rounds",
            Property::Slots => "slots",
            Property::Shuffle => "shuffle",
            Property::Ranking => "ranking",
            Property::VoteAmounts => "vote-amounts",
            Property::Handlers => "handlers",
            Property::KernelText => "kernel-text",
            Property::Digest => "digest",
        }
    }
}

impl fmt::Display for Property {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl FromStr for Property {
    type Err = Error;

    fn from_str(s: &str) -> Result<Property> {
        Property::ALL
            .into_iter()
            .find(|property| property.name() == s)
            .ok_or_else(|| Error::Usage(format!("unknown L1 property {s}")))
    }
}

/// What an L1 run covers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct L1Options {
    /// The properties to run.
    pub properties: Vec<Property>,
    /// Cases per property.
    pub cases: u32,
    /// The seed of the case generator.
    pub seed: u64,
}

/// Run the properties of `options` against the oracle `server` and the subjects of `registry`.
pub fn run(server: &mut OracleServer, registry: &Registry, options: &L1Options) -> Result<Report> {
    let _ = (server, registry, options);
    todo!()
}
