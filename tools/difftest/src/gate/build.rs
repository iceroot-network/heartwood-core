//! The build inputs of a record's tested paths, which the path rule holds as well as the paths
//! themselves: of each tested crate's manifest (and the fuzz targets' manifest) the tables that
//! reach its build, the fuzz targets' lockfile, and of the workspace's own manifest and lockfile
//! only what reaches the tested crates' build: the locked versions, sources, checksums and
//! dependencies of every package the tested crates depend on, the workspace's package settings,
//! profiles, lints and patches, and the entries of the workspace's dependency table that those
//! packages take. A dependency added to a crate that is not tested, a test target, or a comment
//! changes nothing; a version bump of a library a tested crate uses does.

use std::collections::{BTreeMap, BTreeSet};

/// The workspace's manifest and lockfile, at the tree's root.
pub(super) const WORKSPACE: [&str; 2] = ["Cargo.toml", "Cargo.lock"];

/// The lockfile of the fuzz targets, a build input of every path under `fuzz`.
pub(super) const FUZZ_LOCK: &str = "fuzz/Cargo.lock";

/// The directories whose manifests build what `paths` test: the workspace crates' (every crate
/// under `packages` for the whole directory, `tools/difftest` for the harness) and the fuzz
/// targets' (`fuzz`), in order. `packages` lists the directories under `packages`.
pub(super) fn crate_dirs(paths: &[String], packages: &[String]) -> Vec<String> {
    let mut out = BTreeSet::new();
    for path in paths {
        let parts: Vec<&str> = path.split('/').collect();
        match parts.as_slice() {
            ["packages"] => out.extend(packages.iter().map(|p| format!("packages/{p}"))),
            ["packages", krate, ..] if !krate.is_empty() => {
                out.insert(format!("packages/{krate}"));
            }
            ["tools", "difftest", ..] | ["tools"] => {
                out.insert("tools/difftest".to_owned());
            }
            ["fuzz", ..] => {
                out.insert("fuzz".to_owned());
            }
            _ => {}
        }
    }
    out.into_iter().collect()
}

/// A manifest's tables in order, each its header (`[profile.release]`, `[[bin]] 2` for the second
/// `[[bin]]`; the empty header for what comes before the first table) and its statements: each
/// `key = value` on one line, a value spread over several lines (an array, an inline table)
/// joined, comments and blank lines left out.
fn tables(manifest: &str) -> Vec<(String, Vec<String>)> {
    let mut out: Vec<(String, Vec<String>)> = vec![(String::new(), Vec::new())];
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut pending = String::new();
    for line in manifest.lines().map(strip_comment) {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if pending.is_empty() && line.starts_with('[') {
            let header = if line.starts_with("[[") {
                let n = counts.entry(line.to_owned()).or_insert(0);
                *n += 1;
                format!("{line} {n}")
            } else {
                line.to_owned()
            };
            out.push((header, Vec::new()));
            continue;
        }
        if !pending.is_empty() {
            pending.push(' ');
        }
        pending.push_str(line);
        if balanced(&pending) {
            let statement = std::mem::take(&mut pending);
            if let Some((_, statements)) = out.last_mut() {
                statements.push(statement);
            }
        }
    }
    if !pending.is_empty()
        && let Some((_, statements)) = out.last_mut()
    {
        statements.push(pending);
    }
    out.retain(|(header, statements)| !header.is_empty() || !statements.is_empty());
    out
}

/// Whether every bracket and brace a statement opens outside a string is closed.
fn balanced(text: &str) -> bool {
    let mut depth = 0i64;
    let mut quoted = false;
    for c in text.chars() {
        match c {
            '"' => quoted = !quoted,
            '[' | '{' if !quoted => depth += 1,
            ']' | '}' if !quoted => depth -= 1,
            _ => {}
        }
    }
    depth <= 0
}

/// The key of a statement, unquoted.
fn key_of(statement: &str) -> &str {
    statement
        .split_once('=')
        .map_or(statement, |(key, _)| key)
        .trim()
        .trim_matches('"')
}

/// The tables of a crate's manifest that reach its build, by header: every table but those of
/// its tests, benchmarks, examples and dev-dependencies.
fn build_tables(manifest: &str) -> BTreeMap<String, Vec<String>> {
    tables(manifest)
        .into_iter()
        .filter(|(header, _)| {
            let left_out = ["[[test]]", "[[bench]]", "[[example]]", "[dev-dependencies"]
                .iter()
                .any(|start| header.starts_with(start))
                || header.contains(".dev-dependencies");
            !left_out
        })
        .collect()
}

/// Whether a crate's manifest changed in what reaches its build (a manifest that appeared or went
/// away did).
pub(super) fn manifest_changed(before: Option<&str>, after: Option<&str>) -> bool {
    match (before, after) {
        (Some(before), Some(after)) => build_tables(before) != build_tables(after),
        (None, None) => false,
        _ => true,
    }
}

/// The `name` of a manifest's `[package]` table.
pub(super) fn package_name(manifest: &str) -> Option<String> {
    tables(manifest)
        .into_iter()
        .find(|(header, _)| header == "[package]")
        .and_then(|(_, statements)| {
            statements
                .iter()
                .find_map(|statement| assigned(statement, "name"))
        })
}

/// The member directories a workspace's manifest names (`members`), its globs as written: a
/// pattern ending in `/*` stands for every directory under its stem.
pub(super) fn members(manifest: &str) -> Vec<String> {
    let Some((_, statements)) = tables(manifest)
        .into_iter()
        .find(|(header, _)| header == "[workspace]")
    else {
        return Vec::new();
    };
    statements
        .iter()
        .filter(|statement| key_of(statement) == "members")
        .flat_map(|statement| {
            let value = statement.split_once('=').map_or("", |(_, v)| v);
            value
                .split(['[', ']', ','])
                .map(|item| item.trim().trim_matches('"').to_owned())
                .filter(|item| !item.is_empty())
                .collect::<Vec<_>>()
        })
        .collect()
}

/// A line without its comment (a `#` outside a string).
fn strip_comment(line: &str) -> &str {
    let mut quoted = false;
    for (i, c) in line.char_indices() {
        match c {
            '"' => quoted = !quoted,
            '#' if !quoted => return &line[..i],
            _ => {}
        }
    }
    line
}

/// The string value of `key = "value"` on `line`.
fn assigned(line: &str, key: &str) -> Option<String> {
    let (k, v) = line.split_once('=')?;
    (k.trim() == key).then(|| v.trim().trim_matches('"').to_owned())
}

/// One package of a lockfile.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Locked {
    name: String,
    version: String,
    source: String,
    checksum: String,
    /// Its dependencies as the lockfile names them: `name` or `name version`.
    dependencies: Vec<String>,
}

/// The packages of a lockfile, in its order.
fn lock_packages(lock: &str) -> Vec<Locked> {
    let mut out: Vec<Locked> = Vec::new();
    let mut in_dependencies = false;
    for line in lock.lines() {
        let line = line.trim();
        if line == "[[package]]" {
            out.push(Locked::default());
            in_dependencies = false;
            continue;
        }
        let Some(package) = out.last_mut() else {
            continue;
        };
        if in_dependencies {
            if line.starts_with(']') {
                in_dependencies = false;
            } else {
                let name = line.trim_end_matches(',').trim_matches('"');
                package.dependencies.push(name.to_owned());
            }
            continue;
        }
        if line.starts_with("dependencies = [") {
            in_dependencies = !line.ends_with(']');
            continue;
        }
        for (key, slot) in [
            ("name", &mut package.name),
            ("version", &mut package.version),
            ("source", &mut package.source),
            ("checksum", &mut package.checksum),
        ] {
            if let Some(value) = assigned(line, key) {
                *slot = value;
            }
        }
    }
    out
}

/// The packages `roots` depend on, the roots too, by `name version`, as the lockfile resolves
/// their dependencies.
fn closure(lock: &[Locked], roots: &BTreeSet<String>) -> BTreeMap<String, Locked> {
    let mut out = BTreeMap::new();
    let mut work: Vec<&Locked> = lock.iter().filter(|p| roots.contains(&p.name)).collect();
    while let Some(package) = work.pop() {
        let key = format!("{} {}", package.name, package.version);
        if out.insert(key, package.clone()).is_some() {
            continue;
        }
        for dependency in &package.dependencies {
            let mut words = dependency.split_whitespace();
            let name = words.next().unwrap_or_default();
            let version = words.next();
            work.extend(
                lock.iter()
                    .filter(|p| p.name == name && version.is_none_or(|v| v == p.version)),
            );
        }
    }
    out
}

/// How a table of the workspace's manifest reaches the crates' build.
enum Reach {
    /// Every crate's build (package settings, profiles, lints, patches, and any table the rule
    /// does not know, which counts whole).
    Shared,
    /// The workspace's dependency table: an entry reaches the crates that take it.
    Dependencies,
    /// One entry of the dependency table, written as a table of its own.
    Dependency(String),
    /// Nothing that is built (`[workspace.metadata]`, the tools' own settings).
    None,
}

/// How the table `header` of the workspace's manifest reaches the build.
fn reach_of(header: &str) -> Reach {
    let name = header.trim_start_matches('[').trim_end_matches(']').trim();
    if name == "workspace.dependencies" {
        Reach::Dependencies
    } else if let Some(dependency) = name.strip_prefix("workspace.dependencies.") {
        Reach::Dependency(dependency.trim_matches('"').to_owned())
    } else if name == "workspace.metadata" || name.starts_with("workspace.metadata.") {
        Reach::None
    } else {
        Reach::Shared
    }
}

/// The keys of the `[workspace]` table that name the members, which change with the tree's
/// crates and not with any crate's build.
const MEMBERSHIP: [&str; 3] = ["members", "exclude", "default-members"];

/// What of the workspace's manifest reaches every crate's build, by header, and the entries of its
/// dependency table, by name.
#[derive(Debug, Default, PartialEq, Eq)]
struct WorkspaceManifest {
    shared: BTreeMap<String, Vec<String>>,
    dependencies: BTreeMap<String, Vec<String>>,
}

fn workspace_manifest(manifest: &str) -> WorkspaceManifest {
    let mut out = WorkspaceManifest::default();
    for (header, statements) in tables(manifest) {
        match reach_of(&header) {
            Reach::Shared => {
                let kept: Vec<String> = statements
                    .into_iter()
                    .filter(|s| header != "[workspace]" || !MEMBERSHIP.contains(&key_of(s)))
                    .collect();
                if !kept.is_empty() {
                    out.shared.insert(header, kept);
                }
            }
            Reach::Dependencies => {
                for statement in statements {
                    let key = key_of(&statement);
                    let name = key.split('.').next().unwrap_or(key).trim_matches('"');
                    out.dependencies
                        .entry(name.to_owned())
                        .or_default()
                        .push(statement);
                }
            }
            Reach::Dependency(name) => {
                out.dependencies.entry(name).or_default().extend(statements);
            }
            Reach::None => {}
        }
    }
    out
}

/// The workspace's manifest and lockfile at two commits.
pub(super) struct Workspace<'a> {
    /// The manifest and lockfile at the record's head.
    pub before: (&'a str, &'a str),
    /// The manifest and lockfile at the final head.
    pub after: (&'a str, &'a str),
}

/// What of the workspace's manifest and lockfile reaches the build of the crates named `roots`
/// (their package names) and changed between the two commits, each as a short phrase, and the
/// names of the packages of the tree (no registry source) the crates are built with at either
/// commit, whose own manifests are build inputs too.
pub(super) fn workspace_changes(
    ws: &Workspace<'_>,
    roots: &BTreeSet<String>,
) -> (Vec<String>, BTreeSet<String>) {
    let mut out = Vec::new();
    let (before, after) = (
        closure(&lock_packages(ws.before.1), roots),
        closure(&lock_packages(ws.after.1), roots),
    );
    let versions = |c: &BTreeMap<String, Locked>, name: &str| -> Vec<String> {
        c.values()
            .filter(|p| p.name == name)
            .map(|p| p.version.clone())
            .collect()
    };
    let every: BTreeSet<String> = before
        .values()
        .chain(after.values())
        .map(|p| p.name.clone())
        .collect();
    for name in &every {
        let (was, now) = (versions(&before, name), versions(&after, name));
        match (was.as_slice(), now.as_slice()) {
            ([one], [other]) if one != other => {
                out.push(format!("Cargo.lock: {name} {one} -> {other}"));
            }
            _ => {
                for gone in was.iter().filter(|v| !now.contains(v)) {
                    out.push(format!("Cargo.lock: {name} {gone} removed"));
                }
                for new in now.iter().filter(|v| !was.contains(v)) {
                    out.push(format!("Cargo.lock: {name} {new} added"));
                }
            }
        }
        for version in was.iter().filter(|v| now.contains(v)) {
            let key = format!("{name} {version}");
            if before.get(&key) != after.get(&key) {
                out.push(format!("Cargo.lock: {name} {version} changed"));
            }
        }
    }
    let (manifest_before, manifest_after) = (
        workspace_manifest(ws.before.0),
        workspace_manifest(ws.after.0),
    );
    let headers: BTreeSet<&String> = manifest_before
        .shared
        .keys()
        .chain(manifest_after.shared.keys())
        .collect();
    for header in headers {
        if manifest_before.shared.get(header) != manifest_after.shared.get(header) {
            out.push(format!("Cargo.toml: {header} changed"));
        }
    }
    let used: BTreeSet<&String> = every.iter().chain(roots.iter()).collect();
    for name in used {
        if manifest_before.dependencies.get(name) != manifest_after.dependencies.get(name) {
            out.push(format!(
                "Cargo.toml: the workspace's {name} dependency changed"
            ));
        }
    }
    let local = before
        .values()
        .chain(after.values())
        .filter(|p| p.source.is_empty())
        .map(|p| p.name.clone())
        .collect();
    (out, local)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOCK: &str = r#"
version = 4

[[package]]
name = "app"
version = "0.1.0"
dependencies = [
 "lib",
 "util 1.0.0",
]

[[package]]
name = "lib"
version = "2.0.0"
source = "registry+https://example.invalid/index"
checksum = "aa"

[[package]]
name = "util"
version = "1.0.0"
source = "registry+https://example.invalid/index"
checksum = "bb"

[[package]]
name = "other"
version = "0.1.0"
dependencies = [
 "flate",
]

[[package]]
name = "flate"
version = "1.0.0"
"#;

    const MANIFEST: &str = r#"
[workspace.package]
edition = "2024" # the edition

[workspace.dependencies]
lib = "2"
flate = "1"

[profile.release]
panic = "abort"
"#;

    fn roots() -> BTreeSet<String> {
        BTreeSet::from(["app".to_owned()])
    }

    fn changes(manifest: &str, lock: &str) -> Vec<String> {
        workspace_changes(
            &Workspace {
                before: (MANIFEST, LOCK),
                after: (manifest, lock),
            },
            &roots(),
        )
        .0
    }

    #[test]
    fn a_crate_s_paths_bring_its_manifest_and_the_fuzz_paths_the_fuzz_build() {
        let paths = [
            "packages/crypto/src".to_owned(),
            "fuzz/fuzz_targets/block_decode.rs".to_owned(),
            "tools/oracle/lib".to_owned(),
        ];
        assert_eq!(crate_dirs(&paths, &[]), ["fuzz", "packages/crypto"]);
        let all = crate_dirs(&["packages".to_owned()], &["a".to_owned(), "b".to_owned()]);
        assert_eq!(all, ["packages/a", "packages/b"]);
        assert_eq!(
            package_name("[package]\nname = \"heartwood-crypto\" # the crate\n"),
            Some("heartwood-crypto".to_owned())
        );
    }

    #[test]
    fn a_crate_s_test_targets_dev_dependencies_and_comments_are_not_its_build() {
        let manifest = "[package]\nname = \"a\"\n\n[dependencies]\nlib = \"2\"\n";
        let more = format!(
            "{manifest}# a note\n[dev-dependencies]\ntracing = \"0.1\"\n\n[[test]]\nname = \"t\"\n"
        );
        assert!(!manifest_changed(Some(manifest), Some(&more)));
        let feature = format!("{manifest}\n[features]\nx = []\n");
        assert!(manifest_changed(Some(manifest), Some(&feature)));
        let bumped = manifest.replace("\"2\"", "\"3\"");
        assert!(manifest_changed(Some(manifest), Some(&bumped)));
        assert!(manifest_changed(None, Some(manifest)));
        assert!(!manifest_changed(None, None));
    }

    #[test]
    fn a_change_outside_the_tested_crates_build_is_not_a_change() {
        assert!(changes(MANIFEST, LOCK).is_empty());
        // Another crate's dependency, and a comment.
        let lock = LOCK.replace(
            "name = \"flate\"\nversion = \"1.0.0\"",
            "name = \"flate\"\nversion = \"1.1.0\"",
        );
        let manifest = MANIFEST.replace("flate = \"1\"", "flate = \"1.1\" # newer");
        assert!(
            changes(&manifest, &lock).is_empty(),
            "{:?}",
            changes(&manifest, &lock)
        );
        let manifest = MANIFEST.replace("# the edition", "# the language edition");
        assert!(changes(&manifest, LOCK).is_empty());
    }

    #[test]
    fn a_version_profile_lint_or_dependency_entry_the_crates_use_is_a_change() {
        let lock = LOCK.replace("version = \"2.0.0\"", "version = \"1.9.0\"");
        assert_eq!(changes(MANIFEST, &lock), ["Cargo.lock: lib 2.0.0 -> 1.9.0"]);
        let lock = LOCK.replace("checksum = \"bb\"", "checksum = \"cc\"");
        assert_eq!(changes(MANIFEST, &lock), ["Cargo.lock: util 1.0.0 changed"]);
        let manifest = MANIFEST.replace("panic = \"abort\"", "panic = \"unwind\"");
        assert_eq!(
            changes(&manifest, LOCK),
            ["Cargo.toml: [profile.release] changed"]
        );
        let manifest = MANIFEST.replace(
            "lib = \"2\"",
            "lib = { version = \"2\", features = [\"x\"] }",
        );
        assert_eq!(
            changes(&manifest, LOCK),
            ["Cargo.toml: the workspace's lib dependency changed"]
        );
        let manifest = format!("{MANIFEST}\n[workspace.lints.rust]\nunsafe_code = \"allow\"\n");
        assert_eq!(
            changes(&manifest, LOCK),
            ["Cargo.toml: [workspace.lints.rust] changed"]
        );
    }

    #[test]
    fn two_versions_of_one_library_are_told_apart() {
        let lock = LOCK.replace("\"util 1.0.0\",", "\"util 1.0.0\",\n \"util 2.0.0\",")
            + "\n[[package]]\nname = \"util\"\nversion = \"2.0.0\"\n\
               source = \"registry+https://example.invalid/index\"\nchecksum = \"dd\"\n";
        let ws = |before: &str, after: &str| {
            workspace_changes(
                &Workspace {
                    before: (MANIFEST, before),
                    after: (MANIFEST, after),
                },
                &roots(),
            )
            .0
        };
        assert!(ws(&lock, &lock).is_empty());
        // The older of the two versions changes; the newer one is the same.
        let older = lock.replace("checksum = \"bb\"", "checksum = \"cc\"");
        assert_eq!(ws(&lock, &older), ["Cargo.lock: util 1.0.0 changed"]);
        let bumped = lock
            .replace("\"util 1.0.0\"", "\"util 1.5.0\"")
            .replace("version = \"1.0.0\"\nsource", "version = \"1.5.0\"\nsource");
        // The crate that takes it names the new version too.
        assert_eq!(
            ws(&lock, &bumped),
            [
                "Cargo.lock: app 0.1.0 changed",
                "Cargo.lock: util 1.0.0 removed",
                "Cargo.lock: util 1.5.0 added"
            ]
        );
    }

    #[test]
    fn the_resolver_and_a_dependency_s_own_table_reach_the_build_and_the_members_do_not() {
        let manifest = "[workspace]\nresolver = \"2\"\nmembers = [\n  \"packages/*\",\n]\n\n\
                        [workspace.dependencies.lib]\nversion = \"2\"\n\n\
                        [workspace.metadata.tool]\nx = 1\n";
        let ws = |after: &str| {
            workspace_changes(
                &Workspace {
                    before: (manifest, LOCK),
                    after: (after, LOCK),
                },
                &roots(),
            )
            .0
        };
        assert!(ws(manifest).is_empty());
        let more = manifest.replace("\"packages/*\",", "\"packages/*\",\n  \"tools/x\",");
        assert!(ws(&more).is_empty(), "{:?}", ws(&more));
        assert_eq!(members(&more), ["packages/*", "tools/x"]);
        let metadata = manifest.replace("x = 1", "x = 2");
        assert!(ws(&metadata).is_empty());
        assert_eq!(
            ws(&manifest.replace("resolver = \"2\"", "resolver = \"3\"")),
            ["Cargo.toml: [workspace] changed"]
        );
        assert_eq!(
            ws(&manifest.replace("version = \"2\"", "version = \"2.1\"")),
            ["Cargo.toml: the workspace's lib dependency changed"]
        );
        // A table the rule does not know counts whole.
        let unknown = format!("{manifest}\n[workspace.someday]\nflag = true\n");
        assert_eq!(ws(&unknown), ["Cargo.toml: [workspace.someday] changed"]);
    }
}
