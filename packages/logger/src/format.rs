//! The line format of the reference's pretty printer.
//!
//! A line is the event's UTC time as `[YYYY-MM-DD HH:MM:ss.lll]`, the level in capitals, a colon,
//! and the message, then a newline; an empty message leaves `LEVEL:` at the end. This is what the
//! reference writes, quirks included:
//!
//! - its printer knows only `INFO`, `ERROR` and `DEBUG` among the eight levels, and writes the
//!   others as a placeholder that the logger then replaces, first occurrence in the line, with the
//!   level's name. A message that contains the placeholder, `USERLVL`, at `info`, `error` or
//!   `debug` has it replaced instead;
//! - with colour (the console), `info` is green, `error` red, `debug` blue and every other level
//!   white, and the message is cyan, closed and reopened around each line break and after each
//!   colour reset inside it. Without colour (files), there are no escape codes. The logger's own
//!   level styles add nothing, as in a node process without a terminal;
//! - the year is padded to four characters, a sign included (`00-1`), and a time outside the range
//!   of a JavaScript date writes no line.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use heartwood_kernel::log::Level;
use heartwood_kernel::timestamp::UtcDateTime;

/// The level placeholder of the reference's printer.
const PLACEHOLDER: &str = "USERLVL";
/// The message colour's opening and closing codes.
const CYAN: (&str, &str) = ("\u{1b}[36m", "\u{1b}[39m");

/// The emoji of each code, from the reference's emoji table.
static EMOJI: LazyLock<BTreeMap<&'static str, &'static str>> = LazyLock::new(|| {
    include_str!("emoji.tsv")
        .lines()
        .filter(|line| !line.starts_with('#'))
        .filter_map(|line| line.split_once('\t'))
        .collect()
});

/// The line for `message` at `level`, logged at `unix_ms` milliseconds since the Unix epoch, with
/// the level and message coloured when `colour` is set. The line ends with a newline. `None` when
/// the time is outside the range of a JavaScript date, where the reference writes no line.
pub fn line(level: Level, unix_ms: i64, message: &str, colour: bool) -> Option<String> {
    let time = UtcDateTime::from_unix_ms(unix_ms)?;
    let (label, code) = match level {
        Level::Info => ("INFO", 32),
        Level::Error => ("ERROR", 31),
        Level::Debug => ("DEBUG", 34),
        _ => (PLACEHOLDER, 37),
    };
    let mut line = format!(
        "[{:0>4}-{:02}-{:02} {:02}:{:02}:{:02}.{:03}] ",
        time.year, time.month, time.day, time.hour, time.minute, time.second, time.millisecond
    );
    if colour {
        line.push_str(&format!("\u{1b}[{code}m{label}\u{1b}[39m:"));
    } else {
        line.push_str(label);
        line.push(':');
    }
    if !message.is_empty() {
        line.push(' ');
        if colour {
            line.push_str(&coloured(message, CYAN));
        } else {
            line.push_str(message);
        }
    }
    line.push('\n');
    Some(line.replacen(PLACEHOLDER, &level.name().to_ascii_uppercase(), 1))
}

/// `text` in a colour, as the reference's colour library styles it: a reset of the colour inside
/// the text reopens it, and the colour is closed before and reopened after each line break.
fn coloured(text: &str, (open, close): (&str, &str)) -> String {
    let reopened = if text.contains('\u{1b}') {
        text.replace(close, &format!("{close}{open}"))
    } else {
        text.to_owned()
    };
    let mut out = String::from(open);
    let mut lines = reopened.split('\n').peekable();
    while let Some(part) = lines.next() {
        if lines.peek().is_none() {
            out.push_str(part);
            break;
        }
        match part.strip_suffix('\r') {
            Some(before) => out.push_str(&format!("{before}{close}\r\n{open}")),
            None => out.push_str(&format!("{part}{close}\n{open}")),
        }
    }
    out.push_str(close);
    out
}

/// `text` with every emoji code the reference knows (`:dove_of_peace:`) replaced by its emoji
/// when `keep` is set, or removed when it is not; unknown codes stay as they are. A code is a run
/// of letters, digits, `_`, `-` and `+` between two colons, read from the left.
pub fn emojify(text: &str, keep: bool) -> String {
    let is_code_char = |c: char| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '+');
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find(':') {
        let (before, from_colon) = rest.split_at(start);
        out.push_str(before);
        let after = from_colon.get(1..).unwrap_or_default();
        let name_len = after
            .find(|c: char| !is_code_char(c))
            .unwrap_or(after.len());
        let name = after.get(..name_len).unwrap_or_default();
        match after
            .get(name_len..)
            .and_then(|tail| tail.strip_prefix(':'))
        {
            Some(tail) if !name.is_empty() => {
                match EMOJI.get(name) {
                    Some(emoji) if keep => out.push_str(emoji),
                    Some(_) => {}
                    None => {
                        out.push(':');
                        out.push_str(name);
                        out.push(':');
                    }
                }
                rest = tail;
            }
            _ => {
                out.push(':');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}
