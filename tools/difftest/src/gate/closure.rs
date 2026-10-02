//! The files a Rust module's code reaches in its own crate, for a record whose tested paths name
//! a module rather than every file of its crate.
//!
//! A narrowing of the criteria may hold `rust:<path>` (a module: a directory with its `mod.rs`, or
//! a file) and `rust-leaf:<path>` (a file). `gate record` replaces them with files, read from the
//! tree it records: the module's file, and the file of every module of the crate its code names
//! by path (`crate::a::b`, `<the library's name>::a`, `super::c`, `self::d`, or a child module it
//! declares and names), transitively, each module by its deepest file; the files of
//! `include_str!` and `include_bytes!`; and each leaf, whose own paths are not followed. A leaf is
//! a file that names every command's types (the argument parser), and the crate's root files
//! (`lib.rs`, `main.rs`) are leaves always: following them would reach every module, though a
//! module that uses one of their items runs only that item. Code in a `#[cfg(test)]` module, in
//! comments and in strings names nothing.
//!
//! The record keeps the files, so a change to any of them (a new path among them, too) makes it
//! stale. What the paths do not show is not seen: an `impl` in a file that no path reaches, and a
//! leaf's item that starts to use a module the leaf alone names.

use std::collections::{BTreeSet, VecDeque};
use std::path::{Path, PathBuf};

use crate::errors::{Error, Result};

/// The prefix of a narrowing's entry that names a module.
pub const MODULE: &str = "rust:";

/// The prefix of a narrowing's entry that names a leaf.
pub const LEAF: &str = "rust-leaf:";

/// The files of the tree under `root` that `paths` name: each path as it is, a module's closure
/// for a `rust:` entry and the leaf's file for a `rust-leaf:` entry, relative to the root.
pub fn expand(root: &Path, paths: &[String]) -> Result<Vec<String>> {
    if !paths
        .iter()
        .any(|p| p.starts_with(MODULE) || p.starts_with(LEAF))
    {
        return Ok(paths.to_vec());
    }
    let root = std::fs::canonicalize(root).map_err(|e| Error::io(root, e))?;
    let root = root.as_path();
    let leaves: BTreeSet<PathBuf> = paths
        .iter()
        .filter_map(|p| p.strip_prefix(LEAF))
        .map(|p| root.join(p))
        .collect();
    let mut out = BTreeSet::new();
    for path in paths {
        if let Some(module) = path.strip_prefix(MODULE) {
            // A file outside the tree (an include from elsewhere) is no tested path.
            for file in closure(root, module, &leaves)? {
                out.extend(
                    file.strip_prefix(root)
                        .ok()
                        .map(|p| p.to_string_lossy().into_owned()),
                );
            }
        } else if let Some(leaf) = path.strip_prefix(LEAF) {
            out.insert(leaf.to_owned());
        } else {
            out.insert(path.clone());
        }
    }
    Ok(out.into_iter().collect())
}

/// The crate a module lives in: its `src` directory and the name its library is used by.
struct Crate {
    src: PathBuf,
    name: Option<String>,
}

impl Crate {
    /// The crate of the module at `path` (under `root`): the nearest `src` directory above it.
    fn of(root: &Path, path: &str) -> Result<Crate> {
        let full = root.join(path);
        let src = full
            .ancestors()
            .find(|a| a.file_name().is_some_and(|n| n == "src"))
            .ok_or_else(|| Error::Usage(format!("{MODULE}{path}: no crate's src directory")))?
            .to_path_buf();
        let manifest = src
            .parent()
            .map(|dir| dir.join("Cargo.toml"))
            .and_then(|m| std::fs::read_to_string(m).ok())
            .unwrap_or_default();
        Ok(Crate {
            name: library_name(&manifest),
            src,
        })
    }

    /// The file of the module `path` (its segments under the crate's root), when it has one.
    fn file(&self, path: &[String]) -> Option<PathBuf> {
        if path.is_empty() {
            return Some(self.src.join("lib.rs")).filter(|f| f.is_file());
        }
        let base = path.iter().fold(self.src.clone(), |dir, seg| dir.join(seg));
        [base.with_extension("rs"), base.join("mod.rs")]
            .into_iter()
            .find(|f| f.is_file())
    }

    /// The deepest module of `path` that has a file: the module of an item, or the item's own
    /// module when it is one.
    fn deepest(&self, path: &[String]) -> Option<PathBuf> {
        (0..=path.len()).rev().find_map(|n| self.file(&path[..n]))
    }

    /// The module path of `file`.
    fn module_of(&self, file: &Path) -> Vec<String> {
        let rel = file.strip_prefix(&self.src).unwrap_or(file);
        let mut segs: Vec<String> = rel
            .with_extension("")
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();
        if segs.last().is_some_and(|s| s == "mod") || segs == ["lib"] || segs == ["main"] {
            segs.pop();
        }
        segs
    }

    /// Whether `file` is a root file of the crate.
    fn is_root(&self, file: &Path) -> bool {
        file.parent() == Some(self.src.as_path())
            && file
                .file_name()
                .is_some_and(|n| n == "lib.rs" || n == "main.rs")
    }
}

/// The name a crate's library is used by: its `[lib]` name, else its package's with `-` as `_`.
fn library_name(manifest: &str) -> Option<String> {
    let mut table = "";
    let (mut lib, mut package) = (None, None);
    for line in manifest.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.starts_with('[') {
            table = line;
        } else if let Some((key, value)) = line.split_once('=')
            && key.trim() == "name"
        {
            let value = value.trim().trim_matches('"').replace('-', "_");
            match table {
                "[lib]" => lib = Some(value),
                "[package]" => package = Some(value),
                _ => {}
            }
        }
    }
    lib.or(package)
}

/// The closure of the module at `path` under `root`.
fn closure(root: &Path, path: &str, leaves: &BTreeSet<PathBuf>) -> Result<BTreeSet<PathBuf>> {
    let krate = Crate::of(root, path)?;
    let full = root.join(path);
    let start = if full.is_dir() {
        Some(full.join("mod.rs"))
            .filter(|f| f.is_file())
            .or_else(|| Some(full.with_extension("rs")).filter(|f| f.is_file()))
    } else {
        Some(full).filter(|f| f.is_file())
    }
    .ok_or_else(|| Error::Usage(format!("{MODULE}{path}: no such module")))?;
    let mut seen = BTreeSet::new();
    let mut data = BTreeSet::new();
    let mut work = VecDeque::from([start]);
    while let Some(file) = work.pop_front() {
        if !seen.insert(file.clone()) || leaves.contains(&file) || krate.is_root(&file) {
            continue;
        }
        let text = std::fs::read_to_string(&file).map_err(|e| Error::io(&file, e))?;
        data.extend(includes(&krate, &file, &without_tests(&text)));
        let code = without_tests(&code_only(&text));
        let module = krate.module_of(&file);
        for named in paths_in(&code, krate.name.as_deref()) {
            let base: Vec<String> = match named.head {
                Head::Crate => Vec::new(),
                Head::Current => module.clone(),
                Head::Up(n) => module[..module.len().saturating_sub(n)].to_vec(),
            };
            let full: Vec<String> = base.into_iter().chain(named.segments).collect();
            work.extend(krate.deepest(&full));
        }
        for child in children(&code) {
            if names(&code, &child) {
                let mut path = module.clone();
                path.push(child);
                work.extend(krate.file(&path));
            }
        }
    }
    seen.extend(data);
    Ok(seen)
}

/// Where a path starts.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Head {
    /// The crate's root (`crate::`, or the library's name).
    Crate,
    /// The current module (`self::`).
    Current,
    /// So many modules up (`super::`).
    Up(usize),
}

/// A path the code names.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Named {
    head: Head,
    segments: Vec<String>,
}

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The paths that `code` names from the crate's root, the module or a module above, with every
/// `{...}` group of a `use` spelt out.
fn paths_in(code: &str, library: Option<&str>) -> Vec<Named> {
    let chars: Vec<char> = code.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if !is_ident(chars[i]) || (i > 0 && (is_ident(chars[i - 1]) || chars[i - 1] == ':')) {
            i += 1;
            continue;
        }
        let end = chars[i..]
            .iter()
            .position(|c| !is_ident(*c))
            .map_or(chars.len(), |n| i + n);
        let word: String = chars[i..end].iter().collect();
        let head = match word.as_str() {
            "crate" => Some(Head::Crate),
            "self" => Some(Head::Current),
            "super" => Some(Head::Up(1)),
            w if Some(w) == library => Some(Head::Crate),
            _ => None,
        };
        let (Some(head), true) = (head, chars[end..].starts_with(&[':', ':'])) else {
            i = end;
            continue;
        };
        let (tails, next) = path_tail(&chars, end + 2);
        for tail in tails {
            let mut head = head.clone();
            let mut segments: Vec<String> = Vec::new();
            for seg in tail {
                match (&mut head, seg.as_str()) {
                    (Head::Up(n), "super") if segments.is_empty() => *n += 1,
                    (_, "self") | (_, "*") => {}
                    _ => segments.push(seg),
                }
            }
            out.push(Named { head, segments });
        }
        i = next.max(end);
    }
    out
}

/// The rest of a path from `at` (after a `::`): each path it spells out (a `{...}` group gives
/// several), as segments, and where it ends.
fn path_tail(chars: &[char], at: usize) -> (Vec<Vec<String>>, usize) {
    let mut i = at;
    while chars.get(i).is_some_and(|c| c.is_whitespace()) {
        i += 1;
    }
    if chars.get(i) == Some(&'{') {
        let mut out = Vec::new();
        let mut j = i + 1;
        loop {
            while chars.get(j).is_some_and(|c| c.is_whitespace() || *c == ',') {
                j += 1;
            }
            match chars.get(j) {
                None => return (out, j),
                Some('}') => return (out, j + 1),
                _ => {}
            }
            let (inner, next) = path_tail(chars, j);
            out.extend(inner);
            if next <= j {
                return (out, j + 1);
            }
            j = skip_rename(chars, next);
        }
    }
    if chars.get(i) == Some(&'*') {
        return (vec![vec!["*".to_owned()]], i + 1);
    }
    let end = chars[i..]
        .iter()
        .position(|c| !is_ident(*c))
        .map_or(chars.len(), |n| i + n);
    if end == i {
        return (vec![Vec::new()], i);
    }
    let segment: String = chars[i..end].iter().collect();
    // `name as other` ends the path.
    if chars[end..].starts_with(&[':', ':']) {
        let (tails, next) = path_tail(chars, end + 2);
        let spelt = tails
            .into_iter()
            .map(|mut t| {
                t.insert(0, segment.clone());
                t
            })
            .collect();
        return (spelt, next);
    }
    (vec![vec![segment]], end)
}

/// Where a `use` item's rename (` as name`) after `at` ends, or `at` without one.
fn skip_rename(chars: &[char], at: usize) -> usize {
    let mut i = at;
    while chars.get(i).is_some_and(|c| c.is_whitespace()) {
        i += 1;
    }
    let renamed =
        chars[i..].starts_with(&['a', 's']) && chars.get(i + 2).is_some_and(|c| c.is_whitespace());
    if !renamed {
        return at;
    }
    i += 2;
    while chars.get(i).is_some_and(|c| c.is_whitespace()) {
        i += 1;
    }
    while chars.get(i).is_some_and(|c| is_ident(*c)) {
        i += 1;
    }
    i
}

/// The child modules a file declares in files of their own (`mod name;`).
fn children(code: &str) -> Vec<String> {
    let mut out = Vec::new();
    for statement in code.split(';') {
        let words: Vec<&str> = statement.split_whitespace().collect();
        if let [.., "mod", name] = words.as_slice()
            && name.chars().all(is_ident)
        {
            out.push((*name).to_owned());
        }
    }
    out
}

/// Whether `code` names the child module `name` by a path (`name::...`).
fn names(code: &str, name: &str) -> bool {
    let pattern = format!("{name}::");
    code.match_indices(&pattern).any(|(at, _)| {
        code[..at]
            .chars()
            .next_back()
            .is_none_or(|c| !is_ident(c) && c != ':')
    })
}

/// The files `include_str!` and `include_bytes!` read: a literal path beside the file, or the
/// crate's directory (`CARGO_MANIFEST_DIR`) joined with the literals after it.
fn includes(krate: &Crate, file: &Path, text: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for mac in ["include_str!(", "include_bytes!("] {
        for (at, _) in text.match_indices(mac) {
            let rest = &text[at + mac.len()..];
            let Some(close) = rest.find(");").or_else(|| rest.find(')')) else {
                continue;
            };
            let call = &rest[..close];
            let literals: Vec<&str> = call.split('"').skip(1).step_by(2).collect();
            let path = if call.contains("CARGO_MANIFEST_DIR") {
                let dir = krate
                    .src
                    .parent()
                    .map(Path::to_path_buf)
                    .unwrap_or_default();
                let joined: String = literals
                    .iter()
                    .filter(|l| **l != "CARGO_MANIFEST_DIR")
                    .copied()
                    .collect();
                dir.join(joined.trim_start_matches('/'))
            } else if let [one] = literals.as_slice() {
                file.parent().unwrap_or(Path::new(".")).join(one)
            } else {
                continue;
            };
            if let Ok(found) = std::fs::canonicalize(&path) {
                out.push(found);
            }
        }
    }
    out
}

/// `text` with its comments, strings and character literals blanked (each replaced by spaces),
/// so that only code is left to read paths from.
fn code_only(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    let blank = |out: &mut String, from: &[char]| {
        out.extend(from.iter().map(|c| if *c == '\n' { '\n' } else { ' ' }));
    };
    while i < chars.len() {
        let rest = &chars[i..];
        if i > 0 && is_ident(chars[i - 1]) && is_ident(rest[0]) {
            // Inside a word (`for`, `br`): no literal starts here.
            out.push(rest[0]);
            i += 1;
        } else if rest.starts_with(&['/', '/']) {
            let end = rest.iter().position(|c| *c == '\n').unwrap_or(rest.len());
            blank(&mut out, &rest[..end]);
            i += end;
        } else if rest.starts_with(&['/', '*']) {
            let end = (2..rest.len())
                .find(|&k| rest[k - 1] == '*' && rest[k] == '/')
                .map_or(rest.len(), |k| k + 1);
            blank(&mut out, &rest[..end]);
            i += end;
        } else if rest[0] == '"' || (rest[0] == 'r' && matches!(rest.get(1), Some('"' | '#'))) {
            let end = string_end(rest);
            blank(&mut out, &rest[..end]);
            i += end;
        } else if rest[0] == '\'' {
            // A character literal ('x', '\n'), or a lifetime, which stays.
            let end = match rest.get(1) {
                Some('\\') => rest.iter().skip(3).position(|c| *c == '\'').map(|n| n + 4),
                Some(_) if rest.get(2) == Some(&'\'') => Some(3),
                _ => None,
            };
            match end {
                Some(end) => {
                    blank(&mut out, &rest[..end]);
                    i += end;
                }
                None => {
                    out.push('\'');
                    i += 1;
                }
            }
        } else {
            out.push(rest[0]);
            i += 1;
        }
    }
    out
}

/// Where a string literal that starts `rest` ends (just after it): `"..."` with escapes, or a raw
/// string `r#"..."#`.
fn string_end(rest: &[char]) -> usize {
    if rest[0] == 'r' {
        let hashes = rest[1..].iter().take_while(|c| **c == '#').count();
        let open = 1 + hashes;
        if rest.get(open) != Some(&'"') {
            return 1;
        }
        let mut k = open + 1;
        while k < rest.len() {
            if rest[k] == '"'
                && rest[k + 1..]
                    .iter()
                    .take(hashes)
                    .filter(|c| **c == '#')
                    .count()
                    == hashes
            {
                return k + 1 + hashes;
            }
            k += 1;
        }
        return rest.len();
    }
    let mut k = 1;
    while k < rest.len() {
        match rest[k] {
            '\\' => k += 2,
            '"' => return k + 1,
            _ => k += 1,
        }
    }
    rest.len()
}

/// `code` without its `#[cfg(test)]` modules.
fn without_tests(code: &str) -> String {
    let mut out = code.to_owned();
    while let Some(at) = out.find("#[cfg(test)]") {
        let after = &out[at..];
        let Some(open) = after.find('{') else {
            out.replace_range(at.., "");
            break;
        };
        // Only a module is cut: a test attribute on an item ends with that item's `;` or block.
        let head = &after[..open];
        let semicolon = head.find(';');
        if let Some(semi) = semicolon {
            out.replace_range(at..at + semi + 1, "");
            continue;
        }
        let mut depth = 0usize;
        let mut end = after.len();
        for (k, c) in after[open..].char_indices() {
            match c {
                '{' => depth += 1,
                '}' => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        end = open + k + 1;
                        break;
                    }
                }
                _ => {}
            }
        }
        out.replace_range(at..at + end, "");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A crate in a scratch tree: its files, relative to the tree's root.
    fn tree(files: &[(&str, &str)]) -> crate::store::ScratchDir {
        let dir = crate::store::ScratchDir::new("closure").expect("scratch");
        for (path, text) in files {
            let full = dir.path().join(path);
            std::fs::create_dir_all(full.parent().expect("parent")).expect("dirs");
            std::fs::write(full, text).expect("write");
        }
        dir
    }

    #[test]
    fn a_module_reaches_what_its_code_names_and_no_more() {
        let dir = tree(&[
            (
                "t/Cargo.toml",
                "[package]\nname = \"the-tool\"\n\n[lib]\npath = \"src/lib.rs\"\n",
            ),
            (
                "t/src/lib.rs",
                "pub mod a;\npub mod b;\npub mod c;\npub mod cli;\npub mod d;\npub mod e;\n",
            ),
            (
                "t/src/a/mod.rs",
                "mod inner;\nmod unused;\nuse crate::{b::thing, c};\n\
                 pub fn f() { inner::g(); crate::cli::Args::default(); }\n\
                 // crate::d::never in a comment\nconst S: &str = \"crate::d::never\";\n\
                 #[cfg(test)]\nmod tests {\n    use crate::e::only_in_tests;\n}\n",
            ),
            (
                "t/src/a/inner.rs",
                "use super::super::d::x;\nconst D: &str = include_str!(\"data.txt\");\n\
                 #[cfg(test)]\nmod tests {\n    const T: &str = include_str!(\"only-tests.txt\");\n}\n",
            ),
            ("t/src/a/data.txt", "data\n"),
            ("t/src/a/only-tests.txt", "data\n"),
            ("t/src/a/unused.rs", "use crate::e::y;\n"),
            ("t/src/b.rs", "pub fn thing() {}\n"),
            ("t/src/c.rs", "pub fn c() { the_tool::b::thing(); }\n"),
            (
                "t/src/cli.rs",
                "pub struct Args; use crate::e::all_commands;\n",
            ),
            ("t/src/d.rs", "pub fn x() {}\n"),
            ("t/src/e.rs", "pub fn y() {}\n"),
        ]);
        let files = expand(
            dir.path(),
            &[
                "rust:t/src/a".to_owned(),
                "rust-leaf:t/src/cli.rs".to_owned(),
                "data".to_owned(),
            ],
        )
        .expect("expand");
        assert_eq!(
            files,
            [
                "data",
                "t/src/a/data.txt",
                "t/src/a/inner.rs",
                "t/src/a/mod.rs",
                "t/src/b.rs",
                "t/src/c.rs",
                "t/src/cli.rs",
                "t/src/d.rs",
            ]
        );
    }

    #[test]
    fn a_group_of_a_use_is_spelt_out_and_super_counts_its_steps() {
        let code = "use crate::{a::{b, c::d}, e as f}; use super::super::g::h; self::i::j();";
        let named = paths_in(code, None);
        let spelt: Vec<(Head, Vec<&str>)> = named
            .iter()
            .map(|n| {
                (
                    n.head.clone(),
                    n.segments.iter().map(String::as_str).collect(),
                )
            })
            .collect();
        assert_eq!(
            spelt,
            [
                (Head::Crate, vec!["a", "b"]),
                (Head::Crate, vec!["a", "c", "d"]),
                (Head::Crate, vec!["e"]),
                (Head::Up(2), vec!["g", "h"]),
                (Head::Current, vec!["i", "j"]),
            ]
        );
    }

    #[test]
    fn strings_comments_and_test_modules_name_nothing() {
        let text = "let a = 'x'; let b = \"crate::q\"; let c = r#\"crate::r\"#; // crate::s\n\
                    /* crate::t */ fn f<'a>(x: &'a str) {}\n#[cfg(test)]\nmod tests { use crate::u; }\n";
        let code = without_tests(&code_only(text));
        assert!(paths_in(&code, None).is_empty(), "{code}");
        assert!(code.contains("fn f<'a>"), "{code}");
    }
}
