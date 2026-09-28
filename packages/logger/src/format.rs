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
//!   white, and the message is cyan. Without colour (files), there are no escape codes. The
//!   logger's own level styles add nothing, as in a node process without a terminal;
//! - the year is padded to four characters, a sign included (`00-1`), and a time outside the range
//!   of a JavaScript date writes no line.
//!
//! One difference, on purpose: the reference writes a message's characters as they are, so a
//! message that holds outside text (a peer's or a relay's error payload) can write lines of its
//! own and restyle a terminal. Here the text, the message with its fields, is [`escape`]d first,
//! so one call writes exactly one line; a text without such characters writes the reference's
//! bytes.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fmt::Write as _;
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
/// the level and message coloured when `colour` is set. The message is [`escape`]d, and the line
/// ends with its only newline. `None` when the time is outside the range of a JavaScript date,
/// where the reference writes no line.
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
        // Escaped, the message holds no line break and no escape code, which the reference's
        // colour library would close and reopen the colour around.
        let (open, close) = if colour { CYAN } else { ("", "") };
        let _ = write!(line, " {open}{}{close}", escape(message));
    }
    line.push('\n');
    Some(line.replacen(PLACEHOLDER, &level.name().to_ascii_uppercase(), 1))
}

/// `text` with each character that could end a line or disguise one written as an escape, in the
/// form JSON uses: a line feed, carriage return and tab as `\n`, `\r` and `\t`, and as `\uXXXX`
/// every other control character (C0, DEL and C1), the line and paragraph separators (U+2028,
/// U+2029) and the bidirectional controls (Unicode's `Bidi_Control`: U+061C, U+200E, U+200F,
/// U+202A to U+202E, U+2066 to U+2069). Backslashes are kept, so a text without such a character
/// is returned as it is, and compact JSON inside a text stays JSON of the same value. The escape
/// is not meant to be undone: `\n` in a line is an escaped line feed or a backslash and an `n`
/// that the text held.
pub fn escape(text: &str) -> Cow<'_, str> {
    let escaped = |c: char| {
        c.is_control()
            || matches!(c, '\u{61c}' | '\u{200e}' | '\u{200f}')
            || matches!(c, '\u{2028}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
    };
    if !text.contains(escaped) {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len() + 16);
    for c in text.chars() {
        let _ = match c {
            '\n' | '\r' | '\t' => write!(out, "{}", c.escape_default()),
            c if escaped(c) => write!(out, "\\u{:04x}", u32::from(c)),
            c => write!(out, "{c}"),
        };
    }
    Cow::Owned(out)
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
