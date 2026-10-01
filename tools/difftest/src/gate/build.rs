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

/// The tables of a crate's manifest that reach its build, without comments and blank lines, by
/// header: every table but those of its tests, benchmarks, examples and dev-dependencies.
fn build_tables(manifest: &str) -> BTreeMap<String, Vec<String>> {
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut header: Option<String> = Some(String::new());
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for line in manifest.lines().map(strip_comment) {
        let line = line.trim();
        if line.starts_with('[') {
            let left_out = ["[[test]]", "[[bench]]", "[[example]]", "[dev-dependencies"]
                .iter()
                .any(|start| line.starts_with(start))
                || line.contains(".dev-dependencies");
            header = (!left_out).then(|| {
                // A repeated table (`[[bin]]`) is told apart by its place.
                let n = counts.entry(line.to_owned()).or_insert(0);
                *n += 1;
                format!("{line} {n}")
            });
            continue;
        }
        if let Some(header) = &header
            && !line.is_empty()
        {
            out.entry(header.clone()).or_default().push(line.to_owned());
        }
    }
    out
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
    let mut in_package = false;
    for line in manifest.lines().map(strip_comment) {
        let line = line.trim();
        if line.starts_with('[') {
            in_package = line == "[package]";
        } else if in_package && let Some(value) = assigned(line, "name") {
            return Some(value);
        }
    }
    None
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

/// The sections of the workspace's manifest that reach every crate's build, without comments and
/// blank lines: the package settings, the profiles, the lints and the patches, by header.
fn shared_sections(manifest: &str) -> BTreeMap<String, Vec<String>> {
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut header: Option<String> = None;
    for line in manifest.lines().map(strip_comment) {
        let line = line.trim();
        if line.starts_with('[') {
            let shared = [
                "[workspace.package]",
                "[profile.",
                "[workspace.lints",
                "[patch",
            ];
            header = shared
                .iter()
                .any(|start| line.starts_with(start))
                .then(|| line.to_owned());
            continue;
        }
        if let Some(header) = &header
            && !line.is_empty()
        {
            out.entry(header.clone()).or_default().push(line.to_owned());
        }
    }
    out
}

/// The entries of the workspace's dependency table, by name, without comments.
fn workspace_dependencies(manifest: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut inside = false;
    for line in manifest.lines().map(strip_comment) {
        let line = line.trim();
        if line.starts_with('[') {
            inside = line == "[workspace.dependencies]";
        } else if inside && let Some((name, value)) = line.split_once('=') {
            out.insert(name.trim().to_owned(), value.trim().to_owned());
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

/// What changed in the workspace's manifest and lockfile between the two commits that reaches
/// the build of the crates named `roots` (their package names), each as a short phrase.
pub(super) fn workspace_changes(ws: &Workspace<'_>, roots: &BTreeSet<String>) -> Vec<String> {
    let mut out = Vec::new();
    let (before, after) = (
        closure(&lock_packages(ws.before.1), roots),
        closure(&lock_packages(ws.after.1), roots),
    );
    let names = |c: &BTreeMap<String, Locked>| -> BTreeMap<String, Locked> {
        c.values().map(|p| (p.name.clone(), p.clone())).collect()
    };
    let (by_name_before, by_name_after) = (names(&before), names(&after));
    let every: BTreeSet<&String> = by_name_before.keys().chain(by_name_after.keys()).collect();
    for name in &every {
        match (by_name_before.get(*name), by_name_after.get(*name)) {
            (Some(was), Some(now)) if was.version != now.version => out.push(format!(
                "Cargo.lock: {name} {} -> {}",
                was.version, now.version
            )),
            (Some(was), Some(now)) if was != now => {
                out.push(format!("Cargo.lock: {name} {} changed", now.version));
            }
            (None, Some(now)) => out.push(format!("Cargo.lock: {name} {} added", now.version)),
            (Some(was), None) => out.push(format!("Cargo.lock: {name} {} removed", was.version)),
            _ => {}
        }
    }
    let (shared_before, shared_after) = (shared_sections(ws.before.0), shared_sections(ws.after.0));
    let headers: BTreeSet<&String> = shared_before.keys().chain(shared_after.keys()).collect();
    for header in headers {
        if shared_before.get(header) != shared_after.get(header) {
            out.push(format!("Cargo.toml: {header} changed"));
        }
    }
    let (deps_before, deps_after) = (
        workspace_dependencies(ws.before.0),
        workspace_dependencies(ws.after.0),
    );
    let used: BTreeSet<&String> = every.iter().copied().chain(roots.iter()).collect();
    for name in used {
        if deps_before.get(name) != deps_after.get(name) {
            out.push(format!(
                "Cargo.toml: the workspace's {name} dependency changed"
            ));
        }
    }
    out
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
}
