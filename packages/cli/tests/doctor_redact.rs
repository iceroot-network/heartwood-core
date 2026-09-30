//! Redaction: address labels and their classes, identity labels, the home directory, the user's
//! and the host's names, hex runs, and the cleaning and capping of untrusted text.

use std::net::IpAddr;
use std::path::{Path, PathBuf};

use heartwood_cli::doctor::redact::{MAX_EXCERPT_CHARS, Redactor, class, clean};
use heartwood_cli::doctor::report::{HostIdentity, Id, IdKind};

fn identity() -> HostIdentity {
    HostIdentity {
        home: Some(PathBuf::from("/users/alice")),
        user: Some("alice".into()),
        host_name: Some("node7.example.org".into()),
    }
}

fn ip(text: &str) -> IpAddr {
    text.parse().unwrap()
}

#[test]
fn every_address_class_is_kept() {
    let classes = [
        ("192.0.2.1", "public"),
        ("203.0.113.9", "public"),
        ("10.1.2.3", "private"),
        ("172.16.0.1", "private"),
        ("192.168.1.1", "private"),
        ("127.0.0.1", "loopback"),
        ("127.240.1.9", "loopback"),
        ("169.254.10.1", "link-local"),
        ("0.0.0.0", "unspecified"),
        ("2001:db8::1", "public"),
        ("fd12:3456::1", "private"),
        ("fe80::1", "link-local"),
        ("::1", "loopback"),
        ("::", "unspecified"),
        ("::ffff:10.0.0.1", "private"),
        ("::ffff:127.0.0.1", "loopback"),
    ];
    for (text, expected) in classes {
        assert_eq!(class(ip(text)), expected, "{text}");
    }
}

#[test]
fn labels_are_numbered_by_first_appearance_and_stable_within_a_report() {
    let identity = identity();
    let mut redactor = Redactor::new(&identity, true);
    assert_eq!(redactor.address(ip("192.0.2.1")), "addr-1 (public)");
    assert_eq!(redactor.address(ip("10.0.0.1")), "addr-2 (private)");
    assert_eq!(redactor.address(ip("192.0.2.1")), "addr-1 (public)");
    assert_eq!(redactor.address(ip("::ffff:192.0.2.1")), "addr-1 (public)");
    assert_eq!(redactor.address(ip("2001:db8::1")), "addr-3 (public)");
    assert_eq!(redactor.address(ip("2001:db8:0:0::1")), "addr-3 (public)");
    assert_eq!(
        redactor.untrusted("from 10.0.0.1 and 198.51.100.4"),
        "from addr-2 (private) and addr-4 (public)"
    );
    assert_eq!(redactor.address(ip("198.51.100.4")), "addr-4 (public)");
    let mut shown = Redactor::new(&identity, false);
    assert_eq!(shown.address(ip("192.0.2.1")), "192.0.2.1");
    assert_eq!(shown.untrusted("from 192.0.2.1"), "from 192.0.2.1");
}

#[test]
fn untrusted_text_has_ipv4_and_every_ipv6_form_replaced() {
    let identity = identity();
    let mut redactor = Redactor::new(&identity, true);
    let cases = [
        (
            "peer 192.0.2.1:4001 refused",
            "peer addr-1 (public):4001 refused",
        ),
        ("at [2001:db8::1]:4001.", "at [addr-2 (public)]:4001."),
        (
            "mapped ::ffff:192.0.2.1 is v4",
            "mapped addr-1 (public) is v4",
        ),
        ("zoned fe80::1%eth0 here", "zoned addr-3 (link-local) here"),
        (
            "zoned fe80::1%25eth0.100, end",
            "zoned addr-3 (link-local), end",
        ),
        (
            "full 2001:DB8:0:0:0:0:0:1 again",
            "full addr-2 (public) again",
        ),
        ("loopback ::1.", "loopback addr-4 (loopback)."),
        (
            "list 10.0.0.1,10.0.0.2;",
            "list addr-5 (private),addr-6 (private);",
        ),
        ("url http://203.0.113.5/x", "url http://addr-7 (public)/x"),
        ("glued x192.0.2.1", "glued xaddr-1 (public)"),
        ("prefix 10.0.0.0/24.", "prefix addr-8 (private)/24."),
        ("split 192.0\u{200b}.2.1", "split addr-1 (public)"),
        ("from:2001:db8::1 now", "from:addr-2 (public) now"),
        ("Error:fe80::1", "Error:addr-3 (link-local)"),
        ("glued x2001:db8:0:1::9y", "glued xaddr-9 (public)y"),
    ];
    for (text, expected) in cases {
        assert_eq!(redactor.untrusted(text), expected, "{text}");
    }
}

#[test]
fn text_that_only_looks_like_an_address_is_kept() {
    let identity = identity();
    let mut redactor = Redactor::new(&identity, true);
    let kept = [
        "std::io::Error: heartwood_cli::doctor::run failed",
        "at 14:03:11.000 on 2026-10-02",
        "version 4.3.1-heartwood.0.3.0 and 1.2.3",
        "mac aa:bb:cc:dd:ee:ff",
        "Error:: nothing",
        "ratio 3:2 and 10:20:30",
    ];
    for text in kept {
        assert_eq!(redactor.untrusted(text), text);
    }
}

#[test]
fn hex_runs_of_64_characters_or_more_are_replaced() {
    let identity = identity();
    let mut redactor = Redactor::new(&identity, true);
    let run = |n: usize| "a1".repeat(n / 2) + &"f".repeat(n % 2);
    assert_eq!(redactor.untrusted(&run(63)), run(63));
    let text = format!("key {} and 0x{}", run(64), run(66).to_uppercase());
    assert_eq!(
        redactor.untrusted(&text),
        "key [hex, 64 characters] and 0x[hex, 66 characters]"
    );
    let text = format!("{}{}", run(64), run(64));
    assert_eq!(redactor.untrusted(&text), "[hex, 128 characters]");
}

#[test]
fn control_characters_escape_sequences_and_bidirectional_marks_are_removed() {
    let cases = [
        ("a\nb\tc\rd\u{0}e\u{7f}f", "a b c d e f"),
        ("\u{1b}[1;31mred\u{1b}[0m text", "red text"),
        ("\u{1b}]0;window title\u{7}after", "after"),
        ("\u{1b}]8;;http://x\u{1b}\\link", "link"),
        ("\u{1b}Pdevice\u{1b}\\x", "x"),
        ("\u{9b}2Jcleared", "cleared"),
        ("\u{1b}7saved", "saved"),
        (
            "\u{202e}evil\u{202c} \u{2066}isolate\u{2069}",
            "evil isolate",
        ),
        ("\u{200e}mark\u{200f}\u{61c}", "mark"),
        ("zero\u{200b}width\u{feff}\u{2060}", "zerowidth"),
        // Format characters a terminal does not show but a reader of the text receives.
        ("tag\u{e0001}\u{e0069}\u{e0067}\u{e006e}\u{e007f}s", "tags"),
        ("soft\u{ad}hyphen\u{34f}\u{180e}", "softhyphen"),
        ("selector\u{fe0f}\u{e0100}\u{180b}", "selector"),
        ("anchor\u{fff9}\u{fffa}\u{fffb}", "anchor"),
        ("arabic\u{600}\u{6dd}\u{70f}\u{890}\u{8e2}", "arabic"),
        ("other\u{110bd}\u{13430}\u{1bca0}\u{1d173}\u{206a}", "other"),
        ("filler\u{115f}\u{1160}\u{3164}\u{ffa0}end", "filler    end"),
        (
            "line\u{2028}break\u{2029}para\u{85}next",
            "line break para next",
        ),
        (
            "a forged\n[2026-10-02 14:03:11.000] INFO: line",
            "a forged [2026-10-02 14:03:11.000] INFO: line",
        ),
    ];
    let identity = HostIdentity::default();
    let mut redactor = Redactor::new(&identity, true);
    for (text, expected) in cases {
        assert_eq!(clean(text), expected, "{text:?}");
        let out = redactor.untrusted(text);
        assert!(!out.chars().any(char::is_control), "{out:?}");
    }
    // An invisible character inside a hex run or an address hides neither.
    let split = format!("{}\u{e0041}{}", "ab".repeat(20), "cd".repeat(20));
    assert_eq!(redactor.untrusted(&split), "[hex, 80 characters]");
    let split = format!("{}\u{ad}{}", "ab".repeat(20), "cd".repeat(20));
    assert_eq!(redactor.untrusted(&split), "[hex, 80 characters]");
    assert_eq!(redactor.untrusted("192.0\u{ad}.2.1"), "addr-1 (public)");
    assert_eq!(
        redactor.untrusted("2001:db8:\u{e0041}:1"),
        "addr-2 (public)"
    );
}

#[test]
fn the_home_directory_the_user_and_the_host_are_replaced() {
    let identity = identity();
    let redactor = Redactor::new(&identity, true);
    let paths = [
        (
            "/users/alice/.config/solar-core/testnet/delegates.json",
            "~/.config/solar-core/testnet/delegates.json",
        ),
        ("/users/alice", "~"),
        ("/srv/alice/data", "/srv/user/data"),
        ("/srv/Node7/data", "/srv/host/data"),
        ("/srv/node7.example.org/data", "/srv/host/data"),
        ("/users/alicex/data", "/users/alicex/data"),
        ("/srv/alice-data", "/srv/alice-data"),
        ("relative/alice", "relative/user"),
        ("/srv/line\nbreak", "/srv/line break"),
    ];
    for (path, expected) in paths {
        assert_eq!(redactor.path(Path::new(path)), expected, "{path}");
    }
    let mut redactor = Redactor::new(&identity, true);
    let texts = [
        ("cannot open /users/alice/x/y", "cannot open ~/x/y"),
        ("on node7.example.org as Alice", "on host as user"),
        ("host NODE7 and node7.local", "host host and host.local"),
        (
            "alicexyz node77 /users/alice2",
            "alicexyz node77 /users/alice2",
        ),
    ];
    for (text, expected) in texts {
        assert_eq!(redactor.untrusted(text), expected, "{text}");
    }
}

#[test]
fn hide_identity_labels_delegate_names_and_public_keys_only() {
    let identity = identity();
    let name = |text: &str| Id::new(IdKind::Username, text).unwrap();
    let key = Id::new(IdKind::PublicKey, &format!("02{}", "ab".repeat(32))).unwrap();
    let block = Id::new(IdKind::Hex64, &"cd".repeat(32)).unwrap();
    let version = Id::new(IdKind::Version, "4.3.1").unwrap();
    let mut shown = Redactor::new(&identity, true);
    assert_eq!(shown.id(&name("genesis_1")), "genesis_1");
    assert_eq!(shown.id(&key), key.as_str());
    let mut hidden = Redactor::new(&identity, true).hide_identity(true);
    assert_eq!(hidden.id(&name("genesis_1")), "delegate-1");
    assert_eq!(hidden.id(&name("genesis_2")), "delegate-2");
    assert_eq!(hidden.id(&name("genesis_1")), "delegate-1");
    assert_eq!(hidden.id(&key), "pk-1");
    assert_eq!(hidden.id(&block), block.as_str());
    assert_eq!(hidden.id(&version), "4.3.1");
    assert_eq!(
        hidden.untrusted("Delegate genesis_1 forged; genesis_10 waits"),
        "Delegate delegate-1 forged; genesis_10 waits"
    );
    assert_eq!(
        shown.untrusted("Delegate genesis_1 forged"),
        "Delegate genesis_1 forged"
    );
}

#[test]
fn an_excerpt_is_capped_after_it_is_redacted() {
    let identity = identity();
    let mut redactor = Redactor::new(&identity, true);
    let long = "x".repeat(600);
    assert_eq!(redactor.untrusted(&long).chars().count(), MAX_EXCERPT_CHARS);
    let wide = "é".repeat(300);
    assert_eq!(redactor.untrusted(&wide), "é".repeat(MAX_EXCERPT_CHARS));
    let near_end = format!("{} 192.0.2.123 {}", "y".repeat(150), "z".repeat(40));
    let out = redactor.untrusted(&near_end);
    assert_eq!(out.chars().count(), MAX_EXCERPT_CHARS);
    assert!(!out.contains("192"), "{out}");
    assert!(out.ends_with(" addr-1 (p"), "{out}");
    let hex = format!("{}{}", "q".repeat(144), "ab".repeat(40));
    let out = redactor.untrusted(&hex);
    assert!(out.ends_with("[hex, 80 charact"), "{out}");
}

/// A small deterministic generator, so that the sweeps below are the same on every run.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self, below: u64) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 33) % below
    }
}

#[test]
fn no_address_between_common_delimiters_survives_and_no_input_panics() {
    let identity = identity();
    let mut redactor = Redactor::new(&identity, true);
    let mut lcg = Lcg(7);
    let before = [" ", ",", "(", "[", "=", "/", ";", "\"", "'", ":", "\t"];
    let after = [" ", ",", ")", "]:4001", "/", ";", "\"", "'", ":", "\t", "."];
    for _ in 0..2_000 {
        let v4 = std::net::Ipv4Addr::from(u32::try_from(lcg.next(1 << 32)).unwrap());
        let v6 = std::net::Ipv6Addr::from(
            u128::from(lcg.next(1 << 32)) << 96 | u128::from(lcg.next(1 << 16)),
        );
        for address in [v4.to_string(), v6.to_string(), format!("::ffff:{v4}")] {
            let before = before[usize::try_from(lcg.next(11)).unwrap()];
            let after = after[usize::try_from(lcg.next(11)).unwrap()];
            let text = format!("peer{before}{address}{after}end");
            let out = redactor.untrusted(&text);
            assert!(!out.contains(&address), "{text:?} gave {out:?}");
        }
    }
    let alphabet: Vec<char> =
        "0123456789abcdefABCDEF:.%[]-_ xé\u{1b}\u{9b}\u{7}\n\u{202e}\u{200b}\u{ad}\u{e0041}\u{fe0f}\u{3164}\\;/"
            .chars()
            .collect();
    for _ in 0..5_000 {
        let len = usize::try_from(lcg.next(300)).unwrap();
        let text: String = (0..len)
            .map(|_| alphabet[usize::try_from(lcg.next(alphabet.len() as u64)).unwrap()])
            .collect();
        let out = redactor.untrusted(&text);
        assert!(out.chars().count() <= MAX_EXCERPT_CHARS);
        assert!(
            !out.chars().any(|c| c.is_control()
                || matches!(
                    c,
                    '\u{202e}' | '\u{200b}' | '\u{ad}' | '\u{e0041}' | '\u{fe0f}'
                )),
            "{out:?}"
        );
    }
}
