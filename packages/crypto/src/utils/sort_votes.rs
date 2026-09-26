//! The canonical order of a vote asset, and the key order of the reference's vote object.
//!
//! The reference keeps votes in a JavaScript object, whose keys iterate in a fixed order: keys that
//! are array indices (`"0"` to `"4294967294"` in canonical decimal form) first, in ascending
//! numeric order, then every other key in insertion order. Delegate names always contain a
//! character that is not a digit, so a vote that the rules accept is never reordered; the order
//! only shows in the id of a transaction whose vote asset names something else, which the rules
//! then remove.

use crate::transactions::types::Votes;

/// `votes` in canonical order: basis points descending, then names ascending by UTF-16 code unit,
/// which for the delegate names the rules accept (ASCII only) is byte order. As in the reference,
/// the result is a vote object again, so a name that is an array index comes first.
pub fn sort_votes(votes: &Votes) -> Votes {
    let mut entries: Vec<(&String, u16)> = votes
        .iter()
        .map(|(name, &basis_points)| (name, basis_points))
        .collect();
    entries.sort_by(|a, b| {
        b.1.cmp(&a.1)
            .then_with(|| a.0.encode_utf16().cmp(b.0.encode_utf16()))
    });
    let mut sorted = Votes::with_capacity(entries.len());
    for (name, basis_points) in entries {
        insert_as_object_key(&mut sorted, name.clone(), basis_points);
    }
    sorted
}

/// Set `votes[name] = basis_points` with the key order of a JavaScript object: a new array-index
/// key goes before every other key and after the smaller array indices; any other new key goes
/// last; an existing key keeps its place and takes the new value.
pub(crate) fn insert_as_object_key(votes: &mut Votes, name: String, basis_points: u16) {
    if let Some(value) = votes.get_mut(&name) {
        *value = basis_points;
        return;
    }
    match array_index(&name) {
        Some(index) => {
            let position = votes
                .keys()
                .take_while(|key| array_index(key).is_some_and(|other| other < index))
                .count();
            votes.shift_insert(position, name, basis_points);
        }
        None => {
            votes.insert(name, basis_points);
        }
    }
}

/// The value of `key` if it is a JavaScript array index: the canonical decimal text of an integer
/// from 0 to `2^32 − 2`.
fn array_index(key: &str) -> Option<u32> {
    let canonical = key == "0" || (!key.starts_with('0') && !key.is_empty());
    if !canonical || key.len() > 10 || !key.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    key.parse::<u32>().ok().filter(|&index| index != u32::MAX)
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn votes(entries: &[(&str, u16)]) -> Votes {
        entries
            .iter()
            .map(|&(name, basis_points)| (name.to_owned(), basis_points))
            .collect()
    }

    fn names(votes: &Votes) -> Vec<&str> {
        votes.keys().map(String::as_str).collect()
    }

    #[test]
    fn weights_then_names() {
        let sorted = sort_votes(&votes(&[
            ("b2", 2500),
            ("a9", 2500),
            ("b10", 2500),
            ("a10", 2500),
        ]));
        assert_eq!(names(&sorted), ["a10", "a9", "b10", "b2"]);
        let sorted = sort_votes(&votes(&[("c", 4000), ("b", 2000), ("a", 4000)]));
        assert_eq!(names(&sorted), ["a", "c", "b"]);
        assert!(sort_votes(&Votes::new()).is_empty());
    }

    #[test]
    fn array_indices_come_first() {
        assert_eq!(array_index("0"), Some(0));
        assert_eq!(array_index("4294967294"), Some(4_294_967_294));
        for key in [
            "",
            "01",
            "-1",
            "+1",
            "1.0",
            "4294967295",
            "99999999999",
            "1a",
            "a1",
        ] {
            assert_eq!(array_index(key), None, "{key:?}");
        }
        let mut object = Votes::new();
        for (name, value) in [("b", 1), ("10", 2), ("a", 3), ("2", 4), ("b", 5), ("02", 6)] {
            insert_as_object_key(&mut object, name.to_owned(), value);
        }
        assert_eq!(names(&object), ["2", "10", "b", "a", "02"]);
        assert_eq!(object["b"], 5);
        let sorted = sort_votes(&votes(&[("b", 6000), ("1", 4000)]));
        assert_eq!(names(&sorted), ["1", "b"]);
    }

    #[test]
    fn utf16_order_for_other_names() {
        // U+10000 sorts after U+FF61 in UTF-8 byte order but before it in UTF-16 code units.
        let sorted = sort_votes(&votes(&[("\u{10000}", 5000), ("\u{ff61}", 5000)]));
        assert_eq!(names(&sorted), ["\u{10000}", "\u{ff61}"]);
    }

    proptest! {
        #[test]
        fn sorted_is_a_permutation_in_order(
            entries in prop::collection::vec(("[a-z_.!@$&0-9]{1,6}", 1u16..=10_000), 0..20)
        ) {
            let input: Votes = entries.into_iter().collect();
            let sorted = sort_votes(&input);
            prop_assert_eq!(sorted.len(), input.len());
            for (name, basis_points) in &input {
                prop_assert_eq!(sorted.get(name), Some(basis_points));
            }
            let plain: Vec<_> = sorted
                .iter()
                .filter(|(name, _)| array_index(name).is_none())
                .collect();
            for pair in plain.windows(2) {
                let (a, b) = (pair[0], pair[1]);
                prop_assert!(a.1 > b.1 || (a.1 == b.1 && a.0.as_bytes() < b.0.as_bytes()));
            }
            prop_assert_eq!(sort_votes(&sorted), sorted);
        }
    }
}
