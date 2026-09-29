//! The V8 sort against Node 18's `Array.prototype.sort`: the same comparators (a total order, one
//! with three-way ties and cycles, one without structure, one that is an order with now and then
//! a flipped answer) over sorted, reversed, random and blocky inputs of 1 to 257 items give the
//! same order, and the sort is not a stable sort.
//!
//! `tests/data/v8-sort.json` is made by `tests/data/generate.js` from Node's own sort.

use std::cmp::Ordering;

use heartwood_api::search::v8_sort;
use serde_json::Value;

/// The mixing function of the generator (`mix` in `generate.js`).
fn mix(a: i64, b: i64, seed: i64) -> u32 {
    let (a, b, seed) = (a as i32, b as i32, seed as i32);
    let mut h = (a ^ seed).wrapping_mul(0x9e37_79b1_u32 as i32)
        ^ b.wrapping_add(0x7f4a_7c15)
            .wrapping_mul(0x85eb_ca6b_u32 as i32);
    h ^= ((h as u32) >> 15) as i32;
    h = h.wrapping_mul(0x2c1b_3c6d);
    h ^= ((h as u32) >> 13) as i32;
    h as u32
}

fn ordering(sign: i64) -> Ordering {
    sign.cmp(&0)
}

fn compare(kind: &str, seed: i64, a: i64, b: i64) -> Ordering {
    let ascending = (a - b).signum();
    ordering(match kind {
        "ascending" => ascending,
        "cycle" => [0, -1, 1][(((a - b) % 3) + 3) as usize % 3],
        "hash" => i64::from(mix(a, b, seed) % 3) - 1,
        "noisy" => {
            if mix(a, b, seed).is_multiple_of(9) {
                -ascending
            } else {
                ascending
            }
        }
        other => panic!("comparator {other}"),
    })
}

fn cases() -> Vec<Value> {
    serde_json::from_str(include_str!("data/v8-sort.json")).unwrap()
}

fn numbers(value: &Value) -> Vec<i64> {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n.as_i64().unwrap())
        .collect()
}

#[test]
fn matches_node_on_every_case() {
    let cases = cases();
    assert!(cases.len() >= 400);
    for case in &cases {
        let (kind, seed) = (
            case["comparator"].as_str().unwrap(),
            case["seed"].as_i64().unwrap(),
        );
        let mut items = numbers(&case["items"]);
        v8_sort(&mut items, |a, b| compare(kind, seed, *a, *b));
        assert_eq!(
            items,
            numbers(&case["sorted"]),
            "{kind} {} of {} items, seed {seed}",
            case["shape"],
            items.len()
        );
    }
}

#[test]
fn the_cases_have_cycles_and_merges() {
    // The comparators that are no order must show: an answer of Node's that a stable sort of the
    // same comparator would not give (checked against a stable insertion sort).
    let (mut differing, mut merging) = (0, 0);
    for case in cases() {
        let (kind, seed) = (
            case["comparator"].as_str().unwrap(),
            case["seed"].as_i64().unwrap(),
        );
        let items = numbers(&case["items"]);
        let mut stable: Vec<i64> = Vec::new();
        for item in &items {
            // A stable insertion sort: after the last item that does not sort after it.
            let at = stable
                .iter()
                .rposition(|x| compare(kind, seed, *item, *x) != Ordering::Less);
            stable.insert(at.map_or(0, |i| i + 1), *item);
        }
        differing += usize::from(stable != numbers(&case["sorted"]));
        merging += usize::from(items.len() > 64);
    }
    assert!(
        differing >= 100,
        "{differing} cases differ from a stable sort"
    );
    assert!(merging >= 100, "{merging} cases are long enough to merge");
}

#[test]
fn small_inputs_and_a_total_order() {
    let mut none: Vec<i64> = Vec::new();
    v8_sort(&mut none, |a, b| a.cmp(b));
    assert!(none.is_empty());
    let mut one = vec![7];
    v8_sort(&mut one, |_, _| {
        unreachable!("one item needs no comparison")
    });
    assert_eq!(one, [7]);
    let mut items: Vec<i64> = (0..500).map(|i| (i * 7919) % 503).collect();
    let mut expected = items.clone();
    expected.sort_unstable();
    v8_sort(&mut items, |a, b| a.cmp(b));
    assert_eq!(items, expected);
}

#[test]
fn equal_items_keep_their_order() {
    // A total order with ties: V8's sort is stable for it, as the standard one is.
    let mut items: Vec<(i64, usize)> = (0..300).map(|i| ((i * 31) % 10, i as usize)).collect();
    let mut expected = items.clone();
    expected.sort_by_key(|item| item.0);
    v8_sort(&mut items, |a, b| a.0.cmp(&b.0));
    assert_eq!(items, expected);
}
