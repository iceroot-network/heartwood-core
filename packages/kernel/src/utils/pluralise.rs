//! The reference's `pluralise`, which the pool's and the handlers' messages use for counts.

/// `word`, with an `s` added unless `count` is 1; when `inclusive`, preceded by `count` as
/// JavaScript's `toLocaleString()` writes it with English digit grouping, and a space: for
/// example `pluralise("second", 30, true)` is `"30 seconds"` and `pluralise("byte", 2_000_000,
/// true)` is `"2,000,000 bytes"`.
pub fn pluralise(word: &str, count: i128, inclusive: bool) -> String {
    let _ = (word, count, inclusive);
    todo!("pluralise")
}
