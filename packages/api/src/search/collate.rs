//! ICU's root collation as `a.localeCompare(b, "en", {numeric: true})` gives it under Node 18.20.8
//! (ICU 74.2), for the characters of usernames (`[a-z0-9!@$&_.]`) and of Base58.
//!
//! In that alphabet a string is a run of primary weights (punctuation, then digit runs valued as
//! numbers, then letters without regard to case) and, when those tie, the case of its letters
//! (lower before upper). Leading zeros of a number make no difference at all. Characters outside
//! the alphabet come after it, by code point; they are not part of what the reference compares.

use std::cmp::Ordering;

/// The primary weights of the alphabet's punctuation, in ICU's order.
const PUNCTUATION: &str = "_!.@&$";

#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum Primary {
    Punctuation(usize),
    /// A digit run: its significant digits' count, then the digits.
    Number(usize, String),
    Letter(char),
    Other(char),
}

/// The primary weights of `text` and the case of its letters (true for upper case).
fn keys(text: &str) -> (Vec<Primary>, Vec<bool>) {
    let (mut primary, mut upper) = (Vec::new(), Vec::new());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c.is_ascii_digit() {
            let mut digits = String::from(c);
            while let Some(d) = chars.next_if(char::is_ascii_digit) {
                digits.push(d);
            }
            let digits = digits.trim_start_matches('0');
            primary.push(Primary::Number(digits.len(), digits.to_owned()));
        } else if let Some(rank) = PUNCTUATION.find(c) {
            primary.push(Primary::Punctuation(rank));
        } else if c.is_ascii_alphabetic() {
            primary.push(Primary::Letter(c.to_ascii_lowercase()));
            upper.push(c.is_ascii_uppercase());
        } else {
            primary.push(Primary::Other(c));
        }
    }
    (primary, upper)
}

/// `a.localeCompare(b, "en", {numeric: true})`.
pub fn locale_compare(a: &str, b: &str) -> Ordering {
    let (primary_a, upper_a) = keys(a);
    let (primary_b, upper_b) = keys(b);
    primary_a
        .cmp(&primary_b)
        .then_with(|| upper_a.cmp(&upper_b))
}
