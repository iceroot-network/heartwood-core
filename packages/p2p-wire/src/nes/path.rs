//! The path normalisation of the frame parser: `pathname + search` of the WHATWG URL parser for
//! `http://127.0.0.1/` followed by the path.
//!
//! With a special scheme and a fixed host, only these steps of the URL parser apply: trailing C0
//! controls and spaces are removed, and every tab, LF and CR; `/` and `\` end a segment; `?` starts
//! the query and `#` the fragment, which is dropped; `.` and `%2e` segments (any case) are
//! single-dot segments, `..`, `.%2e`, `%2e.` and `%2e%2e` double-dot segments, which remove the
//! previous segment; code points of the path percent-encode set, and every code point above
//! U+007E, are percent-encoded as UTF-8 bytes in uppercase hex, and `%` is left as it is; the query
//! uses the special-query percent-encode set; `search` is empty when the query is empty.

use std::fmt::Write as _;

#[cfg(test)]
mod tests;

/// `pathname + search` of `new URL("http://127.0.0.1/" + raw)`.
pub(crate) fn normalise(raw: &str) -> String {
    let input: String = raw
        .trim_end_matches(|c: char| c <= ' ')
        .chars()
        .filter(|c| !matches!(c, '\t' | '\n' | '\r'))
        .collect();
    let before_fragment = input.split('#').next().unwrap_or_default();
    let (path, query) = before_fragment
        .split_once('?')
        .unwrap_or((before_fragment, ""));

    // A dot segment adds nothing, and a double-dot one removes the previous segment, except that
    // the last segment (the one that `?`, `#` or the end closes) leaves an empty segment instead.
    let mut segments: Vec<&str> = Vec::new();
    let mut parts = path.split(['/', '\\']).peekable();
    while let Some(part) = parts.next() {
        let last = parts.peek().is_none();
        let lower = part.to_ascii_lowercase();
        match lower.as_str() {
            ".." | ".%2e" | "%2e." | "%2e%2e" => {
                segments.pop();
                if last {
                    segments.push("");
                }
            }
            "." | "%2e" if last => segments.push(""),
            "." | "%2e" => {}
            _ => segments.push(part),
        }
    }

    let mut out = String::with_capacity(input.len());
    for segment in segments {
        out.push('/');
        percent_encode(&mut out, segment, false);
    }
    if !query.is_empty() {
        out.push('?');
        percent_encode(&mut out, query, true);
    }
    out
}

/// Append `text` to `out` with the code points of the path percent-encode set (`query` false) or
/// of the special-query percent-encode set (`query` true) written as `%XX` per UTF-8 byte. `#` and,
/// in a path, `?` never reach this function: they end the part they would be in.
fn percent_encode(out: &mut String, text: &str, query: bool) {
    for c in text.chars() {
        let encoded = !(' '..='~').contains(&c)
            || matches!(c, ' ' | '"' | '<' | '>')
            || if query {
                c == '\''
            } else {
                matches!(c, '`' | '{' | '}')
            };
        if encoded {
            for byte in c.encode_utf8(&mut [0; 4]).bytes() {
                let _ = write!(out, "%{byte:02X}");
            }
        } else {
            out.push(c);
        }
    }
}
