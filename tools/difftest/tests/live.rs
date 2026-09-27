//! `difftest live check` on recorded fixtures: a run directory with digest files (a torn last line
//! among them), store anchors, step reports, API answers and relay logs for the ban scan with its
//! fault windows. The fixtures are small recordings of dry runs; until they are recorded the test
//! checks only that the command parses.

use heartwood_difftest::cli::{self, Command};

#[test]
fn a_live_check_parses() {
    let args: Vec<String> = ["live", "check", "fixtures/run"]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
    assert_eq!(
        cli::parse(&args).unwrap(),
        Command::LiveCheck {
            dir: "fixtures/run".into()
        }
    );
}
