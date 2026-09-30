//! The pagination service: `api.searchPage` (the standard criteria and `getPage` over items) and
//! `js.rbTrace` (the comparisons of the red-black tree over a script of inserts and removes).

use std::cmp::Ordering;

use heartwood_api::pagination::Pagination;
use heartwood_api::search::rb_tree::{RbTree, top};
use heartwood_api::search::{Criteria, Direction, Sorting, criteria, page};
use serde_json::{Value, json};

use super::{Env, Outcome, Run, failed};
use crate::subject::{Case, Verdict};

/// The operations of this module.
pub(super) const OPS: &[(&str, Run)] = &[("api.searchPage", search_page), ("js.rbTrace", rb_trace)];

/// `input[key]` as a list.
fn list<'a>(input: &'a Value, key: &str) -> Outcome<&'a Vec<Value>> {
    (input[key].as_array()).ok_or_else(|| failed(format!("input.{key} is not a list")))
}

/// The number at `key` of `input`.
fn count(input: &Value, key: &str) -> Outcome<u64> {
    (input[key].as_u64()).ok_or_else(|| failed(format!("input.{key} is not a count")))
}

/// The items that the criteria pass, on the page: 200 with the page, or 500 where the reference
/// throws.
fn search_page(_env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let input = case.input;
    let sorting = list(input, "sorting")?.iter().map(|entry| {
        let property = entry["property"].as_str().unwrap_or_default().to_owned();
        let desc = entry["direction"] == "desc";
        (
            property,
            if desc {
                Direction::Desc
            } else {
                Direction::Asc
            },
        )
    });
    let criterias: Vec<Criteria> = (input["criteria"].as_array().into_iter().flatten())
        .filter_map(|entry| Some(Criteria(entry.as_object()?.clone())))
        .collect();
    let pagination = Pagination {
        offset: count(&input["pagination"], "offset")?,
        limit: count(&input["pagination"], "limit")?,
    };
    let items = list(input, "items")?;
    let outcome = (|| {
        let mut matching = Vec::new();
        for item in items {
            if criterias.is_empty() || criteria::matches(item, &criterias)? {
                matching.push(item.clone());
            }
        }
        page(
            pagination,
            &Sorting(sorting.collect()),
            matching.into_iter(),
        )
    })();
    Ok(match outcome {
        Ok(page) => Verdict::Output(json!({"page": {
            "results": page.results,
            "totalCount": page.total_count,
            "meta": {"totalCountIsEstimate": page.total_count_is_estimate},
        }})),
        Err(error) => Verdict::Rejected {
            class: None,
            detail: format!("{error:?}"),
        },
    })
}

/// The pairs of indices that the tree compares, over a script of inserts and removes or over
/// `getTop` of the values, and the keys at the end.
fn rb_trace(_env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let input = case.input;
    let values: Vec<f64> = (list(input, "values")?.iter())
        .map(|value| {
            value
                .as_f64()
                .ok_or_else(|| failed("a value is not a number"))
        })
        .collect::<Outcome<_>>()?;
    let order = |a: usize, b: usize| match (values.get(a), values.get(b)) {
        (Some(a), Some(b)) => a.partial_cmp(b).unwrap_or(Ordering::Equal),
        _ => Ordering::Equal,
    };
    let mut compared = Vec::new();
    let keys: Vec<usize> = if input["script"].is_array() {
        let mut compare = |a: &usize, b: &usize| {
            compared.push([*a, *b]);
            Ok(order(*a, *b))
        };
        let mut tree = RbTree::default();
        for step in list(input, "script")? {
            let key = (step[1].as_u64().and_then(|key| usize::try_from(key).ok()))
                .ok_or_else(|| failed("a step has no key"))?;
            match step[0].as_str() {
                Some("insert") => tree.insert(key, &mut compare),
                Some("remove") => tree.remove(&key, &mut compare),
                _ => return Err(failed("a step is neither insert nor remove")),
            }
            .map_err(|error| failed(format!("{error:?}")))?;
        }
        tree.into_keys()
    } else {
        let limit = usize::try_from(count(input, "count")?).unwrap_or(usize::MAX);
        let compare = |a: &(usize, usize), b: &(usize, usize)| {
            compared.push([a.0, b.0]);
            Ok(order(a.0, b.0).then(a.0.cmp(&b.0)))
        };
        top(limit, 0..values.len(), compare).map_err(|error| failed(format!("{error:?}")))?
    };
    Ok(Verdict::Output(json!({"compared": compared, "keys": keys})))
}
