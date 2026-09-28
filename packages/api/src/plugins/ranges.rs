//! Byte ranges of an answer, as hapi serves them: a `GET`'s 200 answer with a body carries
//! `accept-ranges: bytes`; a `Range` of one byte range, or of ranges that merge into one, cuts
//! the answer to it (206 with `content-range`); several ranges that do not merge are ignored; a
//! `Range` that does not parse, or names a range that starts past its end, is refused (416 with
//! `content-range: bytes */<length>`). The ranges are read and merged as hapi's range parser reads
//! and merges them, its merge of a range that ends before the one it follows included. A suffix
//! longer than the body is the whole body, where hapi declares more bytes than it sends.

use axum::http::HeaderValue;
use axum::http::header;
use heartwood_crypto::utils::js;

use super::Headers;
use crate::errors::ApiError;

/// The status, the body and the headers of a `GET`'s 200 answer `body` (not empty) for the
/// request's `Range`.
pub(super) fn serve(range: Option<&str>, body: Vec<u8>) -> (u16, Vec<u8>, Headers) {
    let length = body.len() as u64;
    let accept = (header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    let Some(range) = range else {
        return (200, body, vec![accept]);
    };
    let text = |text: String| HeaderValue::try_from(text).unwrap_or(HeaderValue::from_static(""));
    let ranges = read(range, length);
    match ranges.as_deref() {
        None => {
            let error = ApiError::new(416, "Requested Range Not Satisfiable");
            let refused = js::stringify(&error.body(), 0).into_bytes();
            let whole = text(format!("bytes */{length}"));
            (416, refused, vec![(header::CONTENT_RANGE, whole)])
        }
        Some(&[(from, to)]) => {
            let cut = usize::try_from(from).ok().zip(usize::try_from(to).ok());
            let part = cut.and_then(|(from, to)| body.get(from..=to));
            let part = part.map(<[u8]>::to_vec).unwrap_or_default();
            let served = text(format!("bytes {from}-{to}/{length}"));
            (206, part, vec![accept, (header::CONTENT_RANGE, served)])
        }
        Some(_) => (200, body, vec![accept]),
    }
}

/// The ranges of `header` in a body of `length` bytes (at least 1), merged: `None` when it does
/// not read as `bytes=` and ranges (`from-to`, `from-`, `-suffix`) separated by commas, or when a
/// range starts after its end.
fn read(header: &str, length: u64) -> Option<Vec<(u64, u64)>> {
    let space = |c: char| c.is_ascii_whitespace() || c == '\u{b}';
    let unit = header
        .get(..6)
        .filter(|unit| unit.eq_ignore_ascii_case("bytes="));
    let list = unit.and(header.get(6..))?;
    let list = list.trim_start_matches(|c: char| c == ',' || space(c));
    let last = length.checked_sub(1)?;
    let digits = |text: &str| {
        let number = !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit());
        number.then(|| text.parse::<u64>().unwrap_or(u64::MAX))
    };
    let items: Vec<&str> = list.split(',').collect();
    let mut ranges = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        // Spaces go around a comma only, and none before the first range.
        let item = if index > 0 {
            item.trim_start_matches(space)
        } else {
            item
        };
        let item = if index + 1 < items.len() {
            item.trim_end_matches(space)
        } else {
            item
        };
        let (from, to) = item.split_once('-')?;
        let (from, to) = match (from.is_empty(), to.is_empty()) {
            (false, false) => (digits(from)?, digits(to)?.min(last)),
            (false, true) => (digits(from)?, last),
            (true, false) => (length.saturating_sub(digits(to)?), last),
            (true, true) => return None,
        };
        if from > to {
            return None;
        }
        ranges.push((from, to));
    }
    if ranges.len() < 2 {
        return Some(ranges);
    }
    // Sorted by start; from the last, a range that starts at most one byte after the end of the
    // one before it gives that one its end.
    ranges.sort_by_key(|&(from, _)| from);
    let mut earlier = ranges.into_iter().rev();
    let mut current = earlier.next()?;
    let mut merged = Vec::new();
    for mut before in earlier {
        if current.0 <= before.1.saturating_add(1) {
            before.1 = current.1;
        } else {
            merged.push(current);
        }
        current = before;
    }
    merged.push(current);
    merged.reverse();
    Some(merged)
}
