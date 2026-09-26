//! The kernel's helpers beyond the vector classes: the option merge (expected values from the
//! reference's `deepmerge` call), exact remote-access lists, glob entries outside the supported
//! subset, `.env` text written from any map, and edges of the round and slot rules.

mod common;

use std::path::Path;

use common::DEVNET;
use heartwood_kernel::config::dot_env::{self, EnvValue};
use heartwood_kernel::config::merge::deep_merge;
use heartwood_kernel::errors::TimestampError;
use heartwood_kernel::ip;
use heartwood_kernel::paths::resolve_paths;
use heartwood_kernel::timestamp::{UtcDateTime, format_timestamp};
use heartwood_kernel::utils::stringify::stringify;
use heartwood_kernel::{calculate_forging_info, calculate_round, is_new_round};
use indexmap::IndexMap;
use serde_json::Value;

fn parse(text: &str) -> Value {
    serde_json::from_str(text).unwrap()
}

#[test]
fn the_option_merge_is_the_references() {
    for (target, source, merged) in [
        (
            r#"{"a":1,"b":{"c":[1,2],"d":2}}"#,
            r#"{"b":{"c":[3],"e":4},"f":5}"#,
            r#"{"a":1,"b":{"c":[3],"d":2,"e":4},"f":5}"#,
        ),
        (r#"{"a":{"x":1}}"#, r#"{"a":[1]}"#, r#"{"a":[1]}"#),
        (
            r#"{"a":[1]}"#,
            r#"{"a":{"x":1,"constructor":2}}"#,
            r#"{"a":{"x":1}}"#,
        ),
        (
            r#"{"a":1}"#,
            r#"{"a":{"x":1,"constructor":2}}"#,
            r#"{"a":{"x":1,"constructor":2}}"#,
        ),
        (
            r#"{"a":null}"#,
            r#"{"a":{"toString":1,"y":2}}"#,
            r#"{"a":{"toString":1,"y":2}}"#,
        ),
        (
            r#"{"constructor":1,"b":{"toString":2,"z":1}}"#,
            r#"{"constructor":3,"toString":4,"__proto__":{"p":1},"valueOf":5}"#,
            r#"{"constructor":3,"b":{"z":1}}"#,
        ),
        (
            r#"{"b":1,"2":2,"a":3}"#,
            r#"{"1":4,"c":5,"a":{"x":1}}"#,
            r#"{"1":4,"2":2,"b":1,"a":{"x":1},"c":5}"#,
        ),
        (
            r#"{"a":{"b":{"c":1}}}"#,
            r#"{"a":{"b":{"d":2}}}"#,
            r#"{"a":{"b":{"c":1,"d":2}}}"#,
        ),
        (
            r#"{"a":"s"}"#,
            r#"{"a":"t","b":false}"#,
            r#"{"a":"t","b":false}"#,
        ),
        (r#"{"__proto__":{"q":1},"k":1}"#, r#"{"k":2}"#, r#"{"k":2}"#),
    ] {
        let actual = deep_merge(&parse(target), &parse(source));
        assert_eq!(
            serde_json::to_string(&actual).unwrap(),
            merged,
            "{target} + {source}"
        );
    }
}

#[test]
fn remote_access_lists_match_exact_addresses_and_deny_when_empty() {
    let list = |entries: &[&str]| entries.iter().map(|e| (*e).to_owned()).collect::<Vec<_>>();
    assert!(!ip::is_allowed_exactly(&[], "127.0.0.1"));
    let loopback = list(&["127.0.0.1", "::ffff:127.0.0.1", "::1"]);
    for address in ["127.0.0.1", "::1", "[::1]", "::ffff:127.0.0.1"] {
        assert!(ip::is_allowed_exactly(&loopback, address), "{address}");
    }
    for address in ["127.0.0.2", "0:0:0:0:0:0:0:1", "", "*"] {
        assert!(!ip::is_allowed_exactly(&loopback, address), "{address}");
    }
    assert!(!ip::is_allowed_exactly(&list(&["127.0.0.*"]), "127.0.0.1"));
    // The glob lists: empty allows everyone and bans no one.
    assert!(ip::is_whitelisted(&[], "10.0.0.1"));
    assert!(!ip::is_blacklisted(&[], "10.0.0.1"));
}

#[test]
fn glob_entries_outside_the_subset_match_nothing() {
    let one = |entry: &str| vec![entry.to_owned()];
    for entry in [
        "10.0.0.(1|2)",
        "10.0.0.{1,2}",
        "10.0.0.1+",
        "10.0.[0",
        "10.0.0]*",
        "10.0.0.[1!]",
        "a/*",
        "1!*",
    ] {
        assert!(!ip::is_blacklisted(&one(entry), "10.0.0.1"), "{entry}");
    }
    // Backward ranges are refused by the reference's regular expressions too.
    assert!(!ip::is_blacklisted(&one("10.0.0.[9-0]"), "10.0.0.5"));
    assert!(!ip::is_blacklisted(&one("10.0.0.[!1-]"), "10.0.0.5"));
    // Text that no socket address has matches no glob entry, but can equal a plain one.
    assert!(!ip::is_blacklisted(&one("*"), "a/b"));
    assert!(ip::is_blacklisted(&one("a/b"), "a/b"));
    assert!(ip::is_blacklisted(&one("1.2.3.4"), "./1.2.3.4"));
    // A supported entry.
    assert!(ip::is_blacklisted(&one("10.0.0.[1-3]*"), "10.0.0.25"));
    assert!(!ip::is_blacklisted(&one("10.0.0.[!1-3]*"), "10.0.0.25"));
}

#[test]
fn canonical_json_sorts_by_utf16_and_drops_prototype_names() {
    let value = parse(r#"{"b":[{"z":1,"constructor":2}],"a":1e21,"￿":0,"😀":1,"":null}"#);
    assert_eq!(
        stringify(&value),
        "{\"\":null,\"a\":1e+21,\"b\":[{\"z\":1}],\"\u{1f600}\":1,\"\u{ffff}\":0}"
    );
}

#[test]
fn env_text_is_written_in_javascript_key_order() {
    let pairs: IndexMap<String, EnvValue> = [
        ("b".to_owned(), EnvValue::Text("x y".to_owned())),
        ("10".to_owned(), EnvValue::Number(1e21)),
        (String::new(), EnvValue::Bool(true)),
        ("2".to_owned(), EnvValue::Number(f64::INFINITY)),
        ("a".to_owned(), EnvValue::Bool(false)),
    ]
    .into_iter()
    .collect();
    assert_eq!(
        dot_env::stringify(&pairs),
        "2=Infinity\n10=1e+21\nb=\"x y\"\na=false"
    );
    let parsed = dot_env::parse("A=1\nA=2\nB=\"\"\"\nC=\"x\"#");
    assert_eq!(
        parsed.into_iter().collect::<Vec<_>>(),
        [
            ("A".to_owned(), EnvValue::Number(2.0)),
            ("B".to_owned(), EnvValue::Text("\"".to_owned())),
            ("C".to_owned(), EnvValue::Text("\"x\"#".to_owned())),
        ]
    );
}

#[test]
fn round_and_slot_edges() {
    let milestones = &DEVNET.milestones;
    let zero = calculate_round(0, milestones);
    assert_eq!((zero.round, zero.round_height, zero.next_round), (0, 0, 1));
    assert!(!is_new_round(0, milestones));
    let last = calculate_round(u32::MAX, milestones);
    assert_eq!(
        (last.round, last.round_height, last.max_delegates),
        (81_037_119, 4_294_967_255, 53)
    );
    let before = calculate_forging_info(-424, 1, milestones);
    assert_eq!(
        (
            before.current_forger,
            before.next_forger,
            before.block_timestamp
        ),
        (0, 1, -424)
    );
    // The first second of the widest time is the start of its slot.
    let min = calculate_forging_info(i64::MIN, 1, milestones);
    assert!(min.current_forger <= 0 && min.can_forge);
}

#[test]
fn timestamps_outside_a_date_or_with_a_bad_epoch_are_refused() {
    assert_eq!(
        format_timestamp(0, "2026-01-01"),
        Err(TimestampError::Epoch("2026-01-01".to_owned()))
    );
    assert_eq!(
        format_timestamp(i64::MAX, "2026-01-01T00:00:00Z"),
        Err(TimestampError::OutOfRange {
            epoch_stamp: i64::MAX
        })
    );
    assert_eq!(UtcDateTime::from_unix_ms(8_640_000_000_000_001), None);
    let first = UtcDateTime::from_unix_ms(-8_640_000_000_000_000).unwrap();
    assert_eq!(first.to_iso_string(), "-271821-04-20T00:00:00.000Z");
}

#[test]
fn paths_resolve_against_the_working_directory() {
    let env = |name: &str| (name == "CORE_PATH_LOG").then(|| "../logs/./x/".to_owned());
    let paths = resolve_paths(
        "solar",
        "devnet",
        &env,
        Path::new("/srv/node"),
        Path::new("/opt/run"),
    );
    assert_eq!(paths.log, Path::new("/opt/logs/x"));
    if cfg!(target_os = "linux") {
        assert_eq!(paths.temp, Path::new("/tmp/node/solar-core/devnet"));
        assert_eq!(
            paths.data,
            Path::new("/srv/node/.local/share/solar-core/devnet")
        );
    }
}
