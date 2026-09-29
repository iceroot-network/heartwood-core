//! The collation of the wallet search against Node 18.20.8's `localeCompare(b, "en", {numeric:
//! true})`, over the username alphabet and Base58: single characters, digit runs with and without
//! leading zeros, strings that tie in the primary weights (case, zeros) and random strings.
//!
//! `tests/data/collate.json` is made by `tests/data/generate.js` from Node itself. The
//! `js.localeCompare` records of the vector class V34 are compared too: from `tests/vectors`, where
//! a missing class fails the test, or from the run `API_VECTORS` names (a path from the workspace
//! root, such as a local oracle run in `target/vectors/local`), held to the class's minimum.

use std::cmp::Ordering;

use heartwood_api::search::locale_compare;

mod data;

fn sign(order: Ordering) -> i64 {
    order as i64
}

#[test]
fn matches_node_on_the_known_pairs() {
    let pairs: Vec<(String, String, i64)> =
        serde_json::from_str(include_str!("data/collate.json")).unwrap();
    assert!(pairs.len() >= 700);
    for (a, b, expected) in &pairs {
        assert_eq!(sign(locale_compare(a, b)), *expected, "{a:?} against {b:?}");
    }
    let mixed = pairs
        .iter()
        .filter(|(a, b, _)| a.to_lowercase() == b.to_lowercase() && a != b)
        .count();
    let equal = pairs.iter().filter(|(_, _, order)| *order == 0).count();
    assert!(mixed >= 20, "{mixed} pairs differ only in case");
    assert!(equal >= 5, "{equal} pairs are equal");
}

#[test]
fn matches_node_on_the_vector_records() {
    let Some(records) = data::vector_records("js.localeCompare") else {
        return;
    };
    for record in &records {
        let input = &record["input"];
        let (a, b) = (input["a"].as_str().unwrap(), input["b"].as_str().unwrap());
        assert_eq!(
            sign(locale_compare(a, b)),
            record["output"]["order"],
            "{a:?} against {b:?}"
        );
    }
}

#[test]
fn the_order_of_the_alphabet() {
    // Punctuation, then numbers, then letters, each in ICU's order (the marks: _ ! . @ & $).
    let order = [
        "_", "!", ".", "@", "&", "$", "0", "1", "9", "10", "a", "b", "z",
    ];
    for (i, a) in order.iter().enumerate() {
        for b in &order[i + 1..] {
            assert_eq!(locale_compare(a, b), Ordering::Less, "{a} before {b}");
            assert_eq!(locale_compare(b, a), Ordering::Greater, "{b} after {a}");
        }
    }
}

#[test]
fn digit_runs_are_numbers_and_zeros_do_not_count() {
    assert_eq!(locale_compare("a9", "a10"), Ordering::Less);
    assert_eq!(locale_compare("a09", "a9"), Ordering::Equal);
    assert_eq!(locale_compare("0", "00"), Ordering::Equal);
    assert_eq!(locale_compare("007b", "7a"), Ordering::Greater);
    assert_eq!(locale_compare("a1b2", "a01b02"), Ordering::Equal);
    assert_eq!(
        locale_compare("999999999999999999999", "1000000000000000000000"),
        Ordering::Less
    );
}

#[test]
fn case_decides_only_a_tie_and_lower_case_comes_first() {
    assert_eq!(locale_compare("ab", "AB"), Ordering::Less);
    assert_eq!(locale_compare("Ab", "aB"), Ordering::Greater);
    assert_eq!(locale_compare("AB", "ac"), Ordering::Less);
    assert_eq!(locale_compare("a", "A"), Ordering::Less);
}

#[test]
fn a_prefix_comes_first_and_the_order_is_antisymmetric() {
    let alphabet: Vec<String> = "aAbB09_.!@$&zZ"
        .chars()
        .flat_map(|a| {
            [
                "".to_owned(),
                a.to_string(),
                format!("{a}{a}"),
                format!("{a}1"),
                format!("{a}01"),
            ]
        })
        .collect();
    for a in &alphabet {
        for b in &alphabet {
            assert_eq!(
                locale_compare(a, b),
                locale_compare(b, a).reverse(),
                "{a:?} against {b:?}"
            );
        }
    }
    assert_eq!(locale_compare("", "a"), Ordering::Less);
    assert_eq!(locale_compare("ab", "abc"), Ordering::Less);
}
