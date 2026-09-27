use super::normalise;

/// Raw paths and `pathname + search` of `new URL("http://127.0.0.1/" + raw)` as Node 18.20.8
/// gives them.
const CASES: &[(&str, &str)] = &[
    // The examples the frame parser was checked with.
    ("a/../p2p.peer.getStatus", "/p2p.peer.getStatus"),
    ("p2p.peer.get\tStatus  ", "/p2p.peer.getStatus"),
    ("./p2p.peer.getStatus#x", "/p2p.peer.getStatus"),
    ("p2p\\peer?q=a b'", "/p2p/peer?q=a%20b%27"),
    ("%2e%2E/x", "/x"),
    ("\u{FFFD}\u{FFFD}", "/%EF%BF%BD%EF%BF%BD"),
    ("p2p.peer.getStatus", "/p2p.peer.getStatus"),
    // Trailing C0 controls and spaces; tabs, LF and CR anywhere.
    (" ", "/"),
    ("\u{0}", "/"),
    ("a? ", "/a"),
    ("foo\tbar", "/foobar"),
    ("a\n/b\r", "/a/b"),
    ("a/b\u{1} \u{2}", "/a/b"),
    ("\u{0}a", "/%00a"),
    ("foo\t\u{91}%91", "/foo%C2%91%91"),
    // Query and fragment.
    ("a?", "/a"),
    ("a?#", "/a"),
    ("a?b?c", "/a?b?c"),
    ("a#b?c", "/a"),
    ("a?b#c#d", "/a?b"),
    ("?", "/"),
    ("#", "/"),
    ("?x", "/?x"),
    ("q?\u{1}x", "/q?%01x"),
    ("\u{E9}?\u{E9}", "/%C3%A9?%C3%A9"),
    ("sum?q=a\"b<c>d'e`f{g}h", "/sum?q=a%22b%3Cc%3Ed%27e`f{g}h"),
    // Dot segments, from the URL standard's path tests.
    ("foo/bar/..", "/foo/"),
    ("foo/bar/../", "/foo/"),
    ("foo/./bar", "/foo/bar"),
    ("foo/.%2e/bar", "/bar"),
    ("foo/%2E%2e", "/"),
    ("foo/..bar", "/foo/..bar"),
    ("foo/%2e.bar", "/foo/%2e.bar"),
    ("foo/bar/../ton/../../a", "/a"),
    ("foo/../../..", "/"),
    ("foo/../../../ton", "/ton"),
    ("foo/%2e", "/foo/"),
    ("foo/%2e%2", "/foo/%2e%2"),
    ("foo/%2e./%2e%2e/.%2e/%2e.bar", "/%2e.bar"),
    ("///../..", "//"),
    ("foo/bar//../..", "/foo/"),
    ("foo/bar//..", "/foo/bar/"),
    ("..", "/"),
    (".", "/"),
    ("./", "/"),
    ("../", "/"),
    ("a/.", "/a/"),
    ("a/./", "/a/"),
    ("%2E", "/"),
    ("%2e%2E%2f", "/%2e%2E%2f"),
    (".%2E/b", "/b"),
    ("a/b/%2E%2E/c", "/a/c"),
    ("a\\..\\b", "/b"),
    // Percent-encoding: `%` is kept, the path set is encoded, non-ASCII as UTF-8.
    ("%20foo", "/%20foo"),
    ("foo%", "/foo%"),
    ("foo%2", "/foo%2"),
    ("foo%2zbar", "/foo%2zbar"),
    ("foo%2\u{C2}\u{A9}zbar", "/foo%2%C3%82%C2%A9zbar"),
    ("foo%41%7a", "/foo%41%7a"),
    ("foo%00%51", "/foo%00%51"),
    ("(%28:%3A%29)", "/(%28:%3A%29)"),
    ("foo\\\\bar", "/foo//bar"),
    ("%7Ffp3>*i", "/%7Ffp3%3E*i"),
    ("foo/\u{1F600}", "/foo/%F0%9F%98%80"),
    ("a b", "/a%20b"),
    ("`{}^|[]", "/%60%7B%7D^|[]"),
    ("a\u{7F}", "/a%7F"),
    ("/a", "//a"),
];

#[test]
fn normalises_as_the_url_parser_does() {
    for &(raw, expected) in CASES {
        assert_eq!(normalise(raw), expected, "raw path {raw:?}");
    }
}

/// The ASCII code points that the path and the query percent-encode (tab, LF and CR are removed
/// first; `/`, `\`, `?` and `#` are delimiters), as Node 18.20.8 encodes them.
#[test]
fn percent_encode_sets() {
    let c0 = (0u8..0x20).filter(|b| !matches!(b, b'\t' | b'\n' | b'\r'));
    let path: Vec<u8> = c0.clone().chain(*b" \"<>`{}\x7f").collect();
    let query: Vec<u8> = c0.chain(*b" \"'<>\x7f").collect();
    for byte in (0u8..0x80).filter(|b| !b"\t\n\r/\\?#".contains(b)) {
        let c = char::from(byte);
        let escaped = format!("%{byte:02X}");
        let in_path = normalise(&format!("a{c}b"));
        let in_query = normalise(&format!("a?x{c}y"));
        let expect = |set: &[u8], text: &str| {
            if set.contains(&byte) {
                escaped.clone()
            } else {
                text.to_owned()
            }
        };
        assert_eq!(
            in_path,
            format!("/a{}b", expect(&path, &c.to_string())),
            "byte {byte:#04x} in a path"
        );
        assert_eq!(
            in_query,
            format!("/a?x{}y", expect(&query, &c.to_string())),
            "byte {byte:#04x} in a query"
        );
    }
}
