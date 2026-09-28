//! The reference's catch-all route, the documentation's (`GET /{param*}` with inert's directory
//! handler), for a `GET` or `HEAD` that no route matches.
//!
//! hapi routes the path as it normalises it (the escapes of unreserved characters decoded, the
//! others in upper case, dot segments resolved, a trailing slash stripped), and the route's
//! parameter is that whole path without its leading `/`, decoded: a path that does not decode is
//! hapi's 400, before the route's lifecycle (so not charged). The handler serves the documentation
//! for a path of fewer than 20 characters, and inert then refuses (403) a selection that leads
//! above the documentation's directory. The relay does not build the documentation, so every
//! other path is 404.

use super::percent_decode;

/// What the catch-all route answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CatchAll {
    /// The path does not decode: hapi's 400, uncharged.
    Undecodable,
    /// inert's 403: the selection leads above the documentation's directory.
    Forbidden,
    /// 404.
    NotFound,
}

/// The reference's documentation directory, `<install>/packages/api/dist/www`, as far as a path
/// of fewer than 20 characters can climb.
const DIRECTORY: [&str; 4] = ["packages", "api", "dist", "www"];

/// The catch-all route's answer to a `GET` of `path` (as the request gives it).
pub(super) fn answer(path: &str) -> CatchAll {
    let path = hapi_path(path);
    let Some(selection) = percent_decode(path.get(1..).unwrap_or_default()) else {
        return CatchAll::Undecodable;
    };
    let served = path.len() < 20 && !selection.starts_with('/') && !hidden(&selection);
    match served && above(&selection) {
        true => CatchAll::Forbidden,
        false => CatchAll::NotFound,
    }
}

/// `path` as hapi's router normalises it, and without a trailing slash.
fn hapi_path(path: &str) -> String {
    let bytes = path.as_bytes();
    let mut out = String::with_capacity(path.len());
    let mut at = 0;
    while let Some(&byte) = bytes.get(at) {
        let hex = bytes.get(at + 1..at + 3).filter(|_| byte == b'%');
        let hex = hex.filter(|hex| hex.iter().all(u8::is_ascii_hexdigit));
        let Some(hex) = hex.and_then(|hex| std::str::from_utf8(hex).ok()) else {
            out.push(char::from(byte));
            at += 1;
            continue;
        };
        let value = u8::from_str_radix(hex, 16).unwrap_or_default();
        match value.is_ascii_alphanumeric() || b"!$&'()*+,-.:;=@_~".contains(&value) {
            true => out.push(char::from(value)),
            false => out.push_str(&format!("%{}", hex.to_ascii_uppercase())),
        }
        at += 3;
    }
    if out.contains("/.") || out.starts_with('.') {
        out = dot_segments(&out);
    }
    if out.len() > 1 && out.ends_with('/') {
        out.pop();
    }
    out
}

/// hapi's resolution of the dot segments of `path`.
fn dot_segments(path: &str) -> String {
    let mut kept: Vec<&str> = Vec::new();
    let mut last = "";
    for segment in path.split('/') {
        match segment {
            ".." => {
                kept.pop();
            }
            "." => {}
            _ => kept.push(segment),
        }
        last = segment;
    }
    if matches!(last, "." | "..") {
        kept.push("");
    }
    let joined = kept.join("/");
    match path.starts_with('/') && !joined.starts_with('/') {
        true => format!("/{joined}"),
        false => joined,
    }
}

/// inert's hidden selection: a `.` that starts it or follows a `/` or `\`, followed by a
/// character other than `.`, `/` and `\`, or by `.` and a character other than `/` and `\`.
fn hidden(selection: &str) -> bool {
    let chars: Vec<char> = selection.chars().collect();
    let separator = |c: char| c == '/' || c == '\\';
    chars.iter().enumerate().any(|(at, &c)| {
        let starts = at == 0 || chars.get(at - 1).is_some_and(|&c| separator(c));
        let next = chars.get(at + 1).copied();
        let after = chars.get(at + 2).copied();
        c == '.'
            && starts
            && match next {
                Some('.') => after.is_some_and(|c| !separator(c)),
                Some(c) => !separator(c),
                None => false,
            }
    })
}

/// Whether the path Node's `path.join` makes of the documentation's directory and `selection`
/// does not start with that directory's path: the selection climbs above it, and does not lead
/// back to a name that the directory's own path starts.
fn above(selection: &str) -> bool {
    let mut climbed = 0;
    let mut kept: Vec<&str> = Vec::new();
    for segment in selection.split('/') {
        match segment {
            "" | "." => {}
            ".." if kept.pop().is_none() => climbed += 1,
            ".." => {}
            _ => kept.push(segment),
        }
    }
    let tail = DIRECTORY.len().checked_sub(climbed);
    let tail = tail.and_then(|at| DIRECTORY.get(at..));
    match (climbed, tail) {
        (0, _) => false,
        (_, None) => true,
        (_, Some(tail)) => !kept.join("/").starts_with(&tail.join("/")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_catch_all_answers_as_hapi_and_inert_answer() {
        // Measured on hapi 20.2.1 and inert 6.0.5 under Node 18.20.8.
        let cases = [
            ("/x%zz", CatchAll::Undecodable),
            ("/api/nothing%zz", CatchAll::Undecodable),
            ("/%C3", CatchAll::Undecodable),
            ("/%ED%A0%80", CatchAll::Undecodable),
            ("/%", CatchAll::Undecodable),
            ("/a%2", CatchAll::Undecodable),
            ("/api/nothing", CatchAll::NotFound),
            ("/%C3%A9", CatchAll::NotFound),
            ("/%00", CatchAll::NotFound),
            ("/..%2Fx", CatchAll::Forbidden),
            ("/..%2F", CatchAll::Forbidden),
            ("/..%2f..%2fx", CatchAll::Forbidden),
            ("/%2E%2E%2Fx", CatchAll::Forbidden),
            ("/.%2E%2Fx", CatchAll::Forbidden),
            ("/x%2F..%2F..%2Fy", CatchAll::Forbidden),
            ("/a/..%2F..%2Fx", CatchAll::Forbidden),
            ("/abcdefghi/..%2F..", CatchAll::Forbidden),
            ("/..%2Fx/", CatchAll::Forbidden),
            ("/a%zz/..", CatchAll::NotFound),
            // Twenty characters or more: the handler's own 404.
            ("/..%2Fxxxxxxxxxxxxxxxxxxxxxxxxxx", CatchAll::NotFound),
            ("/abcdefghijk..%2Fx", CatchAll::NotFound),
            // A dot segment removes the segment that does not decode.
            ("/%zz/../x", CatchAll::NotFound),
            ("/%zz/%2E%2E/x", CatchAll::NotFound),
            // A backslash is no separator; a hidden or absolute selection is not found.
            ("/..%5Cx", CatchAll::NotFound),
            ("/..%2F.x", CatchAll::NotFound),
            ("/%2F..%2Fx", CatchAll::NotFound),
            // Back into the directory, or to a name that its path starts.
            ("/..%2Fwww", CatchAll::NotFound),
            ("/..%2Fwww2", CatchAll::NotFound),
            ("/..%2F..%2Fdist/www", CatchAll::NotFound),
        ];
        for (path, answer) in cases {
            assert_eq!(super::answer(path), answer, "{path}");
        }
    }

    #[test]
    fn a_path_is_normalised_as_hapi_normalises_it() {
        for (path, normal) in [
            ("/", "/"),
            ("/a/", "/a"),
            ("/%7e%2f%41%2e", "/~%2FA."),
            ("/a/./b/../c", "/a/c"),
            ("/a/..", "/"),
            ("/..", "/"),
            ("/a/b/.", "/a/b"),
            ("/%2E%2E/x", "/x"),
            ("/%%41", "/%A"),
        ] {
            assert_eq!(hapi_path(path), normal, "{path}");
        }
        assert!(hidden(".x") && hidden("a/..b") && hidden("...") && hidden("a\\.b"));
        assert!(!hidden("..") && !hidden("../x") && !hidden(".") && !hidden("a."));
    }
}
