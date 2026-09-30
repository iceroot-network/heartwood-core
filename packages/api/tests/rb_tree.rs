//! The red-black tree that `getTop` keeps, against the reference's `functional-red-black-tree`:
//! the pairs it compares, in order, over scripts of inserts and removes and over `getTop` of a
//! list of values (kind `js.rbTrace` of class V34), and the order it keeps.

mod page_vectors;

use std::cmp::Ordering;

use heartwood_api::errors::ApiError;
use heartwood_api::search::rb_tree::{RbTree, top};
use serde_json::{Value, json};

/// The order of two numbers, and the pair recorded.
fn numbers(values: &[f64], seen: &mut Vec<[usize; 2]>, a: usize, b: usize) -> Ordering {
    seen.push([a, b]);
    values[a].partial_cmp(&values[b]).unwrap()
}

/// Runs the tree over a script, giving the pairs compared and the keys.
fn script(values: &[f64], steps: &[(bool, usize)]) -> (Vec<[usize; 2]>, Vec<usize>) {
    let mut seen = Vec::new();
    let mut tree = RbTree::default();
    let mut compare = |a: &usize, b: &usize| Ok::<_, ApiError>(numbers(values, &mut seen, *a, *b));
    for &(insert, key) in steps {
        if insert {
            tree.insert(key, &mut compare).unwrap();
        } else {
            tree.remove(&key, &mut compare).unwrap();
        }
    }
    let keys = tree.into_keys();
    (seen, keys)
}

/// The record's answer.
fn answer(input: &Value) -> Value {
    let values: Vec<f64> = input["values"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_f64().unwrap())
        .collect();
    if let Some(steps) = input["script"].as_array() {
        let steps: Vec<(bool, usize)> = steps
            .iter()
            .map(|step| (step[0] == "insert", step[1].as_u64().unwrap() as usize))
            .collect();
        let (compared, keys) = script(&values, &steps);
        return json!({"compared": compared, "keys": keys});
    }
    let mut seen = Vec::new();
    let count = input["count"].as_u64().unwrap() as usize;
    let keys = top(
        count,
        0..values.len(),
        |a: &(usize, usize), b: &(usize, usize)| {
            Ok(numbers(&values, &mut seen, a.0, b.0).then(a.0.cmp(&b.0)))
        },
    )
    .unwrap();
    json!({"compared": seen, "keys": keys})
}

#[test]
fn the_comparisons_match_the_reference() {
    let Some(records) = page_vectors::records("js.rbTrace") else {
        return;
    };
    for record in &records {
        assert_eq!(
            answer(&record["input"]),
            record["output"],
            "{}",
            record["name"]
        );
    }
}

/// A generator of numbers below a bound.
fn generator(seed: u64) -> impl FnMut(usize) -> usize {
    let mut state = seed;
    move |bound| {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        usize::try_from(state >> 33).unwrap() % bound
    }
}

#[test]
fn the_keys_stay_in_order_over_random_scripts() {
    let mut random = generator(7);
    for round in 0..300 {
        let values: Vec<f64> = (0..40).map(|_| random(12) as f64).collect();
        let mut held: Vec<usize> = Vec::new();
        let mut steps = Vec::new();
        for key in 0..values.len() {
            if !held.is_empty() && random(3) == 0 {
                steps.push((false, held.swap_remove(random(held.len()))));
            } else {
                steps.push((true, key));
                held.push(key);
            }
        }
        let (_, keys) = script(&values, &steps);
        let ordered: Vec<f64> = keys.iter().map(|&key| values[key]).collect();
        assert!(ordered.is_sorted(), "round {round}: {ordered:?}");
    }
}

#[test]
fn top_keeps_the_first_of_equals_and_the_lowest() {
    let values = [3.0, 1.0, 3.0, 0.0, 1.0, 2.0];
    for (count, expected) in [
        (0, vec![]),
        (1, vec![3]),
        (3, vec![3, 1, 4]),
        (9, vec![3, 1, 4, 5, 0, 2]),
    ] {
        let mut seen = Vec::new();
        let kept = top(
            count,
            0..values.len(),
            |a: &(usize, usize), b: &(usize, usize)| {
                Ok(numbers(&values, &mut seen, a.0, b.0).then(a.0.cmp(&b.0)))
            },
        )
        .unwrap();
        assert_eq!(kept, expected, "count {count}");
    }
}

#[test]
fn the_first_error_of_the_comparison_is_the_answer() {
    let mut calls = 0;
    let refused = top(2, 0..5, |_: &(usize, usize), _: &(usize, usize)| {
        calls += 1;
        if calls == 3 {
            Err(ApiError::internal())
        } else {
            Ok(Ordering::Less)
        }
    });
    assert_eq!(refused, Err(ApiError::internal()));
    assert_eq!(calls, 3);
}
