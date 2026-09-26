use super::*;

#[test]
fn bigint_reads_text_as_javascript_does() {
    let ok = |text: &str| parse_bigint(text);
    assert_eq!(ok(""), Ok(0));
    assert_eq!(ok(" \t\n\u{FEFF}\u{3000}"), Ok(0));
    assert_eq!(ok(" 12\n"), Ok(12));
    assert_eq!(ok("-0"), Ok(0));
    assert_eq!(ok("+5"), Ok(5));
    assert_eq!(ok("-5"), Ok(-5));
    assert_eq!(ok("0x10"), Ok(16));
    assert_eq!(ok("0XfF"), Ok(255));
    assert_eq!(ok("0o17"), Ok(15));
    assert_eq!(ok("0b101"), Ok(5));
    assert_eq!(ok("007"), Ok(7));
    assert_eq!(
        ok("-170141183460469231731687303715884105728"),
        Ok(i128::MIN)
    );
    assert_eq!(ok("170141183460469231731687303715884105727"), Ok(i128::MAX));
    for bad in [
        "1.5", "1e3", "-0x10", "+0x1", "0x", "0b2", "--1", "+", "-", "1 2", "1_000", "\u{85}1",
        "Infinity", "0n",
    ] {
        assert_eq!(ok(bad), Err(BigIntError::Syntax), "{bad:?}");
    }
    assert_eq!(
        ok("170141183460469231731687303715884105728"),
        Err(BigIntError::TooWide)
    );
    assert_eq!(
        ok(&format!("0x{}", "f".repeat(40))),
        Err(BigIntError::TooWide)
    );
    // A syntax error later in the text wins over the width.
    assert_eq!(
        ok(&format!("{}x", "9".repeat(60))),
        Err(BigIntError::Syntax)
    );
}

#[test]
fn utf16_lengths_order_and_cuts() {
    assert_eq!(utf16_len("a\u{e9}\u{1F600}"), 4);
    assert_eq!(utf16_cmp("\u{FF61}", "\u{1F600}"), Ordering::Greater);
    assert_eq!(utf16_cmp("10", "9"), Ordering::Less);
    assert_eq!(utf16_split("abc", 2), ("ab".into(), "c".into()));
    assert_eq!(utf16_split("abc", 5), ("abc".into(), String::new()));
    assert_eq!(
        utf16_split("a\u{1F600}b", 2),
        ("a\u{FFFD}".into(), "\u{FFFD}b".into())
    );
    assert_eq!(
        utf16_split("a\u{1F600}b", 3),
        ("a\u{1F600}".into(), "b".into())
    );
    assert_eq!(utf8_lossy(&[0x61, 0xff, 0xfe]), "a\u{FFFD}\u{FFFD}");
}

#[test]
fn json_strings_are_escaped_as_json_stringify_does() {
    let mut out = String::new();
    json_string(
        &mut out,
        "a\"\\\u{08}\t\n\u{0B}\u{0C}\r\u{1F}\u{7F}\u{2028}\u{e9}",
    );
    assert_eq!(
        out,
        "\"a\\\"\\\\\\b\\t\\n\\u000b\\f\\r\\u001f\u{7F}\u{2028}\u{e9}\""
    );
}
