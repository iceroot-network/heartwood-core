//! The five levels of differential testing and where each one plugs in.
//!
//! | Level | What it compares | Where it plugs in | Arrives with |
//! |---|---|---|---|
//! | L0 | golden vectors against Heartwood, byte-exact and accept/reject | [`l0`]: records to [`crate::subject`] | `heartwood-crypto` (runs) |
//! | L1 | random and mutated cases, one oracle call each | [`crate::oracle::OracleServer`] and the same subjects, driven by `proptest` | the first oracle operations, then property tests |
//! | L2 | block sequences replayed on both sides, a state digest per block | a replay driver over `heartwood-blockchain` and a TypeScript harness plugin | block processing |
//! | L3 | the same scenario on a reference devnet and a Heartwood devnet | a scenario runner over both devnets | the first Heartwood devnet |
//! | L4 | a mixed network of reference and Heartwood nodes | the scenario runner with mixed nodes | mixed networks |
//!
//! Only L0 runs today, with the subjects of `heartwood-crypto` ([`crate::subjects`]).

pub mod l0;
pub mod l1;
pub mod l2;

use std::fmt;
use std::str::FromStr;

use crate::errors::Error;

/// A level of differential testing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    /// Golden vectors written by the reference.
    L0,
    /// Random and mutated cases, one oracle call each.
    L1,
    /// Block sequences replayed on both sides.
    L2,
    /// The same scenario on a reference devnet and a Heartwood devnet.
    L3,
    /// A mixed network of reference and Heartwood nodes.
    L4,
}

impl Level {
    /// Every level, in order.
    pub const ALL: [Level; 5] = [Level::L0, Level::L1, Level::L2, Level::L3, Level::L4];

    /// The level's title in reports.
    pub fn title(self) -> &'static str {
        match self {
            Level::L0 => "L0 golden vectors",
            Level::L1 => "L1 property tests against the Node oracle",
            Level::L2 => "L2 block-sequence replay and state digests",
            Level::L3 => "L3 live devnet scenarios",
            Level::L4 => "L4 mixed network of reference and Heartwood nodes",
        }
    }

    /// What the level first runs Heartwood code with.
    pub fn arrives_with(self) -> &'static str {
        match self {
            Level::L0 => "heartwood-crypto",
            Level::L1 => "the first oracle operations, then property tests",
            Level::L2 => "block processing",
            Level::L3 => "the first Heartwood devnet",
            Level::L4 => "mixed networks",
        }
    }

    /// Whether this build can run the level.
    pub fn is_available(self) -> bool {
        self == Level::L0
    }
}

impl fmt::Display for Level {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Level::L0 => "l0",
            Level::L1 => "l1",
            Level::L2 => "l2",
            Level::L3 => "l3",
            Level::L4 => "l4",
        };
        f.write_str(name)
    }
}

impl FromStr for Level {
    type Err = Error;

    fn from_str(s: &str) -> Result<Level, Error> {
        Level::ALL
            .into_iter()
            .find(|l| l.to_string().eq_ignore_ascii_case(s))
            .ok_or_else(|| Error::Usage(format!("unknown level {s} (l0 to l4)")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_parse_and_only_l0_runs() {
        assert_eq!("L2".parse::<Level>().unwrap(), Level::L2);
        assert!("l5".parse::<Level>().is_err());
        let available: Vec<Level> = Level::ALL
            .into_iter()
            .filter(|l| l.is_available())
            .collect();
        assert_eq!(available, vec![Level::L0]);
    }
}
