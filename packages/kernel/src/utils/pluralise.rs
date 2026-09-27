//! The reference's `pluralise`, which the pool's and the handlers' messages use for counts.

use heartwood_crypto::utils::js;

/// `word`, with an `s` added unless `count` is 1; when `inclusive`, preceded by `count` as
/// JavaScript's `toLocaleString()` writes it with English digit grouping, and a space: for
/// example `pluralise("second", 30, true)` is `"30 seconds"` and `pluralise("byte", 2_000_000,
/// true)` is `"2,000,000 bytes"`.
pub fn pluralise(word: &str, count: i128, inclusive: bool) -> String {
    let suffix = if count == 1 { "" } else { "s" };
    if inclusive {
        format!("{} {word}{suffix}", js::to_locale_integer(count))
    } else {
        format!("{word}{suffix}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_and_words() {
        assert_eq!(pluralise("second", 30, true), "30 seconds");
        assert_eq!(pluralise("second", 1, true), "1 second");
        assert_eq!(pluralise("more block", 1, true), "1 more block");
        assert_eq!(pluralise("byte", 2_000_000, true), "2,000,000 bytes");
        assert_eq!(pluralise("transaction", 0, true), "0 transactions");
        assert_eq!(pluralise("transaction", -1, true), "-1 transactions");
        assert_eq!(pluralise("vote", 1, false), "vote");
        assert_eq!(pluralise("vote", 2, false), "votes");
    }
}
