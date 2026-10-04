//! The line templates against the node's own source: every template's pieces are found, in
//! order, in one log call of the file it names, so a reworded message breaks this test and not a
//! check. A second test lists every log call that interpolates text from outside the process.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use heartwood_cli::Role;
use heartwood_cli::doctor::sources::logs::{LineKind, carries_outside};
use heartwood_cli::doctor::sources::patterns::{TEMPLATES, classify};
use heartwood_kernel::log::Level;

/// The node's source files that hold the log calls the templates were written from.
const SOURCES: &[(&str, &str)] = &[
    (
        "packages/api/src/errors.rs",
        include_str!("../../../packages/api/src/errors.rs"),
    ),
    (
        "packages/blockchain/src/blockchain.rs",
        include_str!("../../../packages/blockchain/src/blockchain.rs"),
    ),
    (
        "packages/blockchain/src/forger_blocks.rs",
        include_str!("../../../packages/blockchain/src/forger_blocks.rs"),
    ),
    (
        "packages/blockchain/src/fork_recovery.rs",
        include_str!("../../../packages/blockchain/src/fork_recovery.rs"),
    ),
    (
        "packages/blockchain/src/node.rs",
        include_str!("../../../packages/blockchain/src/node.rs"),
    ),
    (
        "packages/blockchain/src/process_blocks_job.rs",
        include_str!("../../../packages/blockchain/src/process_blocks_job.rs"),
    ),
    (
        "packages/blockchain/src/processor/unchained.rs",
        include_str!("../../../packages/blockchain/src/processor/unchained.rs"),
    ),
    (
        "packages/blockchain/src/state_machine/actions/sync.rs",
        include_str!("../../../packages/blockchain/src/state_machine/actions/sync.rs"),
    ),
    (
        "packages/core/src/app.rs",
        include_str!("../../../packages/core/src/app.rs"),
    ),
    (
        "packages/core/src/app/forger.rs",
        include_str!("../../../packages/core/src/app/forger.rs"),
    ),
    (
        "packages/core/src/app/relay.rs",
        include_str!("../../../packages/core/src/app/relay.rs"),
    ),
    (
        "packages/forger/src/attestation.rs",
        include_str!("../../../packages/forger/src/attestation.rs"),
    ),
    (
        "packages/forger/src/client.rs",
        include_str!("../../../packages/forger/src/client.rs"),
    ),
    (
        "packages/forger/src/errors.rs",
        include_str!("../../../packages/forger/src/errors.rs"),
    ),
    (
        "packages/forger/src/forger_service.rs",
        include_str!("../../../packages/forger/src/forger_service.rs"),
    ),
    (
        "packages/forger/src/keys.rs",
        include_str!("../../../packages/forger/src/keys.rs"),
    ),
    (
        "packages/forger/src/signer.rs",
        include_str!("../../../packages/forger/src/signer.rs"),
    ),
    (
        "packages/forger/src/signing_record.rs",
        include_str!("../../../packages/forger/src/signing_record.rs"),
    ),
    (
        "packages/kernel/src/config/floor.rs",
        include_str!("../../../packages/kernel/src/config/floor.rs"),
    ),
    (
        "packages/p2p/src/attestation_session.rs",
        include_str!("../../../packages/p2p/src/attestation_session.rs"),
    ),
    (
        "packages/p2p/src/monitor.rs",
        include_str!("../../../packages/p2p/src/monitor.rs"),
    ),
    (
        "packages/p2p/src/server/controllers/blocks.rs",
        include_str!("../../../packages/p2p/src/server/controllers/blocks.rs"),
    ),
    (
        "packages/p2p/src/server/controllers/mod.rs",
        include_str!("../../../packages/p2p/src/server/controllers/mod.rs"),
    ),
    (
        "packages/p2p/src/server/mod.rs",
        include_str!("../../../packages/p2p/src/server/mod.rs"),
    ),
];

/// The emoji table of the logger: a code, a tab, its emoji.
const EMOJI: &str = include_str!("../../logger/src/emoji.tsv");

/// The macros that log, by the level they log at.
const MACROS: &[(&str, Level)] = &[
    ("error", Level::Error),
    ("warning", Level::Warning),
    ("info", Level::Info),
    ("debug", Level::Debug),
    ("notice", Level::Notice),
    ("critical", Level::Critical),
    ("alert", Level::Alert),
    ("emergency", Level::Emergency),
];

/// One log call: the level of its macro, the string literals of its arguments (escapes undone,
/// doubled braces made single) joined by a `\u{1}`.
#[derive(Debug)]
struct Site {
    level: Level,
    text: String,
    /// What is outside the literals, whitespace collapsed: the arguments.
    code: String,
}

/// The string literal that starts at `at` (a quote): the index past it and its content.
fn string_at(bytes: &[u8], at: usize) -> Option<(usize, String)> {
    let mut i = at + 1;
    let mut out = Vec::new();
    while i < bytes.len() {
        match bytes[i] {
            b'"' => return Some((i + 1, String::from_utf8_lossy(&out).into_owned())),
            b'\\' => {
                let next = *bytes.get(i + 1)?;
                i += 2;
                if next == b'\n' || next == b'\r' {
                    // A line continuation: the break and the spaces of the next line go.
                    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                        i += 1;
                    }
                } else {
                    out.push(match next {
                        b'n' => b'\n',
                        b't' => b'\t',
                        other => other,
                    });
                }
            }
            byte => {
                out.push(byte);
                i += 1;
            }
        }
    }
    None
}

/// The raw string literal that starts at `at` (an `r`): the index past it and its content.
fn raw_string_at(bytes: &[u8], at: usize) -> Option<(usize, String)> {
    let mut i = at + 1;
    let mut hashes = 0;
    while bytes.get(i) == Some(&b'#') {
        hashes += 1;
        i += 1;
    }
    if bytes.get(i) != Some(&b'"') {
        return None;
    }
    let start = i + 1;
    (start..bytes.len()).find_map(|end| {
        let closes = bytes[end] == b'"' && (1..=hashes).all(|n| bytes.get(end + n) == Some(&b'#'));
        closes.then(|| {
            let text = String::from_utf8_lossy(&bytes[start..end]).into_owned();
            (end + 1 + hashes, text)
        })
    })
}

/// The index past whatever is at `at` when it is a comment or a character literal.
fn skip_at(bytes: &[u8], at: usize) -> Option<usize> {
    let rest = &bytes[at..];
    if rest.starts_with(b"//") {
        return Some(at + rest.iter().position(|&b| b == b'\n').unwrap_or(rest.len()));
    }
    if rest.starts_with(b"/*") {
        let end = rest.windows(2).position(|pair| pair == b"*/");
        return Some(at + end.map_or(rest.len(), |end| end + 2));
    }
    if rest.first() == Some(&b'\'') {
        // `'x'` and `'\x'` are literals; `'a` is a lifetime.
        let plain = rest.get(2) == Some(&b'\'');
        let escaped = rest.get(1) == Some(&b'\\') && rest.get(3) == Some(&b'\'');
        return Some(
            at + if plain {
                3
            } else if escaped {
                4
            } else {
                1
            },
        );
    }
    None
}

/// The log calls of `source`. Comments and character literals are skipped, so a parenthesis in
/// them does not end a call.
fn sites(source: &str) -> Vec<Site> {
    let bytes = source.as_bytes();
    let mut found = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if let Some(next) = skip_at(bytes, i) {
            i = next;
            continue;
        }
        if bytes[i] == b'"' {
            i = string_at(bytes, i).map_or(i + 1, |(next, _)| next);
            continue;
        }
        let starts_word = i == 0 || !(bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_');
        let rest = &bytes[i..];
        let macro_call = MACROS
            .iter()
            .find(|(name, _)| starts_word && rest.starts_with(format!("{name}!(").as_bytes()));
        let Some((name, level)) = macro_call else {
            i += 1;
            continue;
        };
        i += name.len() + 2;
        let (mut depth, mut text, mut code) = (1_u32, String::new(), String::new());
        while i < bytes.len() && depth > 0 {
            if let Some(next) = skip_at(bytes, i) {
                i = next;
                continue;
            }
            let raw = bytes[i] == b'r'
                && matches!(bytes.get(i + 1), Some(b'"' | b'#'))
                && !(i > 0 && (bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_'));
            let literal = match bytes[i] {
                b'"' => string_at(bytes, i),
                _ if raw => raw_string_at(bytes, i),
                _ => None,
            };
            if let Some((next, content)) = literal {
                text.push_str(&content.replace("{{", "{").replace("}}", "}"));
                text.push('\u{1}');
                i = next;
                continue;
            }
            match bytes[i] {
                b'(' => depth += 1,
                b')' => depth -= 1,
                _ => {}
            }
            if depth > 0 {
                let byte = char::from(bytes[i]);
                code.push(if byte.is_ascii_whitespace() {
                    ' '
                } else {
                    byte
                });
            }
            i += 1;
        }
        found.push(Site {
            level: *level,
            text,
            code,
        });
    }
    found
}

/// The logger's emoji table: code and glyph.
fn table() -> impl Iterator<Item = (&'static str, &'static str)> {
    let lines = EMOJI.lines().filter(|line| !line.starts_with('#'));
    lines.filter_map(|line| line.split_once('\t'))
}

/// The emoji codes of a glyph (several codes can name one), byte for byte: the reader matches
/// the glyph the logger writes exactly, variation selector included.
fn codes(glyph: &str) -> Vec<String> {
    let found = table().filter(|(_, emoji)| *emoji == glyph);
    found.map(|(code, _)| format!(":{code}:")).collect()
}

/// `text` with each emoji code the table knows replaced by its glyph, as the logger writes it.
fn glyphs(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some((before, after)) = rest.split_once(':') {
        out.push_str(before);
        let known = after.split_once(':').and_then(|(code, tail)| {
            let glyph = table().find(|(name, _)| *name == code)?.1;
            Some((glyph, tail))
        });
        match known {
            Some((glyph, tail)) => {
                out.push_str(glyph);
                rest = tail;
            }
            None => {
                out.push(':');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Whether the message of `literal` (a call's format string, or a whole message the node builds
/// before its call) has the whole shape of the template, as the reader matches it: the first piece
/// starts it, the last piece ends it before its emoji, the pieces between follow in order with a
/// placeholder or text between each two, and the emoji the logger writes for its codes ends it.
fn anchored(literal: &str, pieces: &[&str], emoji: Option<&str>) -> bool {
    let message = glyphs(literal);
    let body = match emoji {
        Some(emoji) => message
            .strip_suffix(emoji)
            .and_then(|m| m.strip_suffix(' ')),
        None => Some(message.as_str()),
    };
    let Some(mut text) = body else {
        return false;
    };
    let Some((first, rest)) = pieces.split_first() else {
        return false;
    };
    let Some(after) = text.strip_prefix(first) else {
        return false;
    };
    text = after;
    let Some((last, inner)) = rest.split_last() else {
        return text.is_empty();
    };
    let Some(before) = text.strip_suffix(last) else {
        return false;
    };
    text = before;
    inner.iter().all(|piece| match text.split_once(piece) {
        Some((_, after)) => {
            text = after;
            true
        }
        None => false,
    })
}

/// Whether `text` holds `pieces` in order, and then the trailing `emoji` glyphs as codes in
/// order (space-joined). The first piece may be anywhere: a call's own literal is not the message
/// alone when other text is put in front of it.
fn holds(text: &str, pieces: &[&str], emoji: Option<&str>) -> bool {
    let mut at = 0;
    for piece in pieces.iter().filter(|piece| !piece.is_empty()) {
        match text[at..].find(piece) {
            Some(found) => at += found + piece.len(),
            None => return false,
        }
    }
    let Some(emoji) = emoji else {
        return true;
    };
    // A glyph that is one emoji or a run of them (`🎉 🎉`): each is found after the last.
    emoji.split(' ').all(|glyph| {
        let found = codes(glyph)
            .iter()
            .filter_map(|code| text[at..].find(code.as_str()).map(|i| (i, code.len())))
            .min();
        found.is_some_and(|(i, len)| {
            at += i + len;
            true
        })
    })
}

/// Templates whose message is put together from more than one literal or built before the call.
/// Their pieces are each found in some string literal of the file, in no fixed place. An entry
/// that one log call holds after all is stale and fails.
const COMPOSED: &[&str] = &[
    "ActivePositionEntered",
    "ActivePositionLeft",
    "DelegatesLoaded",
    "NotPeeredEnough",
    "ColdStartSkip",
    "NoNetworkState",
    "OverheightHeaders",
];

/// Templates of a message the node writes as one string literal that is not the argument of a log
/// call: an error's text (`#[error("...")]`), or a warning the loader builds and a log call
/// prints as it is. The whole message is found in one literal. Every template of a start error
/// belongs here.
fn is_literal(kind: &str) -> bool {
    const OTHERS: &[&str] = &[
        "RemoteAccessNoMatch",
        "RemoteAccessEmpty",
        "RelayRequestFailed",
        "HostsDidNotRespond",
    ];
    kind.starts_with("StartError") || OTHERS.contains(&kind)
}

/// The string literals of `source`, comments skipped.
fn literals(source: &str) -> Vec<String> {
    let bytes = source.as_bytes();
    let mut found = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if let Some(next) = skip_at(bytes, i) {
            i = next;
        } else if bytes[i] == b'"' {
            let (next, text) = string_at(bytes, i).unwrap_or((i + 1, String::new()));
            found.push(text.replace("{{", "{").replace("}}", "}"));
            i = next;
        } else {
            i += 1;
        }
    }
    found
}

fn source_of(path: &str) -> &'static str {
    let source = SOURCES.iter().find(|(name, _)| *name == path);
    source.map_or("", |(_, source)| source)
}

/// Whether each piece of a composed message is in some literal of `literals`.
fn scattered(literals: &[String], pieces: &[&str], emoji: Option<&str>) -> bool {
    let has = |needle: &str| literals.iter().any(|literal| literal.contains(needle));
    let pieces_found = pieces
        .iter()
        .map(|piece| piece.trim())
        .filter(|piece| !piece.is_empty())
        .all(has);
    let emoji_found = emoji.is_none_or(|emoji| {
        emoji
            .split(' ')
            .all(|glyph| codes(glyph).iter().any(|code| has(code)))
    });
    pieces_found && emoji_found
}

#[test]
fn every_template_is_found_piece_by_piece_in_the_node_s_source() {
    let mut missing = Vec::new();
    let mut stale = Vec::new();
    for template in TEMPLATES {
        let sites = sites(source_of(template.source));
        let same_level = sites.iter().filter(|site| site.level == template.level);
        let in_a_call = same_level
            .clone()
            .any(|site| holds(&site.text, template.pieces, template.emoji));
        // The call's own format string, not only its pieces somewhere in it.
        let whole_call = same_level.clone().any(|site| {
            let format = site.text.split('\u{1}').next().unwrap_or_default();
            anchored(format, template.pieces, template.emoji)
        });
        let literals = literals(source_of(template.source));
        let described = format!(
            "{} in {}: {:?}",
            template.kind, template.source, template.pieces
        );
        if COMPOSED.contains(&template.kind) {
            match (
                in_a_call,
                scattered(&literals, template.pieces, template.emoji),
            ) {
                (true, _) => stale.push(described),
                (false, false) => missing.push(described),
                (false, true) => {}
            }
        } else if is_literal(template.kind) {
            let one = literals
                .iter()
                .any(|literal| anchored(literal, template.pieces, template.emoji));
            if !whole_call && !one {
                missing.push(described);
            }
        } else if !whole_call {
            missing.push(described);
        }
    }
    assert!(
        missing.is_empty(),
        "no source holds:\n{}",
        missing.join("\n")
    );
    assert!(
        stale.is_empty(),
        "held by a log call, not composed:\n{}",
        stale.join("\n")
    );
}

#[test]
fn a_composed_or_literal_template_with_no_such_kind_is_a_stale_entry() {
    for kind in COMPOSED {
        assert!(TEMPLATES.iter().any(|t| t.kind == *kind), "{kind}");
    }
}

#[test]
fn the_sources_listed_are_the_ones_the_templates_name_and_every_emoji_has_a_code() {
    let named: BTreeSet<&str> = TEMPLATES.iter().map(|t| t.source).collect();
    let listed: BTreeSet<&str> = SOURCES.iter().map(|(name, _)| *name).collect();
    assert_eq!(named, listed);
    for template in TEMPLATES {
        for glyph in template.emoji.iter().flat_map(|emoji| emoji.split(' ')) {
            assert!(!codes(glyph).is_empty(), "{glyph} of {}", template.kind);
        }
    }
}

#[test]
fn the_check_finds_pieces_in_order_and_fails_a_reworded_or_reordered_call() {
    let call = r#"
        // warning!("a comment (with a paren")
        let quote = '(';
        warning!("Delegate {name} will not \
                  sign: {refused} :bomb:", other(")"));
        let lifetime: &'static str = "x";
        info!("second call");
    "#;
    let found = sites(call);
    assert_eq!(found.len(), 2, "{found:?}");
    assert_eq!(found[0].level, Level::Warning);
    assert!(
        found[0]
            .text
            .starts_with("Delegate {name} will not sign: {refused} :bomb:")
    );
    assert_eq!(found[1].level, Level::Info);
    let text = &found[0].text;
    assert!(holds(
        text,
        &["Delegate ", " will not sign: ", ""],
        Some("💣")
    ));
    assert!(!holds(
        text,
        &["Delegate ", " will refuse to sign: ", ""],
        Some("💣")
    ));
    assert!(!holds(text, &[" will not sign: ", "Delegate "], None));
    assert!(!holds(
        text,
        &["Delegate ", " will not sign: ", ""],
        Some("🎉")
    ));
    // The whole call's shape: text added before the first piece or after the last is drift.
    let format = text.split('\u{1}').next().unwrap_or_default();
    assert!(anchored(
        format,
        &["Delegate ", " will not sign: ", ""],
        Some("💣")
    ));
    // Without an emoji, a last piece must end the message.
    assert!(!anchored(
        "Blockchain 100% in sync :100:",
        &["Blockchain 100% in sync"],
        None
    ));
    assert!(!anchored(
        "The Delegate {name} will not sign: {refused} :bomb:",
        &["Delegate ", " will not sign: ", ""],
        Some("💣")
    ));
    assert!(!anchored(
        "Blockchain 100% in sync now :100:",
        &["Blockchain 100% in sync"],
        Some("💯")
    ));
    assert!(anchored(
        "Blockchain 100% in sync :100:",
        &["Blockchain 100% in sync"],
        Some("💯")
    ));
    // A glyph without its variation selector is not the one the logger writes.
    assert!(!anchored(
        "Removing {n}. Reset to height {h} :warning:",
        &["Removing ", ". Reset to height ", ""],
        Some("\u{26a0}")
    ));
    // Doubled braces are single ones in the message.
    let braces = sites(r#"info!("{{braced}} {x}");"#);
    assert_eq!(braces[0].text, "{braced} {x}\u{1}");
    assert!(sites("let warning = 1; awarning!(\"x\");").is_empty());
}

/// The words of `text` that are identifiers.
fn words(text: &str) -> Vec<&str> {
    let word = |c: char| c.is_ascii_alphanumeric() || c == '_';
    text.split(|c: char| !word(c))
        .filter(|w| !w.is_empty())
        .collect()
}

/// The names put inline in a format string (`{name}`, `{name:?}`).
fn inline_names(text: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut rest = text;
    while let Some((_, after)) = rest.split_once('{') {
        let Some((inside, next)) = after.split_once('}') else {
            break;
        };
        names.push(inside.split(':').next().unwrap_or("").to_owned());
        rest = next;
    }
    names
}

/// The `.rs` files below `dir`, sorted.
fn rust_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(next) = pending.pop() {
        let Ok(entries) = fs::read_dir(&next) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

/// What the reader makes of a log call that puts text from outside the process into a line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Class {
    /// A template's capture holds the text, and the kind is one that
    /// [`carries_outside`]: its excerpt is kept, and forgeries inside it are looked for.
    Carrier(&'static str),
    /// A template covers the line, and the text in it is the node's own or the operator's.
    Own(&'static str),
    /// No template covers the line: it is an other line, never an event.
    Untyped,
}

/// The calls of the node's crates (the CLI's own excepted) that log an error-like value: their
/// file below `packages/`, the first literal of the call, and what the reader makes of them. A new
/// call of this kind fails the test until it is classified here.
const CALLS: &[(&str, &str, Class)] = &[
    (
        "state/src/errors.rs",
        "Taking back the {step} failed ({error}); the state is no longer consistent",
        Class::Untyped,
    ),
    (
        "database/src/reader.rs",
        "The store's log could not be reset: {error}",
        Class::Untyped,
    ),
    (
        "api/src/productivity.rs",
        "The productivity table could not be computed: {error}",
        Class::Untyped,
    ),
    (
        "api/src/productivity.rs",
        "The chain's totals could not be read: {error}",
        Class::Untyped,
    ),
    ("blockchain/src/blockchain.rs", "{error}", Class::Untyped),
    (
        "blockchain/src/node.rs",
        "The node stops: {fatal}",
        Class::Carrier("NodeStops"),
    ),
    (
        "blockchain/src/process_blocks_job.rs",
        "{why}",
        Class::Untyped,
    ),
    (
        "blockchain/src/processor/handlers.rs",
        "Block {} refused by the {}",
        Class::Untyped,
    ),
    (
        "blockchain/src/processor/handlers.rs",
        "Critical error occurs when reverting block at height {}. Possible state corruption. Message: {error}",
        Class::Untyped,
    ),
    (
        "blockchain/src/processor/handlers.rs",
        "Block contains a bad transaction: {rejected} :no_entry:",
        Class::Untyped,
    ),
    (
        "blockchain/src/processor/handlers.rs",
        "Block is bad: {error} :no_entry:",
        Class::Untyped,
    ),
    ("blockchain/src/processor/mod.rs", "{}", Class::Untyped),
    (
        "blockchain/src/processor/mod.rs",
        "Block rejected: its reward cannot be computed: {error}",
        Class::Untyped,
    ),
    (
        "blockchain/src/state_machine/actions/mod.rs",
        "{error}",
        Class::Untyped,
    ),
    ("core/src/app/forger.rs", "{w}", Class::Untyped),
    ("core/src/app/forger.rs", "{error}", Class::Untyped),
    ("core/src/app/relay.rs", "{warning}", Class::Untyped),
    ("core/src/app/relay.rs", "{error}", Class::Untyped),
    (
        "core/src/app/relay.rs",
        "{error}: the API is not started",
        Class::Own("ApiNotStarted"),
    ),
    (
        "core/src/harness.rs",
        "the harness's record could not be written: {error}",
        Class::Untyped,
    ),
    (
        "core/src/markers.rs",
        "Ignored {}: {}: {why} :warning:",
        Class::Untyped,
    ),
    ("core/src/signals.rs", "{message}", Class::Untyped),
    (
        "api/src/controllers/transactions.rs",
        "A read of the store for the API failed: {error}",
        Class::Untyped,
    ),
    (
        "api/src/controllers/transactions.rs",
        "A read of the store for the API did not finish: {error}",
        Class::Untyped,
    ),
    (
        "api/src/controllers/transactions.rs",
        "The stored transaction {} cannot be read: {error}",
        Class::Untyped,
    ),
    (
        "api/src/controllers/transactions.rs",
        "A transaction criteria of the API was refused: {error}",
        Class::Untyped,
    ),
    (
        "forger/src/attestation.rs",
        "The relay gave no status to attest: {error}",
        Class::Carrier("NoStatusToAttest"),
    ),
    (
        "forger/src/attestation.rs",
        "The status attestations were not delivered: {error}",
        Class::Carrier("AttestationsNotDelivered"),
    ),
    (
        "forger/src/attestation.rs",
        "Not attesting the relay's status: {error}",
        Class::Carrier("NotAttesting"),
    ),
    (
        "forger/src/client.rs",
        "{message} :bangbang:",
        Class::Carrier("SessionError"),
    ),
    (
        "forger/src/client.rs",
        "Broadcast block failed: {failure} :bangbang:",
        Class::Carrier("BroadcastFailed"),
    ),
    (
        "forger/src/client.rs",
        "Could not sync check: {failure} :bangbang:",
        Class::Carrier("SyncCheckFailed"),
    ),
    (
        "forger/src/forger_service.rs",
        "{text}",
        Class::Carrier("RelayRequestFailed"),
    ),
    (
        "forger/src/forger_service.rs",
        "{refusal}. Will not forge :bomb:",
        Class::Own("ColdStartSkip"),
    ),
    (
        "forger/src/signer.rs",
        "Delegate {name} will not sign: {refused}",
        Class::Own("WillNotSign"),
    ),
    (
        "p2p/src/communicator.rs",
        "Socket data validation error (peer {ip}) : Response validation failed from peer {ip} : {refused} :warning:",
        Class::Untyped,
    ),
    (
        "p2p/src/monitor.rs",
        "Removed {count} because of \"{failure}\" :wastebasket:",
        Class::Untyped,
    ),
    (
        "p2p/src/monitor.rs",
        "{message} :exclamation:",
        Class::Untyped,
    ),
    (
        "p2p/src/monitor.rs",
        "Failed to download blocks {key} from {printed}: {why} :bangbang:",
        Class::Carrier("DownloadFailed"),
    ),
    (
        "p2p/src/monitor.rs",
        "Giving up on trying to download blocks {key}: {why} :bangbang:",
        Class::Carrier("DownloadFailed"),
    ),
    (
        "p2p/src/server/ban_hammer.rs",
        "Banning {ip} for {duration}. Reason: {reason} :oncoming_police_car:",
        Class::Untyped,
    ),
    (
        "p2p/src/server/controllers/mod.rs",
        "{text}",
        Class::Own("RemoteAccessNoMatch"),
    ),
    (
        "p2p/src/testing/attestation.rs",
        "the attestation record could not be written: {error}",
        Class::Untyped,
    ),
];

/// The names an outside text goes by in a log call.
const OUTSIDE: &[&str] = &[
    "error", "err", "message", "msg", "text", "why", "failure", "fatal", "refused", "refusal",
    "reason", "warning", "w", "rejected", "detail", "cause",
];

/// The kind of a sample line of `kind`, as the reader classifies it.
fn sample_of(kind: &str) -> Option<heartwood_cli::doctor::sources::logs::LineKind> {
    let template = TEMPLATES.iter().find(|t| t.kind == kind)?;
    const SAMPLES: &str = include_str!("fixtures/doctor/logs/samples.tsv");
    let row = SAMPLES
        .lines()
        .filter(|line| !line.starts_with('#'))
        .filter_map(|line| {
            let mut columns = line.splitn(4, '\t');
            let (_, _, name, message) = (
                columns.next()?,
                columns.next()?,
                columns.next()?,
                columns.next()?,
            );
            (name == kind).then_some(message)
        })
        .next()?;
    let message = match template.emoji {
        Some(emoji) => format!("{row} {emoji}"),
        None => row.to_owned(),
    };
    classify(template.role, template.level, &message)
}

#[test]
fn every_log_call_of_the_node_that_writes_an_outside_text_is_classified() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut found: BTreeMap<(String, String), usize> = BTreeMap::new();
    let packages = fs::read_dir(&root).expect("the packages of the workspace");
    for package in packages.flatten().filter(|p| p.file_name() != "cli") {
        for file in rust_files(&package.path().join("src")) {
            let Ok(source) = fs::read_to_string(&file) else {
                continue;
            };
            let name = file.strip_prefix(&root).unwrap_or(&file);
            for site in sites(&source) {
                let mut names: Vec<String> =
                    words(&site.code).into_iter().map(str::to_owned).collect();
                names.extend(inline_names(&site.text));
                if names.iter().any(|n| OUTSIDE.contains(&n.as_str())) {
                    let first = site.text.split('\u{1}').next().unwrap_or("").to_owned();
                    let key = (name.to_string_lossy().into_owned(), first);
                    *found.entry(key).or_default() += 1;
                }
            }
        }
    }
    let mut listed: BTreeMap<(String, String), usize> = BTreeMap::new();
    for (file, first, _) in CALLS {
        *listed
            .entry(((*file).to_owned(), (*first).to_owned()))
            .or_default() += 1;
    }
    let new: Vec<_> = found
        .iter()
        .filter(|(k, n)| listed.get(*k) != Some(n))
        .collect();
    let gone: Vec<_> = listed
        .iter()
        .filter(|(k, n)| found.get(*k) != Some(n))
        .collect();
    assert!(new.is_empty(), "calls not classified in CALLS: {new:#?}");
    assert!(
        gone.is_empty(),
        "classified calls that are no longer there: {gone:#?}"
    );
}

#[test]
fn the_kind_of_each_classified_call_is_a_template_and_carries_outside_as_told() {
    for (file, first, class) in CALLS {
        match class {
            Class::Untyped => {}
            Class::Carrier(kind) | Class::Own(kind) => {
                let line = sample_of(kind).unwrap_or_else(|| panic!("no sample of {kind}: {file}"));
                let carries = carries_outside(&line);
                assert_eq!(
                    carries,
                    matches!(class, Class::Carrier(_)),
                    "{kind}: {first}"
                );
            }
        }
    }
}

#[test]
fn no_untyped_call_has_a_template_that_would_classify_its_message() {
    // A call listed as untyped whose text a template holds is a template that was never noticed.
    for (file, first, _) in CALLS
        .iter()
        .filter(|(_, _, class)| *class == Class::Untyped)
    {
        let text = first.replace(":bangbang:", "");
        let matched = TEMPLATES.iter().any(|t| {
            t.source == format!("packages/{file}")
                && holds(&format!("{text}\u{1}"), t.pieces, t.emoji)
        });
        assert!(!matched, "{file}: {first}");
    }
}

/// `text` with each placeholder (`{slot}`) filled with a whole number.
fn filled(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some((before, after)) = rest.split_once('{') {
        out.push_str(before);
        out.push_str("80");
        rest = after.split_once('}').map_or("", |(_, tail)| tail);
    }
    out.push_str(rest);
    out
}

#[test]
fn the_slot_refusals_the_reader_calls_far_are_the_node_s_own_messages() {
    // The signer's and the attester's refusal of a slot too far from the forger's clock, as the
    // forger's error types word them; the reader takes "far" from these exact shapes only.
    const KEY: &str = "02cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd";
    let errors = literals(source_of("packages/forger/src/errors.rs"));
    let far: Vec<String> = (errors.iter())
        .filter(|literal| literal.contains("more than one slot away"))
        .map(|literal| filled(literal))
        .collect();
    assert_eq!(far.len(), 2, "{far:?}");
    let signs = far.iter().filter(|text| {
        let line = format!("Delegate {KEY} will not sign: {text}");
        let kind = classify(Role::Forger, Level::Warning, &line);
        matches!(kind, Some(LineKind::WillNotSign { far_slot: true, .. }))
    });
    let attests = far.iter().filter(|text| {
        let line = format!("Not attesting the relay's status: {text}");
        let kind = classify(Role::Forger, Level::Warning, &line);
        matches!(kind, Some(LineKind::NotAttesting { far_slot: true }))
    });
    assert_eq!((signs.count(), attests.count()), (1, 1), "{far:?}");
}
