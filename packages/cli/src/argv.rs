//! The colon aliases: the reference's commands are `relay:start`, `forger:run`, `network:generate`
//! and so on; Heartwood's command tree is `relay start`, `forger run`, `network generate`. The
//! shim splits a first argument of the form `<group>:<command>` for every command Heartwood builds.
//! It also reads the reference's `--no-daemon` (its argument parser negates any boolean flag that
//! way) as `--daemon=false`, up to a `--` that ends the flags. It leaves every other argument as
//! it is.

use std::ffi::OsString;

/// Every command the shim knows, by group: the colon spelling of each is split in two.
const COMMANDS: [(&str, &str); 4] = [
    ("relay", "run start stop restart status log"),
    ("forger", "run start stop restart status log"),
    ("network", "generate"),
    (
        "devnet",
        "init start stop status logs stop-node start-node restart-node reset wait env",
    ),
];

/// The arguments with a leading `<group>:<command>` split in two and `--no-daemon` spelled
/// `--daemon=false`; the program name, the first element, is kept.
pub fn rewrite(args: Vec<OsString>) -> Vec<OsString> {
    let mut args = args.into_iter();
    let mut out: Vec<OsString> = args.by_ref().take(1).collect();
    if let Some(first) = args.next() {
        match first.to_str().and_then(split) {
            Some((group, command)) => out.extend([group.into(), command.into()]),
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

/// `(group, command)` when `word` is the colon spelling of a known command.
fn split(word: &str) -> Option<(&str, &str)> {
    let (group, command) = word.split_once(':')?;
    COMMANDS
        .iter()
        .any(|(known, commands)| *known == group && commands.split(' ').any(|c| c == command))
        .then_some((group, command))
}
