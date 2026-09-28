//! The escape of a line's text: exactly the characters the logger documents, in the form JSON
//! uses, and nothing else, so backslashes are kept and compact JSON in a message stays JSON.

use std::borrow::Cow;

use heartwood_logger::format::escape;
use serde_json::{Value, json};

/// Unicode's bidirectional controls (the `Bidi_Control` property).
const BIDI_CONTROLS: [char; 12] = [
    '\u{61c}', '\u{200e}', '\u{200f}', '\u{202a}', '\u{202b}', '\u{202c}', '\u{202d}', '\u{202e}',
    '\u{2066}', '\u{2067}', '\u{2068}', '\u{2069}',
];

/// Whether the logger escapes `c`: a control character (C0, DEL and C1), the line or paragraph
/// separator, or a bidirectional control.
fn documented(c: char) -> bool {
    matches!(c, '\0'..='\u{1f}' | '\u{7f}'..='\u{9f}' | '\u{2028}' | '\u{2029}')
        || BIDI_CONTROLS.contains(&c)
}

#[test]
fn every_documented_character_and_no_other_is_escaped() {
    let mut escaped = 0;
    for c in (0..=0x10_ffff).filter_map(char::from_u32) {
        let text = format!("a{c}b");
        let expected = match c {
            '\n' => "a\\nb".to_owned(),
            '\r' => "a\\rb".to_owned(),
            '\t' => "a\\tb".to_owned(),
            c if documented(c) => format!("a\\u{:04x}b", u32::from(c)),
            c => {
                assert!(
                    matches!(escape(&text), Cow::Borrowed(kept) if kept == text),
                    "U+{:04X} is written as it is",
                    u32::from(c)
                );
                continue;
            }
        };
        assert_eq!(escape(&text), expected, "U+{:04X}", u32::from(c));
        escaped += 1;
    }
    // 65 control characters, the 2 separators and the 12 bidirectional controls.
    assert_eq!(escaped, 79);
}

#[test]
fn backslashes_are_kept_and_compact_json_stays_json() {
    // A backslash and an `n` are written as they are, as the reference writes them: the escape
    // is not meant to be undone, since `\n` in a line can also be a line feed it escaped.
    let plain = r#"C:\dir \n \u202e "quoted" \\"#;
    assert!(matches!(escape(plain), Cow::Borrowed(kept) if kept == plain));
    // Compact JSON holds no raw line break, but may hold DEL, C1 characters, the separators and
    // bidirectional controls in its strings, next to escaped quotes and backslashes.
    let value = json!({
        "memo": "a\\\"b\u{2028}c\u{85}d\u{202e}e\u{7f}f\\\u{2066}g\\",
        "height": 42,
    });
    let compact = serde_json::to_string(&value).expect("JSON");
    assert!(compact.contains(['\u{2028}', '\u{85}', '\u{202e}', '\u{7f}', '\u{2066}']));
    let escaped = escape(&compact);
    assert!(matches!(escaped, Cow::Owned(_)));
    assert!(!escaped.contains(documented), "{escaped:?}");
    assert_eq!(
        serde_json::from_str::<Value>(&escaped).expect("JSON"),
        value
    );
}
