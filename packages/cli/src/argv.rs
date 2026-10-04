//! The colon aliases: the reference's commands are `relay:start`, `forger:run`, `network:generate`
//! and so on; Heartwood's command tree is `relay start`, `forger run`, `network generate`. The
//! shim splits a first argument of the form `<group>:<command>` for every command Heartwood builds.
//! It also reads the reference's `--no-daemon` (its argument parser negates any boolean flag that
//! way) as `--daemon=false`, up to a `--` that ends the flags. It leaves every other argument as
//! it is.

use std::ffi::OsString;

/// Every command of the tree that has a colon spelling, spaced: the colon spelling of each is
/// the same words joined with `:` (`relay:start`, `config:forger:private-key`). A command of one
/// word (`top`, `version`) has none, and nor has a group that is no command (`relay`).
const COMMANDS: [&str; 46] = [
    "relay run",
    "relay start",
    "relay stop",
    "relay restart",
    "relay status",
    "relay log",
    "forger run",
    "forger start",
    "forger stop",
    "forger restart",
    "forger status",
    "forger log",
    "core run",
    "core start",
    "core stop",
    "core restart",
    "core status",
    "core log",
    "network generate",
    "snapshot rollback",
    "pool clear",
    "state clear",
    "doctor checks",
    "doctor explain",
    "doctor mcp",
    "env get",
    "env list",
    "env set",
    "env paths",
    "config publish",
    "config forger",
    "config forger private-key",
    "service install",
    "service uninstall",
    "devnet init",
    "devnet start",
    "devnet stop",
    "devnet status",
    "devnet logs",
    "devnet stop-node",
    "devnet start-node",
    "devnet restart-node",
    "devnet reset",
    "devnet wait",
    "devnet env",
    "devnet run",
];

/// The arguments with a leading `<group>:<command>` split into its words and `--no-daemon`
/// spelled `--daemon=false`; the program name, the first element, is kept.
pub fn rewrite(args: Vec<OsString>) -> Vec<OsString> {
    let mut args = args.into_iter();
    let mut out: Vec<OsString> = args.by_ref().take(1).collect();
    if let Some(first) = args.next() {
        match first.to_str().and_then(split) {
            Some(words) => out.extend(words.into_iter().map(OsString::from)),
            None => out.push(first),
        }
    }
    let mut flags = true;
    for arg in args {
        flags &= arg != "--";
        out.push(match arg.to_str() {
            Some("--no-daemon") if flags => "--daemon=false".into(),
            _ => arg,
        });
    }
    out
}

/// The words of `word` when it is the colon spelling of a command of the tree.
fn split(word: &str) -> Option<Vec<String>> {
    let spaced = word.contains(':').then(|| word.replace(':', " "))?;
    let words = spaced.split(' ').map(str::to_owned).collect();
    COMMANDS.contains(&spaced.as_str()).then_some(words)
}
