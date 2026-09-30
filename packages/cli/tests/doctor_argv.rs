//! The doctor's colon forms: `doctor:checks`, `doctor:explain` and `doctor:mcp` are the same
//! commands as `doctor checks`, `doctor explain` and `doctor mcp`.

use std::ffi::OsString;

use heartwood_cli::argv::rewrite;

fn words(text: &str) -> Vec<OsString> {
    text.split_whitespace().map(OsString::from).collect()
}

#[test]
fn the_doctors_colon_forms_split_in_two() {
    for command in ["checks", "explain", "mcp"] {
        let colon = words(&format!("heartwood doctor:{command} --network testnet"));
        let spaced = words(&format!("heartwood doctor {command} --network testnet"));
        assert_eq!(rewrite(colon), spaced, "{command}");
    }
}

#[test]
fn an_unknown_doctor_colon_form_is_left_as_it_is() {
    let line = words("heartwood doctor:repair --network testnet");
    assert_eq!(rewrite(line.clone()), line);
}
