//! The line format of the reference's pretty printer.
//!
//! A line is the event's time as `[YYYY-MM-DD HH:MM:ss.lll]`, the level in capitals, and the
//! message, as the reference's printer writes them. On the console the level is coloured
//! (`emergency` on a red background; `alert`, `critical` and `error` red; `warning` yellow;
//! `notice` green; `info` blue; `debug` magenta); in files it is not.

use heartwood_kernel::log::Level;

/// The line for `message` at `level`, logged at `unix_ms` milliseconds since the Unix epoch, with
/// the level coloured when `colour` is set. The line ends with a newline.
pub fn line(level: Level, unix_ms: i64, message: &str, colour: bool) -> String {
    let _ = (level, unix_ms, message, colour);
    todo!()
}

/// `text` with every emoji code the reference knows (`:dove_of_peace:`) replaced by its emoji
/// when `keep` is set, or removed when it is not; unknown codes stay as they are.
pub fn emojify(text: &str, keep: bool) -> String {
    let _ = (text, keep);
    todo!()
}
