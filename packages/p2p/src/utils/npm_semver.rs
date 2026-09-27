//! The subset of npm's `semver` (7.5.4, not loose) that the version gate and the discovery log
//! need: `valid`, `satisfies` with and without prereleases, and the comparison of `gt` and `lt`.
//!
//! Versions are read exactly as `semver` reads them: at most 256 characters, white space around
//! them ignored, an optional `v`, three numbers without leading zeros and at most 2^53 − 1, then
//! the optional prerelease and build identifiers. Prerelease identifiers compare numerically when
//! both are digits (as JavaScript numbers), digits before others, others by their text.
//!
//! Ranges are sets joined by `||`, each a list of comparators separated by white space, where an
//! operator standing alone (`>=`, `~`, `^` and the others) joins the word after it. A comparator
//! is a version or partial version (`1`, `1.2`, `1.x`, `*`) after an optional `<`, `<=`, `>`,
//! `>=` or `=`, a caret or a tilde, with the prefix `v` or `=` characters `semver` allows; they
//! desugar as `semver` desugars them. Other range syntax (hyphen ranges, and operators or stars
//! inside a word) is outside the subset, and such a range satisfies nothing.

use std::cmp::Ordering;

/// The largest integer JavaScript holds exactly; a version number above it is refused.
const MAX_SAFE: u64 = (1 << 53) - 1;

/// A version: the three numbers and the prerelease identifiers.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Version {
    main: [u64; 3],
    pre: Vec<String>,
}

/// A comparator: its operator (`""` for equality) and version.
type Comparator = (&'static str, Version);

/// `semver.valid(text)`: the version in its normal form, or `None` when `text` is not valid
/// semver.
pub fn valid(text: &str) -> Option<String> {
    parse(text).map(|version| format(&version))
}

/// `semver.satisfies(version, range, { includePrerelease })`.
pub fn satisfies(version: &str, range: &str, include_prerelease: bool) -> bool {
    let (Some(version), Some(sets)) = (parse(version), parse_range(range, include_prerelease))
    else {
        return false;
    };
    let null = |set: &&Vec<Comparator>| set.iter().any(|(op, c)| *op == "<" && is_null(c));
    let live: Vec<&Vec<Comparator>> = sets.iter().filter(|set| !null(set)).collect();
    // With several sets, a set that allows any version stands alone once the null sets are gone.
    if sets.len() > 1 && live.len() > 1 && live.iter().any(|set| set.is_empty()) {
        return version.pre.is_empty() || include_prerelease;
    }
    // A prerelease passes only a set with a prerelease comparator of its own numbers.
    let same_pre = |(_, c): &Comparator| !c.pre.is_empty() && c.main == version.main;
    let prerelease_allowed = |set: &Vec<Comparator>| set.iter().any(same_pre);
    sets.iter().any(|set| {
        set.iter().all(|(op, c)| holds(op, order(&version, c)))
            && (version.pre.is_empty() || include_prerelease || prerelease_allowed(set))
    })
}

/// The order of two valid versions, as `semver.gt` and `semver.lt` compare them; `None` when
/// either is not valid.
pub fn compare(a: &str, b: &str) -> Option<Ordering> {
    Some(order(&parse(a)?, &parse(b)?))
}

/// `new SemVer(text)`.
fn parse(text: &str) -> Option<Version> {
    if text.encode_utf16().count() > 256 {
        return None;
    }
    let text = text.trim_matches(is_js_space);
    let text = text.strip_prefix('v').unwrap_or(text);
    let (text, build) = cut(text, '+');
    let (main, pre) = cut(text, '-');
    let main: Vec<u64> = main.split('.').map(number).collect::<Option<_>>()?;
    let pre = pre.map_or(Some(Vec::new()), identifiers)?;
    build.is_none_or(is_build).then_some(())?;
    version(main.try_into().ok()?, pre)
}

/// A version of `main` and `pre`, when its numbers are at most 2^53 − 1 and its text at most 256
/// characters.
fn version(main: [u64; 3], pre: Vec<String>) -> Option<Version> {
    let version = Version { main, pre };
    (main.iter().all(|n| *n <= MAX_SAFE) && format(&version).len() <= 256).then_some(version)
}

fn format(version: &Version) -> String {
    let [major, minor, patch] = version.main;
    let mut text = format!("{major}.{minor}.{patch}");
    if !version.pre.is_empty() {
        text = format!("{text}-{}", version.pre.join("."));
    }
    text
}

/// `0` or digits without a leading zero, saturating (the caller refuses large values).
fn number(text: &str) -> Option<u64> {
    let digits = !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit());
    (digits && (text == "0" || !text.starts_with('0'))).then(|| text.parse().unwrap_or(u64::MAX))
}

/// Dot-separated prerelease identifiers: letters, digits and `-`, and digits without a leading
/// zero when they are all digits.
fn identifiers(text: &str) -> Option<Vec<String>> {
    (text.split('.'))
        .map(|id| {
            let allowed =
                !id.is_empty() && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-');
            let numeric = id.bytes().all(|b| b.is_ascii_digit());
            (allowed && (!numeric || number(id).is_some())).then(|| id.to_owned())
        })
        .collect()
}

fn order(a: &Version, b: &Version) -> Ordering {
    let pre = || match (a.pre.is_empty(), b.pre.is_empty()) {
        (true, true) => Ordering::Equal,
        (true, false) => Ordering::Greater,
        (false, true) => Ordering::Less,
        (false, false) => {
            for i in 0..=a.pre.len().max(b.pre.len()) {
                match (a.pre.get(i), b.pre.get(i)) {
                    (Some(x), Some(y)) if x == y => {}
                    (Some(x), Some(y)) => return identifier_order(x, y),
                    (x, y) => return x.is_some().cmp(&y.is_some()),
                }
            }
            Ordering::Equal
        }
    };
    a.main.cmp(&b.main).then_with(pre)
}

/// `compareIdentifiers`: numerically (as JavaScript numbers) when both are digits, digits first,
/// else by their text.
fn identifier_order(a: &str, b: &str) -> Ordering {
    let value = |id: &str| {
        id.parse::<f64>()
            .ok()
            .filter(|_| id.bytes().all(|b| b.is_ascii_digit()))
    };
    match (value(a), value(b)) {
        (Some(x), Some(y)) => x.total_cmp(&y),
        (x, y) => y.is_some().cmp(&x.is_some()).then_with(|| a.cmp(b)),
    }
}

fn holds(op: &str, ordering: Ordering) -> bool {
    match op {
        "<" => ordering.is_lt(),
        "<=" => ordering.is_le(),
        ">" => ordering.is_gt(),
        ">=" => ordering.is_ge(),
        _ => ordering.is_eq(),
    }
}

/// `<0.0.0-0`'s version, which no version is below.
fn is_null(version: &Version) -> bool {
    version.main == [0, 0, 0] && version.pre == ["0"]
}

/// The sets of `range`, each without the comparators that allow any version; `None` for a range
/// outside the subset or with an invalid comparator.
fn parse_range(range: &str, include_prerelease: bool) -> Option<Vec<Vec<Comparator>>> {
    let words: Vec<&str> = range.split(is_js_space).filter(|w| !w.is_empty()).collect();
    let normalised = words.join(" ");
    let sets = normalised.split("||").map(|set| {
        // An operator standing alone joins the word after it, as `semver`'s trims do in their
        // order: `<`, `>`, `=` and their pairs when a version follows, then `~`, `~>` and `^`.
        let words = set.split(' ').filter(|w| !w.is_empty()).map(str::to_owned);
        let version_next = |next: &str| {
            let next = next.trim_start_matches(['v', '=']);
            next.starts_with(|c: char| c.is_ascii_digit() || "xX*".contains(c))
        };
        let words = join(words, |word, next| {
            matches!(word, "<" | ">" | "<=" | ">=" | "=") && version_next(next)
        });
        let tokens = join(words, |word, _| matches!(word, "~" | "~>" | "^"));
        (tokens.iter()).try_fold(Vec::new(), |mut comparators, token| {
            comparators.extend(desugar(token, include_prerelease)?);
            Some(comparators)
        })
    });
    sets.collect()
}

/// `words` with each word for which `joins` holds (given the word after it) joined to that word.
fn join(
    words: impl IntoIterator<Item = String>,
    joins: impl Fn(&str, &str) -> bool,
) -> Vec<String> {
    let mut joined = Vec::new();
    let mut words = words.into_iter().peekable();
    while let Some(word) = words.next() {
        match words.next_if(|next| joins(&word, next)) {
            Some(next) => joined.push(format!("{word}{next}")),
            None => joined.push(word),
        }
    }
    joined
}

/// A partial version (`[v=]*` and one to three numbers, `x`, `X` or `*`, the prerelease and build
/// only after the third), the whole of `text`: each number or `None`, and the prerelease.
fn partial(text: &str) -> Option<([Option<u64>; 3], Vec<String>)> {
    let text = text.trim_start_matches(['v', '=']);
    let (text, build) = cut(text, '+');
    let (text, pre) = cut(text, '-');
    let ids: Vec<&str> = text.split('.').collect();
    let later = pre.is_some() || build.is_some();
    if ids.len() > 3 || (later && ids.len() < 3) || build.is_some_and(|b| !is_build(b)) {
        return None;
    }
    let mut numbers = [None; 3];
    for (slot, id) in numbers.iter_mut().zip(ids) {
        if !matches!(id, "x" | "X" | "*") {
            *slot = Some(number(id)?);
        }
    }
    Some((numbers, pre.map_or(Some(Vec::new()), identifiers)?))
}

/// The comparators a token desugars to (`^`, `~`, an operator and a partial version), without
/// those that allow any version.
fn desugar(token: &str, include_prerelease: bool) -> Option<Vec<Comparator>> {
    let zero = || vec!["0".to_owned(); usize::from(include_prerelease)];
    let with = |op: &'static str, main, pre| Some((op, version(main, pre)?));
    let below = |main| with("<", main, vec!["0".to_owned()]);
    let next = |n: u64| n.checked_add(1);
    // `>=0.0.0` (`>=0.0.0-0` with prereleases) allows any version.
    let floor = format!("0.0.0{}", if include_prerelease { "-0" } else { "" });
    let keep = |comparators: Vec<Comparator>| {
        let any = |(op, c): &Comparator| *op == ">=" && format(c) == floor;
        Some(comparators.into_iter().filter(|c| !any(c)).collect())
    };
    let (op, rest) = ["^", "~>", "~", "<=", ">=", "<", ">", "="]
        .into_iter()
        .find_map(|op| Some((op, token.strip_prefix(op)?)))
        .unwrap_or(("", token));
    let ([major, minor, patch], pre) = partial(rest)?;
    let Some(major) = major else {
        // `<*` and `>*` allow no version, the others any.
        let none = matches!(op, "<" | ">").then(|| below([0, 0, 0])).flatten();
        return Some(none.into_iter().collect());
    };
    let (x_minor, x_patch) = (minor.is_none(), minor.is_none() || patch.is_none());
    // An `x` zeroes the numbers after it.
    let (minor, patch) = (minor.unwrap_or(0), patch.filter(|_| !x_minor).unwrap_or(0));
    // The next minor version, or the next major one when the minor is an `x`.
    let up = match x_minor {
        true => next(major).map(|m| [m, 0, 0]),
        false => next(minor).map(|m| [major, m, 0]),
    };
    match op {
        "^" => {
            let floor_low = x_patch || (pre.is_empty() && major == 0);
            let low = if floor_low { zero() } else { pre };
            let high = match (major, minor) {
                (0, 0) if !x_patch => [0, 0, next(patch)?],
                (0, _) if !x_minor => up?,
                _ => [next(major)?, 0, 0],
            };
            keep(vec![with(">=", [major, minor, patch], low)?, below(high)?])
        }
        "~" | "~>" => {
            let low = if x_patch { Vec::new() } else { pre };
            keep(vec![with(">=", [major, minor, patch], low)?, below(up?)?])
        }
        // No `x`: the token stays as it is and is read as a comparator.
        _ if !x_patch => {
            let comparator = (if op == "=" { "" } else { op }, parse(rest)?);
            let any = op == ">=" && rest == floor;
            Some(if any { Vec::new() } else { vec![comparator] })
        }
        "" | "=" => keep(vec![with(">=", [major, minor, 0], zero())?, below(up?)?]),
        ">" | ">=" => {
            let low = if op == ">" { up? } else { [major, minor, 0] };
            keep(vec![with(">=", low, zero())?])
        }
        _ => {
            let high = if op == "<=" { up? } else { [major, minor, 0] };
            keep(vec![below(high)?])
        }
    }
}

/// `text` before and after the first `separator`, if there is one.
fn cut(text: &str, separator: char) -> (&str, Option<&str>) {
    text.split_once(separator)
        .map_or((text, None), |(before, after)| (before, Some(after)))
}

/// Build identifiers: letters, digits and `-`, separated by dots.
fn is_build(text: &str) -> bool {
    (text.split('.'))
        .all(|id| !id.is_empty() && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'))
}

/// JavaScript's white space and line terminators: Unicode's white space without U+0085, and
/// U+FEFF.
fn is_js_space(c: char) -> bool {
    (c.is_whitespace() && c != '\u{85}') || c == '\u{FEFF}'
}

#[cfg(test)]
mod tests;
