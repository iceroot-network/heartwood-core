//! The pagination service's `getPage` over items, against the reference (kind `api.searchPage` of
//! class V34): the page and its count, or the 500 of a comparison that throws (mixed types, plain
//! objects, a Semver that is not valid), which depends on the pairs the tree compares.

mod page_vectors;

use heartwood_api::pagination::Pagination;
use heartwood_api::search::{Direction, Sorting, page};
use serde_json::{Value, json};

/// The sorting of a record's input.
fn sorting(input: &Value) -> Sorting {
    Sorting(
        input["sorting"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| {
                let direction = if entry["direction"] == "desc" {
                    Direction::Desc
                } else {
                    Direction::Asc
                };
                (entry["property"].as_str().unwrap().to_owned(), direction)
            })
            .collect(),
    )
}

/// The page of items, as the record's output shapes it, or `None` for a 500.
fn run(items: Vec<Value>, sorting: &Sorting, offset: u64, limit: u64) -> Option<Value> {
    let page = page(Pagination { offset, limit }, sorting, items.into_iter()).ok()?;
    Some(json!({"page": {
        "results": page.results,
        "totalCount": page.total_count,
        "meta": {"totalCountIsEstimate": page.total_count_is_estimate},
    }}))
}

#[test]
fn the_pages_and_the_refusals_match_the_reference() {
    let Some(records) = page_vectors::records("api.searchPage") else {
        return;
    };
    let (mut pages, mut refused) = (0, 0);
    for record in &records {
        let input = &record["input"];
        let items = input["items"].as_array().unwrap().clone();
        let pagination = &input["pagination"];
        let (offset, limit) = (
            pagination["offset"].as_u64().unwrap(),
            pagination["limit"].as_u64().unwrap(),
        );
        let ours = run(items, &sorting(input), offset, limit);
        match record.get("error") {
            Some(_) => {
                assert_eq!(ours, None, "{}", record["name"]);
                refused += 1;
            }
            None => {
                assert_eq!(ours.as_ref(), Some(&record["output"]), "{}", record["name"]);
                pages += 1;
            }
        }
    }
    // Both answers are in the records, so a sort that never throws, or one that always does, fails.
    assert!(
        pages > 100 && refused > 50,
        "{pages} pages, {refused} refusals"
    );
}

fn asc(property: &str) -> Sorting {
    Sorting(vec![(property.to_owned(), Direction::Asc)])
}

#[test]
fn a_page_is_a_slice_of_the_order() {
    let items: Vec<Value> = [5, 3, 8, 1, 4].iter().map(|v| json!({"v": v})).collect();
    let got = run(items.clone(), &asc("v"), 1, 2).unwrap();
    assert_eq!(got["page"]["results"], json!([{"v": 3}, {"v": 4}]));
    assert_eq!(got["page"]["totalCount"], 5);
    let past = run(items, &asc("v"), 7, 3).unwrap();
    assert_eq!(past["page"]["results"], json!([]));
}

#[test]
fn equal_values_keep_their_order_of_arrival() {
    let items: Vec<Value> = [(1, "a"), (0, "b"), (1, "c"), (0, "d")]
        .iter()
        .map(|(v, id)| json!({"v": v, "id": id}))
        .collect();
    let got = run(items, &asc("v"), 0, 4).unwrap();
    let ids: Vec<&str> = got["page"]["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["b", "d", "a", "c"]);
}

#[test]
fn a_mix_of_types_is_refused_only_where_the_tree_compares_it() {
    let mixed = |list: &[Value]| run(list.to_vec(), &asc("v"), 0, 1);
    assert_eq!(mixed(&[json!({"v": 1}), json!({"v": "a"})]), None);
    assert!(mixed(&[json!({"v": {"a": 1}})]).is_some());
    assert!(mixed(&[json!({"v": {"a": 1}}), json!({})]).is_some());
    assert!(mixed(&[json!({"v": {"a": 1}}), json!({"v": null})]).is_some());
    assert_eq!(
        mixed(&[json!({"v": {"a": 1}}), json!({"v": {"a": 2}})]),
        None
    );
}

#[test]
fn null_follows_values_and_absent_follows_null_in_both_directions() {
    let items: Vec<Value> = vec![
        json!({"id": 0}),
        json!({"id": 1, "v": null}),
        json!({"id": 2, "v": 2}),
        json!({"id": 3, "v": 1}),
    ];
    for (direction, order) in [
        (Direction::Asc, [3, 2, 1, 0]),
        (Direction::Desc, [2, 3, 1, 0]),
    ] {
        let got = run(
            items.clone(),
            &Sorting(vec![("v".into(), direction)]),
            0,
            10,
        )
        .unwrap();
        let ids: Vec<i64> = got["page"]["results"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["id"].as_i64().unwrap())
            .collect();
        assert_eq!(ids, order);
    }
}

#[test]
fn strings_compare_by_utf16_units() {
    let items: Vec<Value> = ["\u{10000}", "\u{FFFD}", "a"]
        .iter()
        .map(|v| json!({"v": v}))
        .collect();
    let got = run(items, &asc("v"), 0, 3).unwrap();
    assert_eq!(
        got["page"]["results"],
        json!([{"v": "a"}, {"v": "\u{10000}"}, {"v": "\u{FFFD}"}])
    );
}

#[test]
fn a_semver_that_is_not_valid_is_refused() {
    let items = |a: &str, b: &str| vec![json!({"v": {"$semver": a}}), json!({"v": {"$semver": b}})];
    assert!(run(items("1.2.3", "1.10.0"), &asc("v"), 0, 2).is_some());
    assert_eq!(run(items("1.2.3", "nope"), &asc("v"), 0, 2), None);
}
