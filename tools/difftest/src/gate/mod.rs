//! The baseline record's evidence: the gate record every gate command writes ([`record`]), the
//! cumulative ledger of the long runs ([`ledger`]), and the commands over them: `gate record`
//! (write a record), `gate ledger merge|show|plan` (another machine's records into the ledger, its
//! totals, what the criteria still need), `gate s1` (the baseline record's table from the records, the
//! ledger and the criteria file), `gate coverage` (the coverage rows' records) and `gate register`
//! (every register entry's trigger, test and hits).

mod build;
pub mod coverage;
pub mod criteria;
pub mod flags;
pub mod git;
pub mod ledger;
pub mod make_record;
pub mod names;
pub mod needs;
pub mod output;
pub mod record;
pub mod register;
pub mod s1;
pub mod store;
pub mod time;
pub mod totals;

use crate::errors::Result;

pub use ledger::{LEDGER_FORMAT, LedgerEntry, LedgerKind};
pub use record::{Counts, GateRecord, GateResult, RECORD_FORMAT};

/// A gate command, as `difftest gate ...` parses it: its verb and the rest of its arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateArgs {
    /// `record`, `ledger`, `s1`, `coverage` or `register`.
    pub verb: GateVerb,
    /// The verb's arguments.
    pub args: Vec<String>,
}

/// The gate commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateVerb {
    /// `gate record`: write one gate record.
    Record,
    /// `gate ledger merge|show|plan`.
    Ledger,
    /// `gate s1`: the baseline record's table.
    S1,
    /// `gate coverage`: the coverage rows' records.
    Coverage,
    /// `gate register`: the register's entries against the code and the records.
    Register,
}

impl GateVerb {
    /// The verb's name.
    pub const fn name(self) -> &'static str {
        match self {
            GateVerb::Record => "record",
            GateVerb::Ledger => "ledger",
            GateVerb::S1 => "s1",
            GateVerb::Coverage => "coverage",
            GateVerb::Register => "register",
        }
    }
}

/// Run a gate command; whether its checks passed. `strict` is the harness's `--strict`, which the
/// command line takes out of the arguments before they get here: it makes `s1`, `coverage`,
/// `register` and `ledger show|plan` demand everything, not only that nothing failed.
pub fn run(args: &GateArgs, strict: bool) -> Result<bool> {
    let mut rest = args.args.clone();
    let takes_strict = match args.verb {
        GateVerb::S1 | GateVerb::Coverage | GateVerb::Register => true,
        GateVerb::Ledger => rest.first().is_some_and(|v| v == "show" || v == "plan"),
        GateVerb::Record => false,
    };
    if strict && takes_strict {
        rest.push("--strict".to_owned());
    }
    match args.verb {
        GateVerb::Record => make_record::run(&rest),
        GateVerb::Ledger => totals::run(&rest),
        GateVerb::S1 => s1::run(&rest),
        GateVerb::Coverage => coverage::run(&rest),
        GateVerb::Register => register::run(&rest),
    }
}
