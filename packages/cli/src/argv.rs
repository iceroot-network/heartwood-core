//! The colon aliases: the reference's commands are `relay:start`, `forger:run`, `network:generate`
//! and so on; Heartwood's command tree is `relay start`, `forger run`, `network generate`. The
//! shim splits a first argument of the form `<group>:<command>` for every command Heartwood builds,
//! and leaves every other argument as it is.

use std::ffi::OsString;

/// The arguments with a leading `<group>:<command>` split in two; the program name, the first
/// element, is kept.
pub fn rewrite(args: Vec<OsString>) -> Vec<OsString> {
    let _ = args;
    todo!("the colon aliases")
}
