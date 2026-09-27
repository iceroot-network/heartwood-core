//! IP addresses and address lists.
//!
//! Address validity follows the reference's address library (`ipaddr.js`): IPv4 in one to four
//! dotted parts, each decimal, octal (a leading `0`) or hexadecimal (`0x`), with the last part
//! filling the remaining bytes; IPv6 with at most one `::`, an optional zone (`%eth0`) and an
//! optional dotted IPv4 tail. The brackets of `[::1]` are removed first, the first `[` and the
//! first `]` only, as the reference removes them.
//!
//! Whitelists and blacklists match each entry against the address text as the reference's glob
//! matcher does:
//!
//! - an entry with no glob syntax (none of `* ? + ( ) | [ ] { }`, and no leading `!` or `.`)
//!   matches the identical text only;
//! - a glob entry is supported when it is a leading run of `!` (an odd count negates the rest)
//!   followed by letters, digits, `:`, `%`, `-`, `]`, `^`, `$`, `!`, `.`, `*`, `?` and classes:
//!   `[` with an optional `!` or `^`, then members other than `]`, `\` and `/`, then `]`. `*`
//!   matches any run, `?` one character, and a run of `?` or a `*` that starts the entry (the
//!   `*` after any `!`) matches no text starting with `.`; a run of `n` dots matches a `.` and
//!   `n − 1` more characters; a negated class matches neither its members nor `.`; `[x]` is `x`,
//!   `[!]` is `!` and `[^]` any character. A `[` that no `]` closes, and the `[` of `[]`, stand
//!   for themselves, and so do the characters up to the last one that is not `*` or `?`, which
//!   is read as the reference's regular expression reads it (`.` any character, `^` nothing);
//!   the `!` or `^` after such a `[` is dropped when two characters follow it. A class whose range
//!   runs backwards (`[9-0]`, or `[!0-]`, whose negation appends `.`) breaks the reference's
//!   regular expression, and the entry then matches no address;
//! - any other glob entry (with `+`, `(`, `{`, `|`, quotes, a backslash and the like), and any
//!   address text that holds `/`, `\` or a line break (no socket address does), matches nothing.
//!   The reference compiles such entries with its full glob syntax; this is the one place where
//!   the two differ.
//!
//! An empty whitelist allows everyone and an empty blacklist bans no one. Remote access lists
//! instead match exact addresses, compared after [`normalise_address`], and an empty one allows no
//! one.

/// The longest glob entry the reference compiles, in UTF-16 code units.
const MAX_GLOB: usize = 65_536;
const DOT: u16 = 0x2e;

/// Whether `ip` (brackets removed) is a valid IPv4 or IPv6 address.
pub fn is_valid_address(ip: &str) -> bool {
    let ip = clean_address(ip);
    is_ipv6(&ip) || is_ipv4(&ip)
}

/// Whether `ip` (brackets removed) is an IPv6 address.
pub fn is_ipv6_address(ip: &str) -> bool {
    is_ipv6(&clean_address(ip))
}

/// `ip` without brackets, then in brackets if it is an IPv6 address.
pub fn normalise_address(ip: &str) -> String {
    let ip = clean_address(ip);
    if is_ipv6_address(&ip) {
        format!("[{ip}]")
    } else {
        ip
    }
}

/// `ip` with the first `[` and the first `]` removed.
pub fn clean_address(ip: &str) -> String {
    ip.replacen('[', "", 1).replacen(']', "", 1)
}

/// Whether `remote_address` matches an entry of `whitelist`, or the whitelist is empty.
pub fn is_whitelisted(whitelist: &[String], remote_address: &str) -> bool {
    whitelist.is_empty()
        || whitelist
            .iter()
            .any(|entry| glob_matches(remote_address, entry))
}

/// Whether `remote_address` matches an entry of `blacklist`.
pub fn is_blacklisted(blacklist: &[String], remote_address: &str) -> bool {
    blacklist
        .iter()
        .any(|entry| glob_matches(remote_address, entry))
}

/// Whether `remote_address` is one of the addresses of `allowed`, compared after normalisation;
/// an empty list allows no one.
pub fn is_allowed_exactly(allowed: &[String], remote_address: &str) -> bool {
    let remote = normalise_address(remote_address);
    allowed
        .iter()
        .any(|entry| normalise_address(entry) == remote)
}

/// `ipaddr.js`'s `IPv4.isValid`.
fn is_ipv4(text: &str) -> bool {
    let Some(values) = text.split('.').map(ipv4_part).collect::<Option<Vec<u64>>>() else {
        return false;
    };
    match values.as_slice() {
        [whole] => *whole <= 0xffff_ffff,
        [a, rest] => *a <= 0xff && *rest <= 0xff_ffff,
        [a, b, rest] => *a <= 0xff && *b <= 0xff && *rest <= 0xffff,
        [a, b, c, d] => [a, b, c, d].iter().all(|octet| **octet <= 0xff),
        _ => false,
    }
}

/// One dotted IPv4 part as `ipaddr.js` reads it: decimal digits, `0` and octal digits, or `0x`
/// and hexadecimal digits. `None` for anything else, a leading `0` included before an `8` or a
/// `9`. Large values saturate; every limit they are compared with is far below. The peer
/// client's address rules read the parts with it.
pub fn ipv4_part(part: &str) -> Option<u64> {
    if let Some(hex) = part.strip_prefix("0x").or_else(|| part.strip_prefix("0X")) {
        return radix_value(hex, 16);
    }
    match part.as_bytes() {
        [b'0', rest @ ..] if !rest.is_empty() => radix_value(part.get(1..)?, 8),
        _ => radix_value(part, 10),
    }
}

/// `text` as digits of `radix`, saturating; `None` when it is empty or holds another character.
fn radix_value(text: &str, radix: u32) -> Option<u64> {
    if text.is_empty() {
        return None;
    }
    text.chars().try_fold(0_u64, |value, digit| {
        let digit = digit.to_digit(radix)?;
        Some(
            value
                .saturating_mul(u64::from(radix))
                .saturating_add(u64::from(digit)),
        )
    })
}

/// `ipaddr.js`'s `IPv6.isValid`: the parser's parts, eight of at most 16 bits.
fn is_ipv6(text: &str) -> bool {
    text.contains(':')
        && ipv6_parts(text)
            .is_some_and(|parts| parts.len() == 8 && parts.iter().all(|p| *p <= 0xffff))
}

/// `ipaddr.js`'s `IPv6.parser`: the deprecated `::a.b.c.d` form, the plain form, then the form
/// with an IPv4 tail. The groups may be other than eight or above 16 bits, which `ipaddr.js`
/// refuses; the peer client's address rules read addresses with it.
pub fn ipv6_parts(text: &str) -> Option<Vec<u64>> {
    // Every form may end with a zone, `%` and letters or digits, and has no `%` before it.
    let (body, zone) = match text.find('%') {
        Some(at) => {
            let zone = text.get(at..)?;
            let name = zone.get(1..)?;
            if name.is_empty() || !name.bytes().all(|b| b.is_ascii_alphanumeric()) {
                return None;
            }
            (text.get(..at)?, zone)
        }
        None => (text, ""),
    };
    if let Some(quad) = body.strip_prefix("::").filter(|quad| is_quad(quad)) {
        return ipv6_parts(&format!("::ffff:{quad}{zone}"));
    }
    if !body.contains('.') {
        return has_ipv6_shape(body).then(|| expand(body, 8)).flatten();
    }
    let colon = body.rfind(':')?;
    let (group, quad) = (body.get(..=colon)?, body.get(colon.checked_add(1)?..)?);
    if !has_ipv6_shape(group) || !is_quad(quad) {
        return None;
    }
    // The tail's octets are read with `parseInt` and no radix: decimal, or hexadecimal after `0x`.
    let octets = quad
        .split('.')
        .map(
            |part| match part.strip_prefix("0x").or_else(|| part.strip_prefix("0X")) {
                Some(hex) => radix_value(hex, 16),
                None => radix_value(part, 10),
            },
        )
        .collect::<Option<Vec<u64>>>()?;
    let mut parts = expand(group.get(..colon)?, 6)?;
    match octets.as_slice() {
        [a, b, c, d] if octets.iter().all(|octet| *octet <= 0xff) => {
            parts.push(a.saturating_mul(256).saturating_add(*b));
            parts.push(c.saturating_mul(256).saturating_add(*d));
            Some(parts)
        }
        _ => None,
    }
}

/// Hexadecimal runs and runs of one or two colons, starting with a run of digits or with `::`:
/// the text the plain form's pattern and the IPv4-tail form's prefix accept, less texts that
/// `expand` refuses anyway (three or more colons in a row).
fn has_ipv6_shape(text: &str) -> bool {
    let rest = text.strip_prefix("::").unwrap_or(text);
    (rest.is_empty() || rest.starts_with(|c: char| c.is_ascii_hexdigit()))
        && rest.chars().all(|c| c == ':' || c.is_ascii_hexdigit())
        && !text.contains(":::")
}

/// Four dotted parts, each decimal digits or `0x` and hexadecimal digits.
fn is_quad(text: &str) -> bool {
    let parts: Vec<&str> = text.split('.').collect();
    parts.len() == 4
        && parts.iter().all(|part| {
            let hex = part.strip_prefix("0x").or_else(|| part.strip_prefix("0X"));
            match hex {
                Some(digits) => !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_hexdigit()),
                None => !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()),
            }
        })
}

/// `ipaddr.js`'s `expandIPv6`: `::` filled with zeros up to `parts` groups, then each group read
/// as hexadecimal; an empty group reads as a value no group may have.
fn expand(text: &str, parts: usize) -> Option<Vec<u64>> {
    if text.find("::") != text.rfind("::") {
        return None;
    }
    let colons = text
        .matches(':')
        .count()
        .saturating_sub(usize::from(text.starts_with("::")))
        .saturating_sub(usize::from(text.ends_with("::")));
    let zeros = parts.checked_sub(colons)?;
    let mut text = text.replacen("::", &format!(":{}", "0:".repeat(zeros)), 1);
    if text.starts_with(':') {
        text.remove(0);
    }
    if text.ends_with(':') {
        text.pop();
    }
    Some(
        text.split(':')
            .map(|group| radix_value(group, 16).unwrap_or(u64::MAX))
            .collect(),
    )
}

/// Whether the reference's glob matcher matches `address` against `entry` (see the module
/// documentation for the supported entries).
fn glob_matches(address: &str, entry: &str) -> bool {
    if address.is_empty() || address == "./" || entry.is_empty() || entry == "./" {
        return false;
    }
    let special = entry.starts_with(['!', '.'])
        || entry.contains("/!")
        || entry.contains("/.")
        || entry.contains(['*', '?', '+', '(', ')', '|', '[', ']', '{', '}']);
    if !special {
        let unprefixed = address
            .strip_prefix("./")
            .or_else(|| address.strip_prefix(".\\"))
            .unwrap_or(address);
        return entry == address || entry == unprefixed;
    }
    let plain_address = !address.contains(['/', '\\', '\n', '\r', '\u{2028}', '\u{2029}']);
    if entry.len() > MAX_GLOB || !plain_address {
        return false;
    }
    Glob::compile(entry)
        .is_some_and(|glob| glob.matches(&address.encode_utf16().collect::<Vec<u16>>()))
}

/// A supported glob entry, compiled.
struct Glob {
    negated: bool,
    tokens: Vec<Token>,
}

enum Token {
    /// One character.
    Char(u16),
    /// A `.` and then this many more characters.
    Dots(usize),
    /// This many characters, none a `.` when `no_dot` (a run of `?` that starts the entry).
    Any { count: usize, no_dot: bool },
    /// Any run of characters; not one that starts with `.` when `no_dot` (a run of `*` that
    /// starts the entry, after any `!`, except `**` at the end after two or more `!`).
    Star { no_dot: bool },
    /// One character inside the ranges, or outside them and not `.` when negated.
    Class {
        negated: bool,
        ranges: Vec<(u16, u16)>,
    },
}

impl Glob {
    fn compile(entry: &str) -> Option<Glob> {
        let bangs = entry.bytes().take_while(|b| *b == b'!').count();
        let pattern = entry.as_bytes().get(bangs..)?;
        let mut tokens = Vec::new();
        let mut at = 0;
        while let Some(rest) = pattern.get(at..).filter(|rest| !rest.is_empty()) {
            let first = *rest.first()?;
            let run = rest.iter().take_while(|b| **b == first).count();
            let (token, used) = match first {
                b'*' => (
                    Token::Star {
                        no_dot: tokens.is_empty() && !(bangs >= 2 && run == 2 && rest.len() == 2),
                    },
                    run,
                ),
                b'?' => (
                    Token::Any {
                        count: run,
                        no_dot: tokens.is_empty() && bangs == 0,
                    },
                    run,
                ),
                b'.' => (Token::Dots(run), run),
                b'[' => {
                    let (class_tokens, used) = bracket(rest)?;
                    tokens.extend(class_tokens);
                    at = at.checked_add(used)?;
                    continue;
                }
                b if b.is_ascii_alphanumeric() || b":%-]^$!".contains(&b) => {
                    (Token::Char(u16::from(b)), 1)
                }
                _ => return None,
            };
            tokens.push(token);
            at = at.checked_add(used)?;
        }
        Some(Glob {
            negated: bangs & 1 == 1,
            tokens,
        })
    }

    fn matches(&self, text: &[u16]) -> bool {
        let mut reach = vec![false; text.len().saturating_add(1)];
        if let Some(start) = reach.first_mut() {
            *start = true;
        }
        for token in &self.tokens {
            let mut next = vec![false; reach.len()];
            for pos in (0..reach.len()).filter(|pos| reach.get(*pos) == Some(&true)) {
                let rest = text.get(pos..).unwrap_or_default();
                let used = match token {
                    Token::Char(c) => (rest.first() == Some(c)).then_some(1),
                    Token::Dots(count) => {
                        (rest.first() == Some(&DOT) && rest.len() >= *count).then_some(*count)
                    }
                    Token::Any { count, no_dot } => rest
                        .get(..*count)
                        .filter(|chars| !*no_dot || !chars.contains(&DOT))
                        .map(|_| *count),
                    Token::Star { no_dot } => {
                        if *no_dot && rest.first().is_none_or(|c| *c == DOT) {
                            break;
                        }
                        next.iter_mut().skip(pos).for_each(|slot| *slot = true);
                        break;
                    }
                    Token::Class { negated, ranges } => rest
                        .first()
                        .filter(|c| {
                            ranges.iter().any(|(low, high)| (low..=high).contains(c)) != *negated
                        })
                        .map(|_| 1),
                };
                if let Some(slot) = used.and_then(|used| next.get_mut(pos.saturating_add(used))) {
                    *slot = true;
                }
            }
            reach = next;
        }
        let whole = reach.last() == Some(&true);
        whole != self.negated
    }
}

/// The tokens of the `[` at the start of `text` and the length they take; `None` when it is not
/// supported or breaks the reference's regular expression (see the module documentation).
fn bracket(text: &[u8]) -> Option<(Vec<Token>, usize)> {
    match text {
        // `[x]` is the character itself, and `[!]` and `[^]` are read with the `!` or `^` as the
        // only member: `!`, and JavaScript's class of any character.
        [_, b'^', b']', ..] => {
            let any = Token::Any {
                count: 1,
                no_dot: false,
            };
            return Some((vec![any], 3));
        }
        [_, member, b']', ..] if *member != b'\\' => {
            return Some((vec![Token::Char(u16::from(*member))], 3));
        }
        [_, b']', b'-', ..] => return None,
        [_, b']', ..] => return Some((vec![Token::Char(u16::from(b'['))], 1)),
        _ => {}
    }
    let negated = matches!(text.get(1), Some(b'!' | b'^'));
    let start: usize = if negated { 2 } else { 1 };
    if !text.get(start..)?.contains(&b']') {
        return unclosed(text, start);
    }
    let (token, used) = class(text, negated, start)?;
    Some((vec![token], used))
}

/// A `[` that no `]` closes: the reference escapes it and the characters after it, except the
/// last one that is not `*`, `+` or `?`, which it leaves to its regular expression. The `!` or `^`
/// after the `[` is taken, and dropped, only when two such characters follow it.
fn unclosed(text: &[u8], start: usize) -> Option<(Vec<Token>, usize)> {
    let last = |body: &[u8]| {
        body.iter()
            .rposition(|b| !b"*+?".contains(b))
            .filter(|at| *at >= 1)
    };
    let (from, close) = match text.get(start..).and_then(last) {
        Some(close) => (start, close),
        None => match text.get(1..).and_then(last) {
            Some(close) => (1, close),
            None => return Some((vec![Token::Char(u16::from(b'['))], 1)),
        },
    };
    let end = from.checked_add(close)?;
    let inner = text.get(from..end)?;
    if inner
        .iter()
        .any(|b| !b.is_ascii_graphic() || b"\\/".contains(b))
    {
        return None;
    }
    let mut tokens = vec![Token::Char(u16::from(b'['))];
    tokens.extend(inner.iter().map(|b| Token::Char(u16::from(*b))));
    tokens.push(match *text.get(end)? {
        b'.' => Token::Any {
            count: 1,
            no_dot: false,
        },
        b'^' => Token::Class {
            negated: false,
            ranges: Vec::new(),
        },
        b if b.is_ascii_alphanumeric() || b":%-!".contains(&b) => Token::Char(u16::from(b)),
        _ => return None,
    });
    Some((tokens, end.checked_add(1)?))
}

/// The closed class at the start of `text`, with its members from `start`, and its length;
/// `None` when a member is not supported or a range runs backwards. A negated class also refuses
/// `.` and `/`, as the reference appends them to the class before the ranges are read.
fn class(text: &[u8], negated: bool, start: usize) -> Option<(Token, usize)> {
    let end = start.checked_add(text.get(start..)?.iter().position(|b| *b == b']')?)?;
    let mut members = text.get(start..end)?.to_vec();
    if members.is_empty()
        || !members
            .iter()
            .all(|b| b.is_ascii_graphic() && !b"\\/".contains(b))
    {
        return None;
    }
    if negated {
        for extra in *b"./" {
            if !members.contains(&extra) {
                members.push(extra);
            }
        }
    }
    // The ranges as the regular expression engine reads the class: an atom, `-` and an atom make a
    // range, which must not run backwards; any other atom stands for itself.
    let mut ranges = Vec::new();
    let mut rest = members.as_slice();
    while let Some(&low) = rest.first() {
        let (high, used) = match rest {
            [_, b'-', high, ..] => (*high, 3),
            _ => (low, 1),
        };
        if low > high {
            return None;
        }
        ranges.push((u16::from(low), u16::from(high)));
        rest = rest.get(used..).unwrap_or_default();
    }
    Some((Token::Class { negated, ranges }, end.checked_add(1)?))
}
