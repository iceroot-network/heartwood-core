//! The standard criteria, one rule at a time: what matches, what does not, and what the reference
//! throws on (a 500 here).

use heartwood_api::search::criteria::matches;
use heartwood_api::search::{Criteria, big_number, semver};
use serde_json::{Value, json};

fn criteria(value: Value) -> Criteria {
    let Value::Object(map) = value else {
        panic!("criteria are an object")
    };
    Criteria(map)
}

/// `Some(matched)`, or `None` where the reference throws.
fn run(resource: &Value, wanted: Value) -> Option<bool> {
    matches(resource, &[criteria(wanted)]).ok()
}

fn resource() -> Value {
    json!({
        "flag": true,
        "name": "genesis_7",
        "count": 12,
        "balance": big_number("1000000000000000000000"),
        "version": semver("1.2.3"),
        "attributes": {"delegate": {"rank": 5, "username": "alice"}, "extra": {"a": 1, "b": "x"}},
        "gone": null,
        "list": [1, 2],
    })
}

#[test]
fn no_criteria_match_and_every_criteria_object_must() {
    let r = resource();
    assert!(matches(&r, &[]).unwrap());
    assert!(matches(&r, &[criteria(json!({}))]).unwrap());
    let yes = criteria(json!({"count": 12}));
    let no = criteria(json!({"count": 13}));
    assert!(matches(&r, &[yes.clone(), yes.clone()]).unwrap());
    assert!(!matches(&r, &[yes, no]).unwrap());
}

#[test]
fn a_missing_or_null_value_never_matches_and_is_not_examined() {
    let r = resource();
    assert_eq!(run(&r, json!({"absent": "x"})), Some(false));
    assert_eq!(run(&r, json!({"absent": {"from": 1}})), Some(false));
    assert_eq!(run(&r, json!({"gone": 1})), Some(false));
    // Even a criterion the reference would throw on, for a value that is not there.
    assert_eq!(run(&r, json!({"absent": {}})), Some(false));
    assert_eq!(run(&r, json!({"gone": null})), Some(false));
}

#[test]
fn an_array_of_criteria_is_an_alternative() {
    let r = resource();
    assert_eq!(run(&r, json!({"count": [1, 12]})), Some(true));
    assert_eq!(run(&r, json!({"count": [1, 2]})), Some(false));
    assert_eq!(run(&r, json!({"count": []})), Some(false));
    // The first that matches ends the search, so a later invalid one is not seen.
    assert_eq!(run(&r, json!({"count": [12, "x"]})), Some(true));
    assert_eq!(run(&r, json!({"count": ["x", 12]})), None);
}

#[test]
fn a_boolean_takes_true_and_false_as_a_boolean_or_a_string() {
    let r = resource();
    for (criterion, expected) in [
        (json!(true), true),
        (json!("true"), true),
        (json!(false), false),
        (json!("false"), false),
    ] {
        assert_eq!(run(&r, json!({"flag": criterion})), Some(expected));
    }
    for other in [
        json!("yes"),
        json!(1),
        json!("TRUE"),
        json!({}),
        json!(null),
    ] {
        assert_eq!(run(&r, json!({"flag": other})), None);
    }
}

#[test]
fn a_string_is_equal_or_a_percent_pattern() {
    let r = resource();
    assert_eq!(run(&r, json!({"name": "genesis_7"})), Some(true));
    assert_eq!(run(&r, json!({"name": "genesis_"})), Some(false));
    assert_eq!(run(&r, json!({"name": "%nesis%"})), Some(true));
    assert_eq!(run(&r, json!({"name": "%"})), Some(true));
    // Unanchored, in order.
    assert_eq!(run(&r, json!({"name": "gen%_7"})), Some(true));
    assert_eq!(run(&r, json!({"name": "_7%gen"})), Some(false));
    assert_eq!(run(&r, json!({"name": "s%s"})), Some(true));
    assert_eq!(run(&r, json!({"name": "%z%"})), Some(false));
    assert_eq!(run(&r, json!({"name": "ge%ge"})), Some(false));
    // A non-string criterion on a string value throws.
    for other in [json!(7), json!(true), json!({"from": "a"}), json!(null)] {
        assert_eq!(run(&r, json!({"name": other})), None);
    }
}

#[test]
fn a_number_is_equal_or_within_a_range() {
    let r = resource();
    assert_eq!(run(&r, json!({"count": 12})), Some(true));
    assert_eq!(run(&r, json!({"count": "12"})), Some(true));
    assert_eq!(run(&r, json!({"count": " 12 "})), Some(true));
    assert_eq!(run(&r, json!({"count": "0xc"})), Some(true));
    assert_eq!(run(&r, json!({"count": ""})), Some(false));
    assert_eq!(run(&r, json!({"count": {"from": 12}})), Some(true));
    assert_eq!(run(&r, json!({"count": {"from": 13}})), Some(false));
    assert_eq!(run(&r, json!({"count": {"to": 12}})), Some(true));
    assert_eq!(run(&r, json!({"count": {"to": 11}})), Some(false));
    assert_eq!(
        run(&r, json!({"count": {"from": "1", "to": "20"}})),
        Some(true)
    );
    assert_eq!(
        run(&r, json!({"count": {"from": 20, "to": 1}})),
        Some(false)
    );
    // Both bounds are read before either is compared.
    assert_eq!(run(&r, json!({"count": {"from": 20, "to": "x"}})), None);
    assert_eq!(run(&r, json!({"count": {"from": "x"}})), None);
    assert_eq!(run(&r, json!({"count": "x"})), None);
    assert_eq!(run(&r, json!({"count": {}})), None);
    assert_eq!(run(&r, json!({"count": {"other": 1}})), None);
    assert_eq!(run(&r, json!({"count": true})), None);
    assert_eq!(run(&r, json!({"count": null})), None);
}

#[test]
fn a_big_number_is_equal_or_within_a_range() {
    let r = resource();
    let big = "1000000000000000000000";
    assert_eq!(run(&r, json!({"balance": big})), Some(true));
    assert_eq!(
        run(&r, json!({"balance": "1000000000000000000001"})),
        Some(false)
    );
    assert_eq!(run(&r, json!({"balance": {"from": big}})), Some(true));
    assert_eq!(
        run(&r, json!({"balance": {"from": "1000000000000000000001"}})),
        Some(false)
    );
    assert_eq!(
        run(&r, json!({"balance": {"to": "999999999999999999999"}})),
        Some(false)
    );
    assert_eq!(
        run(&r, json!({"balance": {"from": 1, "to": big}})),
        Some(true)
    );
    assert_eq!(
        run(&r, json!({"balance": {"from": "0x10", "to": ""}})),
        Some(false)
    );
    // `from` failing ends the test before `to` is read; `from` holding reads it.
    assert_eq!(
        run(
            &r,
            json!({"balance": {"from": "2000000000000000000000", "to": "x"}})
        ),
        Some(false)
    );
    assert_eq!(run(&r, json!({"balance": {"from": 1, "to": "x"}})), None);
    assert_eq!(run(&r, json!({"balance": {"from": 1.5}})), None);
    assert_eq!(run(&r, json!({"balance": "1e3"})), None);
    assert_eq!(run(&r, json!({"balance": {}})), None);
    assert_eq!(run(&r, json!({"balance": {"other": 1}})), None);
    assert_eq!(run(&r, json!({"balance": [[1]]})), None);
    assert_eq!(run(&r, json!({"balance": null})), None);
    assert_eq!(run(&r, json!({"balance": {"from": null}})), None);
}

#[test]
fn a_semver_is_equal_or_within_a_range() {
    let r = resource();
    assert_eq!(run(&r, json!({"version": "1.2.3"})), Some(true));
    assert_eq!(run(&r, json!({"version": "v1.2.3"})), Some(true));
    assert_eq!(run(&r, json!({"version": " =1.2.3 "})), Some(true));
    assert_eq!(run(&r, json!({"version": "1.2.4"})), Some(false));
    assert_eq!(run(&r, json!({"version": {"from": "1.0.0"}})), Some(true));
    assert_eq!(run(&r, json!({"version": {"from": "1.2.4"}})), Some(false));
    assert_eq!(
        run(&r, json!({"version": {"from": "1.0.0", "to": "1.2.3"}})),
        Some(true)
    );
    assert_eq!(run(&r, json!({"version": {"to": "1.2.2"}})), Some(false));
    assert_eq!(
        run(
            &r,
            json!({"version": {"from": "1.2.3-beta", "to": "2.0.0"}})
        ),
        Some(true)
    );
    assert_eq!(run(&r, json!({"version": "nope"})), None);
    assert_eq!(run(&r, json!({"version": {"from": "nope"}})), None);
    assert_eq!(
        run(&r, json!({"version": {"from": "1.0.0", "to": "nope"}})),
        None
    );
    assert_eq!(run(&r, json!({"version": {}})), None);
    assert_eq!(run(&r, json!({"version": 1})), None);
    assert_eq!(run(&r, json!({"version": null})), None);
    // An own value that is not a version throws.
    let odd = json!({"version": semver("not-a-version")});
    assert_eq!(run(&odd, json!({"version": "1.0.0"})), None);
}

#[test]
fn nested_objects_test_each_named_property() {
    let r = resource();
    assert_eq!(
        run(&r, json!({"attributes": {"delegate": {"rank": 5}}})),
        Some(true)
    );
    assert_eq!(
        run(&r, json!({"attributes": {"delegate": {"rank": 6}}})),
        Some(false)
    );
    assert_eq!(
        run(
            &r,
            json!({"attributes": {"delegate": {"rank": 5, "username": "bob"}}})
        ),
        Some(false)
    );
    assert_eq!(
        run(&r, json!({"attributes": {"delegate": {"absent": 5}}})),
        Some(false)
    );
    assert_eq!(
        run(&r, json!({"attributes": {"nothing": {"rank": 5}}})),
        Some(false)
    );
    assert_eq!(run(&r, json!({"attributes": {}})), Some(true));
    // A number or a boolean has no properties; an empty string neither.
    assert_eq!(run(&r, json!({"attributes": 1})), Some(true));
    assert_eq!(run(&r, json!({"attributes": true})), Some(true));
    assert_eq!(run(&r, json!({"attributes": ""})), Some(true));
    assert_eq!(run(&r, json!({"attributes": null})), None);
    // A string or an array names its characters or indexes as properties.
    assert_eq!(run(&r, json!({"attributes": "ab"})), Some(false));
    // An empty array is an alternative of nothing.
    assert_eq!(run(&r, json!({"attributes": []})), Some(false));
    // An array value is not supported.
    assert_eq!(run(&r, json!({"list": 1})), None);
    assert_eq!(
        run(
            &r,
            json!({"attributes": {"delegate": {"rank": {"from": 1, "to": 9}}}})
        ),
        Some(true)
    );
}

#[test]
fn the_star_key_tests_every_value_of_an_object() {
    let r = resource();
    assert_eq!(
        run(&r, json!({"attributes": {"*": {"rank": 5}}})),
        Some(true)
    );
    assert_eq!(
        run(&r, json!({"attributes": {"*": {"rank": 6}}})),
        Some(false)
    );
    // The values are tried in order and the first that matches ends the search: `1` first.
    assert_eq!(
        run(&r, json!({"attributes": {"extra": {"*": 1}}})),
        Some(true)
    );
    assert_eq!(run(&r, json!({"attributes": {"extra": {"*": "x"}}})), None);
    assert_eq!(run(&r, json!({"attributes": {"extra": {"*": 2}}})), None);
    // Only alone: with another key it is an ordinary property, which is absent.
    assert_eq!(
        run(&r, json!({"attributes": {"extra": {"*": "x", "a": 1}}})),
        Some(false)
    );
}

#[test]
fn a_votes_wildcard_tests_the_names_voted_for() {
    let r = json!({"votingFor": {"alice": {"percent": 1}}, "address": "A"});
    assert_eq!(run(&r, json!({"votingFor": [{"alice": "*"}]})), Some(true));
    assert_eq!(run(&r, json!({"votingFor": [{"bob": "*"}]})), Some(false));
    // Without the wildcard it is a nested criterion.
    assert_eq!(
        run(&r, json!({"votingFor": {"alice": {"percent": 1}}})),
        Some(true)
    );
}

#[test]
fn an_inherited_function_is_not_a_value() {
    let r = resource();
    for name in [
        "toString",
        "valueOf",
        "hasOwnProperty",
        "constructor",
        "__lookupGetter__",
    ] {
        assert_eq!(run(&r, json!({"attributes": {name: 1}})), None, "{name}");
    }
    // Own or unknown names are ordinary.
    assert_eq!(run(&r, json!({"attributes": {"toStrin": 1}})), Some(false));
    let own = json!({"attributes": {"toString": "x"}});
    assert_eq!(
        run(&own, json!({"attributes": {"toString": "x"}})),
        Some(true)
    );
}

#[test]
fn the_criteria_are_read_in_key_order() {
    let r = resource();
    // A failing test before a throwing one ends the search.
    assert_eq!(run(&r, json!({"count": 1, "name": 5})), Some(false));
    assert_eq!(run(&r, json!({"name": 5, "count": 1})), None);
}

#[test]
fn a_resource_that_is_not_an_object_matches_nothing() {
    assert!(!matches(&json!(1), &[criteria(json!({}))]).unwrap());
}
