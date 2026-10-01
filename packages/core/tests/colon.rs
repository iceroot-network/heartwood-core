//! The colon aliases against the command tree: every command of the tree that has a parent
//! (`relay start`, `config forger private-key`, `env set`, ...) is also spelled with colons
//! (`relay:start`), which parses to the same command as the spaced form, and no colon spelling
//! that the tree lacks is taken for a command.

use std::ffi::OsString;

use clap::{Command, CommandFactory};
use heartwood_cli::argv::rewrite;
use heartwood_core::commands::{Cli, parse};

fn words(line: &str) -> Vec<OsString> {
    line.split_whitespace().map(OsString::from).collect()
}

/// The paths of the visible commands below `command`, each with at least two words, as the words
/// that name it.
fn paths(command: &Command, above: &[String], into: &mut Vec<Vec<String>>) {
    for sub in command.get_subcommands().filter(|s| !s.is_hide_set()) {
        let mut path = above.to_vec();
        path.push(sub.get_name().to_owned());
        if sub.get_name() == "help" {
            continue;
        }
        if path.len() >= 2 {
            into.push(path.clone());
        }
        paths(sub, &path, into);
    }
}

/// The commands below the root: the visible commands of the tree, and the three that `doctor`
/// reads from its own line (`doctor checks`, `doctor explain` and `doctor mcp`).
fn tree() -> Vec<Vec<String>> {
    let mut found = Vec::new();
    paths(&Cli::command(), &[], &mut found);
    for word in ["checks", "explain", "mcp"] {
        found.push(vec!["doctor".to_owned(), word.to_owned()]);
    }
    found
}

#[test]
fn every_command_of_the_tree_has_its_colon_spelling() {
    let found = tree();
    // Twelve devnet commands exist only when that feature is enabled.
    let expected = if cfg!(feature = "devnet") { 46 } else { 34 };
    assert_eq!(found.len(), expected, "{found:?}");
    for path in found {
        let (spaced, colon) = (path.join(" "), path.join(":"));
        assert_eq!(
            rewrite(words(&format!("heartwood {colon} --token node1"))),
            words(&format!("heartwood {spaced} --token node1")),
            "{colon}"
        );
    }
}

#[test]
fn a_colon_spelling_parses_as_its_spaced_form() {
    for path in tree() {
        let flags = ["--token", "node1", "--network", "devnet"].map(OsString::from);
        let build = |first: Vec<String>| {
            let mut args = vec![OsString::from("heartwood")];
            args.extend(first.into_iter().map(OsString::from));
            args.extend(flags.clone());
            parse(args)
        };
        let (spaced, colon) = (build(path.clone()), build(vec![path.join(":")]));
        // A command that needs a flag of its own refuses both alike.
        assert_eq!(
            spaced.as_ref().ok(),
            colon.as_ref().ok(),
            "{}",
            path.join(":")
        );
        assert_eq!(spaced.is_ok(), colon.is_ok(), "{}", path.join(":"));
    }
}

#[test]
fn a_spelling_that_is_no_command_is_left_alone() {
    for line in [
        "heartwood relay:nothing",
        "heartwood env:show",
        "heartwood top:now",
        "heartwood config:forger:key",
        "heartwood a:b:c",
    ] {
        assert_eq!(rewrite(words(line)), words(line), "{line}");
    }
}
