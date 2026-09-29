//! ICU's root collation as `a.localeCompare(b, "en", {numeric: true})` gives it under Node 18.20.8
//! (ICU 74.2), for the characters of usernames (`[a-z0-9!@$&_.]`) and of Base58.

use std::cmp::Ordering;

/// `a.localeCompare(b, "en", {numeric: true})`.
pub fn locale_compare(a: &str, b: &str) -> Ordering {
    let _ = (a, b);
    todo!("the numeric collation")
}
