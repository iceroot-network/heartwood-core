//! PostgreSQL's `LIKE` on `bytea`, which the reference's `memo` criteria use.

/// A `LIKE` pattern that ends with its escape character, where a match reached it: PostgreSQL
/// refuses the statement then, and only then.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("LIKE pattern must not end with escape character")]
pub struct PatternError;

/// The outcome of matching a pattern from some position on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    /// The rest of the text matches.
    Match,
    /// The rest of the text does not match, but a later start of a `%` might.
    NoMatch,
    /// No later start of an enclosing `%` can match either.
    Abort,
}

/// Whether `value` matches the `LIKE` pattern `pattern`, byte by byte, as PostgreSQL matches a
/// `bytea`: `%` matches any run of bytes, `_` one byte, `\` makes the next byte literal; bytes
/// compare exactly, so the match is case-sensitive. A pattern that ends with `\` is an error when
/// the match reaches that byte, and matches or fails as usual otherwise.
pub fn like_bytes(pattern: &[u8], value: &[u8]) -> Result<bool, PatternError> {
    Ok(matches(value, pattern)? == Outcome::Match)
}

/// PostgreSQL's `MatchText` on bytes: `text` against `pattern`.
fn matches(mut text: &[u8], mut pattern: &[u8]) -> Result<Outcome, PatternError> {
    if pattern == b"%" {
        return Ok(Outcome::Match);
    }
    while let (Some(&t), Some(&p)) = (text.first(), pattern.first()) {
        match p {
            b'\\' => {
                pattern = rest(pattern);
                let &literal = pattern.first().ok_or(PatternError)?;
                if literal != t {
                    return Ok(Outcome::NoMatch);
                }
            }
            b'%' => return percent(text, rest(pattern)),
            b'_' => {}
            _ if p != t => return Ok(Outcome::NoMatch),
            _ => {}
        }
        text = rest(text);
        pattern = rest(pattern);
    }
    if !text.is_empty() {
        return Ok(Outcome::NoMatch);
    }
    // The text is used up: only a run of `%` can still match the empty rest.
    Ok(if pattern.iter().all(|&p| p == b'%') {
        Outcome::Match
    } else {
        Outcome::Abort
    })
}

/// The rest of a match after a `%`: `pattern` follows the `%`.
fn percent(mut text: &[u8], mut pattern: &[u8]) -> Result<Outcome, PatternError> {
    // A run of `%` and `_` after the `%` matches any text at least as long as its `_`s.
    loop {
        match pattern.first() {
            Some(b'%') => pattern = rest(pattern),
            Some(b'_') => {
                if text.is_empty() {
                    return Ok(Outcome::Abort);
                }
                text = rest(text);
                pattern = rest(pattern);
            }
            _ => break,
        }
    }
    let Some(&first) = pattern.first() else {
        return Ok(Outcome::Match);
    };
    // The pattern goes on with a literal byte: try each start of the text that has it.
    let first = if first == b'\\' {
        *pattern.get(1).ok_or(PatternError)?
    } else {
        first
    };
    while let Some(&t) = text.first() {
        if t == first {
            let outcome = matches(text, pattern)?;
            if outcome != Outcome::NoMatch {
                return Ok(outcome);
            }
        }
        text = rest(text);
    }
    Ok(Outcome::Abort)
}

/// `bytes` without its first byte.
fn rest(bytes: &[u8]) -> &[u8] {
    bytes.get(1..).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn like(pattern: &str, value: &str) -> Result<bool, PatternError> {
        like_bytes(pattern.as_bytes(), value.as_bytes())
    }

    #[test]
    fn literal_bytes_match_exactly_and_by_case() {
        assert_eq!(like("abc", "abc"), Ok(true));
        assert_eq!(like("abc", "ABC"), Ok(false));
        assert_eq!(like("abc", "abcd"), Ok(false));
        assert_eq!(like("abcd", "abc"), Ok(false));
        assert_eq!(like("", ""), Ok(true));
        assert_eq!(like("", "a"), Ok(false));
    }

    #[test]
    fn wildcards_match_runs_and_single_bytes() {
        assert_eq!(like("%", ""), Ok(true));
        assert_eq!(like("%%", ""), Ok(true));
        assert_eq!(like("a%", "abc"), Ok(true));
        assert_eq!(like("%c", "abc"), Ok(true));
        assert_eq!(like("%b%", "abc"), Ok(true));
        assert_eq!(like("%x%", "abc"), Ok(false));
        assert_eq!(like("a_c", "abc"), Ok(true));
        assert_eq!(like("a_c", "ac"), Ok(false));
        assert_eq!(like("___", "abc"), Ok(true));
        assert_eq!(like("%_%_", "a"), Ok(false));
        assert_eq!(like("%_%_", "ab"), Ok(true));
        assert_eq!(like("a%b%c", "aXbYc"), Ok(true));
        assert_eq!(like("a%b%c", "aXcYb"), Ok(false));
        // `_` is one byte, so a two-byte character needs two.
        assert_eq!(like("_", "é"), Ok(false));
        assert_eq!(like("__", "é"), Ok(true));
    }

    #[test]
    fn a_backslash_makes_the_next_byte_literal() {
        assert_eq!(like(r"a\%", "a%"), Ok(true));
        assert_eq!(like(r"a\%", "ab"), Ok(false));
        assert_eq!(like(r"a\_", "a_"), Ok(true));
        assert_eq!(like(r"a\_", "ab"), Ok(false));
        assert_eq!(like(r"a\\", r"a\"), Ok(true));
        assert_eq!(like(r"\a", "a"), Ok(true));
        assert_eq!(like(r"%\%", "50%"), Ok(true));
    }

    #[test]
    fn a_trailing_backslash_is_an_error_only_where_the_match_reaches_it() {
        assert_eq!(like(r"ab\", "abc"), Err(PatternError));
        assert_eq!(like(r"%\", "abc"), Err(PatternError));
        // The first byte already differs, or the text ends first.
        assert_eq!(like(r"ab\", "xyz"), Ok(false));
        assert_eq!(like(r"ab\", "ab"), Ok(false));
        assert_eq!(like(r"\", ""), Ok(false));
    }

    /// Patterns and values with the answers of PostgreSQL 14 (`bytea LIKE bytea`, C locale), from
    /// a run of several thousand random cases that all agreed.
    #[test]
    fn matches_as_postgresql_does() {
        let cases: &[(&str, &str, Result<bool, PatternError>)] = &[
            ("", "\\%é", Ok(false)),
            ("", "__a\\%a", Ok(false)),
            ("", "_bé", Ok(false)),
            ("", "a_", Ok(false)),
            ("", "baé_%", Ok(false)),
            ("", "béaéé", Ok(false)),
            ("%", "%b\\bb", Ok(true)),
            ("%", "%b_aa", Ok(true)),
            ("%", "\\a__\\%", Ok(true)),
            ("%", "_%\\", Ok(true)),
            ("%", "_%a__\\", Ok(true)),
            ("%", "aéébab", Ok(true)),
            ("A", "ba", Ok(false)),
            ("\\", "\\\\b\\bb", Err(PatternError)),
            ("\\", "\\ab__", Err(PatternError)),
            ("\\", "_b%ab\\", Err(PatternError)),
            ("\\", "_éa", Err(PatternError)),
            ("\\", "bébb_%", Err(PatternError)),
            ("\\", "é_a\\%", Err(PatternError)),
            ("\\", "éé%", Err(PatternError)),
            ("a", "a", Ok(true)),
            ("b", "b", Ok(true)),
            ("é", "b", Ok(false)),
            ("é", "ééb%", Ok(false)),
            ("%%", "%_", Ok(true)),
            ("%%", "é%b\\", Ok(true)),
            ("%_", "\\", Ok(true)),
            ("%é", "\\\\bbé", Ok(true)),
            ("\\%", "_a_aé%", Ok(false)),
            ("_%", "bb", Ok(true)),
            ("a%", "aaa%é%", Ok(true)),
            ("aa", "%", Ok(false)),
            ("b\\", "baé", Err(PatternError)),
            ("b_", "\\_éba%", Ok(false)),
            ("%%é", "abé", Ok(true)),
            ("%_%", "a", Ok(true)),
            ("%b%", "b%%_", Ok(true)),
            ("\\%\\", "%\\", Err(PatternError)),
            ("\\%b", "\\b", Ok(false)),
            ("\\_%", "__%", Ok(true)),
            ("_%%", "b_a__", Ok(true)),
            ("_%\\", "b\\a%\\%", Err(PatternError)),
            ("_%_", "b\\", Ok(true)),
            ("_a%", "%a", Ok(true)),
            ("_ab", "bab", Ok(true)),
            ("_b%", "ab", Ok(true)),
            ("_é_", "bbb\\aé", Ok(false)),
            ("a_c", "abc", Ok(true)),
            ("abc", "abc", Ok(true)),
            ("b_%", "b%b", Ok(true)),
            ("éAé", "\\", Ok(false)),
            ("é\\_", "é\\_", Ok(false)),
            ("é_%", "ébéb%\\", Ok(true)),
            ("éa%", "_é", Ok(false)),
            ("%%_%", "a", Ok(true)),
            ("\\%%_", "%%b", Ok(true)),
            ("\\\\_a", "\\_a", Ok(true)),
            ("\\\\é\\", "\\é\\", Err(PatternError)),
            ("\\é%\\", "é%\\", Err(PatternError)),
            ("_%%_", "_b_", Ok(true)),
            ("_\\\\_", "é\\_", Ok(false)),
            ("_b_\\", "_b%\\", Err(PatternError)),
            ("a\\ab", "aab", Ok(true)),
            ("é%%é", "é%aé", Ok(true)),
            ("éAAa", "é_%_%", Ok(false)),
            ("é\\\\\\", "é\\\\", Err(PatternError)),
            ("%%é%b", "é%ééb", Ok(true)),
            ("%\\a%b", "_\\abb", Ok(true)),
            ("%\\a%é", "\\%abé", Ok(true)),
            ("%_a__", "\\abb", Ok(true)),
            ("%a\\b_", "%abb_", Ok(false)),
            ("A\\_\\A", "", Ok(false)),
            ("\\%b_%", "éb_b", Ok(false)),
            ("\\\\bé\\", "\\bé\\", Err(PatternError)),
            ("\\a\\_\\", "abé\\", Ok(false)),
            ("_%b%é", "_abéé", Ok(true)),
            ("_a%b%", "éab", Ok(false)),
            ("_b%%\\", "bb\\%__", Err(PatternError)),
            ("b\\%A\\", "b\\%b", Ok(false)),
            ("%%%_%\\", "bb%_%b", Err(PatternError)),
            ("%%__b_", "_b%_b_", Ok(true)),
            ("%%a\\%_", "ébaaé%", Ok(false)),
            ("\\a\\éab", "éabéab", Ok(false)),
            ("é\\baé%", "__\\a", Ok(false)),
            ("éa_bé%", "%éébé", Ok(false)),
            ("%%%\\%%%", "_aaba", Ok(false)),
            ("%b%%b__", "ab%\\b_é", Ok(false)),
            ("___%ab_", "ab%\\ab\\", Ok(true)),
            ("a%a\\__%", "abab_\\", Ok(false)),
            ("%%a\\bé_%", "%%abébb", Ok(true)),
            ("%%é%__%\\", "a%éab%", Err(PatternError)),
            ("%_%%\\%__", "%b%\\_a", Ok(false)),
            ("\\%a%a\\b%", "\\%abab", Ok(false)),
            ("\\_%%\\%_%", "\\_%\\é_", Ok(false)),
            ("\\a%b_ab_", "ab_ab_", Ok(true)),
            ("\\bé\\b\\ab", "béb\\ab", Ok(false)),
            ("_%%%%%%b", "_%%a_ab", Ok(true)),
            ("a\\\\\\_b_%", "a\\\\_b%", Ok(false)),
            ("ab%_\\\\%%", "ab%_\\é", Ok(true)),
            ("ab_\\%_%%", "ab_\\__é", Ok(false)),
            ("b%\\_b\\%%", "b\\_b%%", Ok(true)),
            ("%baa%b\\é\\", "abaabé_", Err(PatternError)),
            ("\\abb%\\\\b%", "abb\\%b", Ok(false)),
            ("b%__\\\\ab%", "bab\\ab%", Ok(true)),
            ("é%_%\\%%\\é", "éé\\é%é", Ok(true)),
            ("%%%_a\\b%%%", "ab\\aba%b", Ok(true)),
            ("a\\b\\é_a%_\\", "abé\\ab%", Err(PatternError)),
            ("a\\babaé_bb", "ababaéabb", Ok(true)),
            ("%b_b\\\\a%%bé", "abab\\abbé", Ok(true)),
            ("\\a__bb_\\a\\b", "aabbbabab", Ok(false)),
        ];
        for (pattern, value, expected) in cases {
            assert_eq!(
                like(pattern, value),
                *expected,
                "{pattern:?} LIKE {value:?}"
            );
        }
    }
}
