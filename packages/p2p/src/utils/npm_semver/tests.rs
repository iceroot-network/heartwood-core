use std::cmp::Ordering;

use super::{compare, satisfies, valid};

#[test]
fn versions_are_read_strictly() {
    for (text, normal) in [
        ("4.3.1", Some("4.3.1")),
        (" v4.3.1-next.0+b.7 ", Some("4.3.1-next.0")),
        ("4.3.1-heartwood.0.1.0", Some("4.3.1-heartwood.0.1.0")),
        ("4.3", None),
        ("=4.3.1", None),
        ("04.3.1", None),
        ("4.3.1-01", None),
        ("4.3.1-", None),
        ("4.3.1+", None),
        ("9007199254740991.0.0", Some("9007199254740991.0.0")),
        ("9007199254740992.0.0", None),
        ("", None),
    ] {
        assert_eq!(valid(text).as_deref(), normal, "{text:?}");
    }
    assert_eq!(
        valid(&format!("1.0.0-{}", "a".repeat(250))).map(|v| v.len()),
        Some(256)
    );
    assert_eq!(valid(&format!("1.0.0-{}", "a".repeat(251))), None);
}

#[test]
fn versions_compare_as_semver_orders_them() {
    for (a, b, ordering) in [
        ("1.0.0", "1.0.0-alpha", Ordering::Greater),
        ("1.0.0-alpha", "1.0.0-alpha.1", Ordering::Less),
        ("1.0.0-alpha.1", "1.0.0-alpha.beta", Ordering::Less),
        ("1.0.0-beta.2", "1.0.0-beta.11", Ordering::Less),
        ("1.0.0-rc.1", "1.0.0-beta.11", Ordering::Greater),
        ("1.0.0+a", "1.0.0+b", Ordering::Equal),
        ("1.10.0", "1.9.0", Ordering::Greater),
    ] {
        assert_eq!(compare(a, b), Some(ordering), "{a} {b}");
    }
    assert_eq!(compare("1.0", "1.0.0"), None);
}

#[test]
fn ranges_desugar_as_semver_desugars_them() {
    for (version, range, include_prerelease, expected) in [
        ("4.3.1", ">=4.3.0", false, true),
        ("4.2.9", ">=4.3.0", false, false),
        ("4.3.1-heartwood.0.1.0", ">=4.3.0", true, true),
        ("4.3.1-heartwood.0.1.0", ">=4.3.0", false, false),
        ("4.3.0-next.1", ">=4.3.0-next.0", false, true),
        ("3.5.0", "^3.2", false, true),
        ("4.0.0", "^3.2", false, false),
        ("3.2.0-next.4", "^3.2.0-next.0", false, true),
        ("0.2.5", "^0.2.3", false, true),
        ("0.3.0", "^0.2.3", false, false),
        ("0.0.4", "^0.0.3", false, false),
        ("1.2.9", "~1.2.3", false, true),
        ("1.3.0", "~>1.2.3", false, false),
        ("1.3.0", "~ 1.2", false, false),
        ("2.0.0", "1.x || >=2.5.0", false, false),
        ("2.6.0", "1.x || >=2.5.0", false, true),
        ("1.9.9", "> 1.2 <= 1.9", false, true),
        ("1.10.0", "> 1.2 <= 1.9", false, false),
        ("1.2.9", "> 1.2", false, false),
        ("5.0.0", "*", false, true),
        ("5.0.0-a", "*", false, false),
        ("5.0.0-a", "* || 5.0.0-a", false, false),
        ("5.0.0-a", ">=0.0.0 || 5.0.0-a", false, false),
        ("5.0.0-a", "<0.0.0-0 || 5.0.0-a", false, true),
        ("5.0.0", "", false, true),
        ("5.0.0", "<*", false, false),
        ("1.2.3", "=v1.2.3", false, true),
        ("1.2.3", "==1.2.3", false, false),
        ("1.5.0", "==1", false, true),
        ("1.2.3", "1.2.3 - 2.0.0", false, false),
        ("1.2.3", ">=1.2.3*", false, false),
        ("1.0.0", "^1.x.3", true, true),
        ("1.2.3", "^ = 1.2.x = 1.2.x", false, true),
        ("9007199254740991.0.0", "^9007199254740991", false, false),
    ] {
        assert_eq!(
            satisfies(version, range, include_prerelease),
            expected,
            "{version} {range:?} {include_prerelease}"
        );
    }
}
