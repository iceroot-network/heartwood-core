//! The milestone keys of the fee floor and the revoke delay: `dynamicFees` and
//! `blocksToRevokeDelegateResignation`, as the devnet states them, merged across milestones, and
//! the value types the loader refuses although the reference would coerce them or fail later.

#[macro_use]
mod common;
mod blocks_common;

use blocks_common::DEVNET;
use heartwood_crypto::Milestones;
use heartwood_crypto::errors::ConfigError;
use heartwood_crypto::managers::DynamicFees;

/// A milestone file: the devnet's height-1 keys with `extra` appended (JSON members as text),
/// then `later` milestones.
fn milestones(extra: &str, later: &[&str]) -> Result<Milestones, ConfigError> {
    let first = format!(
        r#"{{"height":1,"activeDelegates":53,"blockTime":8,"block":{{"version":0}},
        "epoch":"2026-01-01T00:00:00.000Z","reward":0,
        "burn":{{"feeBasisPoints":9000,"txAmount":2000000}},"transfer":{{}}{extra}}}"#
    );
    let text = format!(
        "[{}]",
        [first.as_str()]
            .iter()
            .chain(later)
            .copied()
            .collect::<Vec<_>>()
            .join(",")
    );
    Milestones::load(&text, &DEVNET.network)
}

fn fees(extra: &str) -> Option<DynamicFees> {
    milestones(extra, &[])
        .unwrap_or_else(|error| panic!("{extra}: {error}"))
        .at(1)
        .dynamic_fees()
        .cloned()
}

fn refused(extra: &str) -> ConfigError {
    match milestones(extra, &[]) {
        Ok(_) => panic!("{extra}: accepted"),
        Err(error) => error,
    }
}

#[test]
fn the_devnet_values() {
    let params = DEVNET.milestones.at(2);
    let table = params.dynamic_fees().expect("the devnet has a fee table");
    assert!(table.enabled());
    assert_eq!(table.min_fee(), 6173);
    assert_eq!(table.addon("transfer"), 85);
    assert_eq!(table.addon("delegateRegistration"), 1_214_968);
    assert_eq!(table.addon("delegateResignation"), 0);
    assert_eq!(table.addon("no such type"), 0);
    assert_eq!(params.blocks_to_revoke_delegate_resignation(), Some(106));
}

#[test]
fn absent_keys_keep_the_reference_defaults() {
    let params = milestones("", &[]).unwrap();
    assert_eq!(params.at(1).dynamic_fees(), None);
    assert_eq!(params.at(1).blocks_to_revoke_delegate_resignation(), None);
    assert_eq!(fees(r#","dynamicFees":null"#), None);
    let disabled = fees(r#","dynamicFees":{}"#).unwrap();
    assert!(!disabled.enabled());
    assert_eq!(disabled.min_fee(), 0);
    assert_eq!(disabled.addon("transfer"), 0);
    let no_addon = fees(r#","dynamicFees":{"enabled":true,"minFee":5,"addonBytes":{}}"#).unwrap();
    assert!(no_addon.enabled());
    assert_eq!(no_addon.addon("transfer"), 0);
}

#[test]
fn numbers_are_read_as_javascript_reads_them() {
    let table = fees(
        r#","dynamicFees":{"enabled":true,"minFee":-5,"addonBytes":{"transfer":85.0,"vote":-0,"x":9007199254740991,"y":-9007199254740991,"1":2e0}}"#,
    )
    .unwrap();
    assert_eq!(table.min_fee(), -5);
    assert_eq!(table.addon("transfer"), 85);
    assert_eq!(table.addon("vote"), 0);
    assert_eq!(table.addon("x"), 9_007_199_254_740_991);
    assert_eq!(table.addon("y"), -9_007_199_254_740_991);
    assert_eq!(table.addon("1"), 2);
    let delay = |value: &str| {
        milestones(
            &format!(r#","blocksToRevokeDelegateResignation":{value}"#),
            &[],
        )
        .unwrap()
        .at(1)
        .blocks_to_revoke_delegate_resignation()
    };
    assert_eq!(delay("106"), Some(106));
    assert_eq!(delay("1.06e2"), Some(106));
    assert_eq!(delay("0"), Some(0));
    // A negative delay never holds a revoke back, as a delay of 0.
    assert_eq!(delay("-3"), Some(0));
}

#[test]
fn the_tables_merge_key_by_key() {
    let loaded = milestones(
        r#","dynamicFees":{"enabled":true,"minFee":10,"addonBytes":{"transfer":1,"vote":2}},"blocksToRevokeDelegateResignation":106"#,
        &[
            r#"{"height":5,"dynamicFees":{"addonBytes":{"vote":3}}}"#,
            r#"{"height":9,"dynamicFees":{"enabled":false},"blocksToRevokeDelegateResignation":10}"#,
            r#"{"height":12,"dynamicFees":null}"#,
        ],
    )
    .unwrap();
    let at = |height: u32| loaded.at(height).dynamic_fees().cloned();
    let five = at(5).unwrap();
    assert_eq!(
        (five.min_fee(), five.addon("transfer"), five.addon("vote")),
        (10, 1, 3)
    );
    assert!(five.enabled());
    let nine = at(9).unwrap();
    assert!(!nine.enabled());
    assert_eq!(nine.addon("vote"), 3);
    assert_eq!(at(12), None);
    assert_eq!(
        loaded.at(8).blocks_to_revoke_delegate_resignation(),
        Some(106)
    );
    assert_eq!(
        loaded.at(9).blocks_to_revoke_delegate_resignation(),
        Some(10)
    );
}

#[test]
fn value_types_the_reference_would_coerce_are_refused() {
    let invalid = |extra: &str, key: &str| {
        assert!(
            matches!(refused(extra), ConfigError::Invalid { height: 1, key: ref k, .. } if k == key),
            "{extra}"
        );
    };
    invalid(r#","dynamicFees":true"#, "dynamicFees");
    invalid(r#","dynamicFees":[]"#, "dynamicFees");
    invalid(r#","dynamicFees":"on""#, "dynamicFees");
    for enabled in ["1", "\"true\"", "null", "{}"] {
        invalid(
            &format!(r#","dynamicFees":{{"enabled":{enabled},"minFee":1,"addonBytes":{{}}}}"#),
            "dynamicFees.enabled",
        );
    }
    for min_fee in ["\"6173\"", "1.5", "9007199254740992", "null", "true", "[1]"] {
        invalid(
            &format!(r#","dynamicFees":{{"enabled":true,"minFee":{min_fee},"addonBytes":{{}}}}"#),
            "dynamicFees.minFee",
        );
        // Also where the table is disabled and the value is never read.
        invalid(
            &format!(r#","dynamicFees":{{"minFee":{min_fee}}}"#),
            "dynamicFees.minFee",
        );
    }
    for addon in ["\"85\"", "85.5", "-9007199254740992", "null", "false", "{}"] {
        invalid(
            &format!(
                r#","dynamicFees":{{"enabled":true,"minFee":1,"addonBytes":{{"transfer":{addon}}}}}"#
            ),
            "dynamicFees.addonBytes.transfer",
        );
    }
    for addons in ["null", "[]", "85"] {
        invalid(
            &format!(r#","dynamicFees":{{"enabled":true,"minFee":1,"addonBytes":{addons}}}"#),
            "dynamicFees.addonBytes",
        );
    }
    for delay in ["\"106\"", "106.5", "9007199254740993", "null", "true"] {
        invalid(
            &format!(r#","blocksToRevokeDelegateResignation":{delay}"#),
            "blocksToRevokeDelegateResignation",
        );
    }
    // A later milestone is checked after the merge, and the refusal names its height.
    assert!(matches!(
        milestones(
            "",
            &[r#"{"height":7,"blocksToRevokeDelegateResignation":"1"}"#]
        ),
        Err(ConfigError::Invalid { height: 7, .. })
    ));
}

#[test]
fn an_enabled_table_states_what_every_fee_check_reads() {
    for (extra, key) in [
        (
            r#","dynamicFees":{"enabled":true,"addonBytes":{}}"#,
            "dynamicFees.minFee",
        ),
        (
            r#","dynamicFees":{"enabled":true,"minFee":1}"#,
            "dynamicFees.addonBytes",
        ),
    ] {
        assert_eq!(
            refused(extra),
            ConfigError::Missing {
                height: 1,
                key: key.to_owned()
            },
            "{extra}"
        );
    }
    // The same keys may be missing while the table is disabled.
    assert!(fees(r#","dynamicFees":{"enabled":false}"#).is_some());
}
