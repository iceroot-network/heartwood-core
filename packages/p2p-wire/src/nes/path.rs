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
