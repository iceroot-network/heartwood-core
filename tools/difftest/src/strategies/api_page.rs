//! P21's small-limit branch: limits 1 to 3 with random offsets over items of mixed JSON types and
//! plain objects at random positions, for `api.searchPage`.
//!
//! Each list has a kind (numbers, strings, booleans, BigNumbers, Semvers or plain objects) that
//! most of its values are of, with `null`, absent values and values of any kind among them, so
//! that the tree reaches a comparison of mixed types only for some of the cases: the pairs it
//! compares decide whether the reference answers 500.

use std::sync::Arc;

use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use serde_json::{Value, json};

use super::kernel::OpCase;
use super::transactions::TxContext;

/// The rule height of the cases.
const HEIGHT: u64 = 2;

/// A value of a kind: numbers, strings, booleans, BigNumbers, Semvers (one that is not valid
/// among them) or plain objects and arrays.
fn scalar(kind: u8) -> BoxedStrategy<Value> {
    match kind {
        0 => prop_oneof![
            4 => (-3i64..=3).prop_map(|number| json!(number)),
            1 => prop::sample::select(vec![0.5, 1.5, -2.5]).prop_map(|number| json!(number)),
        ]
        .boxed(),
        1 => "[a-cA-Cé𐀀\u{fffd}]{0,3}".prop_map(Value::String).boxed(),
        2 => any::<bool>().prop_map(Value::Bool).boxed(),
        3 => (any::<bool>(), "0|[1-9][0-9]{0,22}")
            .prop_map(|(negative, digits)| {
                let sign = if negative && digits != "0" { "-" } else { "" };
                json!({"$bigNumber": format!("{sign}{digits}")})
            })
            .boxed(),
        4 => prop::sample::select(vec![
            "1.0.0",
            "1.2.3",
            "1.10.0",
            "2.0.0",
            "1.2.3-beta",
            "v1.0.0",
            "0.9.9",
            "bad",
        ])
        .prop_map(|version| json!({"$semver": version}))
        .boxed(),
        _ => prop_oneof![(0i64..3).prop_map(|n| json!({"o": n})), Just(json!([1]))].boxed(),
    }
}

/// The sorted value of an item: `None` is an absent one.
fn value(kind: u8) -> BoxedStrategy<Option<Value>> {
    prop_oneof![
        12 => scalar(kind).prop_map(Some),
        2 => Just(Some(Value::Null)),
        2 => Just(None),
        2 => (0u8..6).prop_flat_map(scalar).prop_map(Some),
    ]
    .boxed()
}

/// The sorting: one or two properties, either way round.
fn sorting() -> BoxedStrategy<Value> {
    let ways = prop::sample::select(vec!["asc", "desc"]);
    (ways.clone(), ways, 0u8..4)
        .prop_map(|(first, second, shape)| match shape {
            0 | 1 => json!([{"property": "v", "direction": first}]),
            2 => json!([{"property": "v", "direction": first}, {"property": "w", "direction": second}]),
            _ => json!([{"property": "w", "direction": second}, {"property": "v", "direction": first}]),
        })
        .boxed()
}

/// The cases of P21's small-limit branch.
pub fn page_cases(context: &Arc<TxContext>) -> BoxedStrategy<OpCase> {
    let _ = context;
    let items = (0u8..6).prop_flat_map(|kind| {
        prop::collection::vec((value(kind), prop::option::of(0i64..3)), 0..=9).prop_map(|rows| {
            let items: Vec<Value> = rows
                .into_iter()
                .enumerate()
                .map(|(id, (v, w))| {
                    let mut item = json!({"id": id});
                    for (key, member) in [("v", v), ("w", w.map(|number| json!(number)))] {
                        if let Some(member) = member {
                            item[key] = member;
                        }
                    }
                    item
                })
                .collect();
            items
        })
    });
    (items, sorting(), 0u64..=4, 1u64..=3)
        .prop_map(|(items, sorting, offset, limit)| {
            let input = json!({"items": items, "sorting": sorting, "pagination": {"offset": offset, "limit": limit}});
            ("api.searchPage", HEIGHT, input)
        })
        .boxed()
}
