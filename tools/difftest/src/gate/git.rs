//! The git questions the path rule asks, through the `git` command in a chosen tree.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use super::build;
use crate::errors::{Error, Result};

/// A working tree, addressed by its root.
#[derive(Debug, Clone)]
pub struct Repo {
    root: PathBuf,
}

impl Repo {
    /// The tree rooted at `root`.
    pub fn open(root: &Path) -> Repo {
        Repo {
            root: root.to_path_buf(),
        }
    }

    /// The tree's root, as it was given.
    pub fn root(&self) -> &Path {
        &self.root
    }

    fn run(&self, args: &[&str]) -> Result<Output> {
        Command::new("git")
            .arg("-C")
            .arg(&self.root)
            .args(args)
            .output()
            .map_err(|e| Error::io(&self.root, e))
    }

    fn checked(&self, args: &[&str]) -> Result<String> {
        let output = self.run(args)?;
        if !output.status.success() {
            return Err(Error::Corpus {
                path: self.root.clone(),
                message: format!(
                    "git {}: {}",
                    args.join(" "),
                    String::from_utf8_lossy(&output.stderr).trim()
                ),
            });
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    /// The full commit id `rev` names.
    pub fn resolve(&self, rev: &str) -> Result<String> {
        let spec = format!("{rev}^{{commit}}");
        Ok(self
            .checked(&["rev-parse", "--verify", "--quiet", &spec])?
            .trim()
            .to_owned())
    }

    /// Whether the tree has changes beside its head: tracked changes or files git does not
    /// ignore and does not track.
    pub fn dirty(&self) -> Result<bool> {
        Ok(!self.checked(&["status", "--porcelain"])?.trim().is_empty())
    }

    /// Whether `commit` exists in the tree.
    pub fn knows(&self, commit: &str) -> bool {
        let spec = format!("{commit}^{{commit}}");
        self.run(&["cat-file", "-e", &spec])
            .is_ok_and(|o| o.status.success())
    }

    /// Whether `ancestor` is an ancestor of `descendant` (or the same commit).
    pub fn is_ancestor(&self, ancestor: &str, descendant: &str) -> Result<bool> {
        let output = self.run(&["merge-base", "--is-ancestor", ancestor, descendant])?;
        match output.status.code() {
            Some(0) => Ok(true),
            Some(1) => Ok(false),
            _ => Err(Error::Corpus {
                path: self.root.clone(),
                message: format!(
                    "git merge-base --is-ancestor {ancestor} {descendant}: {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                ),
            }),
        }
    }

    /// The files under `paths` (the whole tree when there are none) that differ between the
    /// commits `from` and `to`, sorted. Documentation files (`*.md`) under a tested directory
    /// are left out, so a commit that changes documentation only leaves every record valid; a
    /// tested path that is itself a documentation file (a guide a command checks) is kept.
    pub fn changed(&self, from: &str, to: &str, paths: &[String]) -> Result<Vec<String>> {
        let (documents, others): (Vec<&String>, Vec<&String>) =
            paths.iter().partition(|p| p.ends_with(".md"));
        let literal = |p: &&String| format!(":(literal){p}");
        let mut files = Vec::new();
        if !others.is_empty() || documents.is_empty() {
            let mut specs: Vec<String> = others.iter().map(literal).collect();
            if specs.is_empty() {
                specs.push(":(top)".to_owned());
            }
            specs.push(":(top,exclude,glob)**/*.md".to_owned());
            files.extend(self.diff(from, to, &specs)?);
        }
        if !documents.is_empty() {
            let specs: Vec<String> = documents.iter().map(literal).collect();
            files.extend(self.diff(from, to, &specs)?);
        }
        files.sort();
        files.dedup();
        Ok(files)
    }

    /// The file `path` at commit `rev`, if it has one.
    pub fn show(&self, rev: &str, path: &str) -> Result<Option<String>> {
        let spec = format!("{rev}:{path}");
        let output = self.run(&["show", &spec])?;
        Ok(output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).into_owned()))
    }

    /// The names of the directories under `dir` at commit `rev`.
    pub fn dirs(&self, rev: &str, dir: &str) -> Result<Vec<String>> {
        let tree = format!("{rev}:{dir}");
        let output = self.run(&["ls-tree", "-d", "--name-only", &tree])?;
        Ok(String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(str::to_owned)
            .collect())
    }

    fn diff(&self, from: &str, to: &str, specs: &[String]) -> Result<Vec<String>> {
        let mut args = vec!["diff", "--name-only", "-z", from, to, "--"];
        args.extend(specs.iter().map(String::as_str));
        let text = self.checked(&args)?;
        Ok(text
            .split('\0')
            .filter(|f| !f.is_empty())
            .map(str::to_owned)
            .collect())
    }
}

/// Whether an entry made at one head still counts at another.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Validity {
    /// Its head is an ancestor of the final head and none of its paths changed since.
    Valid,
    /// It ran on a tree with changes beside its head.
    Dirty,
    /// Its head is not a commit of this tree.
    Unknown,
    /// Its head is not an ancestor of the final head.
    NotAncestor,
    /// One of its paths or build inputs changed since; the changed files (the first few), or
    /// what changed in a build input.
    Stale(Vec<String>),
    /// It was made with a version of the devnet tooling the criteria do not accept, or names
    /// none where the criteria require it; why.
    Tooling(String),
}

impl Validity {
    /// Whether the entry counts.
    pub fn counts(&self) -> bool {
        matches!(self, Validity::Valid)
    }

    /// Whether the entry no longer counts for a reason that is no fault of its run (its tree or
    /// its tooling moved on since): stale, or made with tooling the criteria no longer accept.
    pub fn is_superseded(&self) -> bool {
        matches!(self, Validity::Stale(_) | Validity::Tooling(_))
    }

    /// One phrase for a table.
    pub fn describe(&self) -> String {
        match self {
            Validity::Valid => "valid".to_owned(),
            Validity::Dirty => "dirty tree".to_owned(),
            Validity::Unknown => "head unknown to this tree".to_owned(),
            Validity::NotAncestor => "head is not an ancestor".to_owned(),
            Validity::Stale(files) => {
                let shown: Vec<&str> = files.iter().take(3).map(String::as_str).collect();
                let more = files.len().saturating_sub(3);
                if more == 0 {
                    format!("stale: {} changed", shown.join(", "))
                } else {
                    format!("stale: {} and {more} more changed", shown.join(", "))
                }
            }
            Validity::Tooling(why) => format!("tooling: {why}"),
        }
    }
}

/// The path rule against one final head.
#[derive(Debug)]
pub struct PathRule {
    repo: Repo,
    head: String,
    allow_dirty: bool,
    cache: std::cell::RefCell<std::collections::HashMap<(String, Vec<String>), Validity>>,
    /// The workspace's member directories at each commit asked, by package name.
    members: std::cell::RefCell<std::collections::HashMap<String, BTreeMap<String, String>>>,
}

impl PathRule {
    /// The rule for `head` (a revision) in `repo`; dirty entries count only with `allow_dirty`.
    pub fn new(repo: Repo, head: &str, allow_dirty: bool) -> Result<PathRule> {
        let head = repo.resolve(head)?;
        Ok(PathRule {
            repo,
            head,
            allow_dirty,
            cache: std::cell::RefCell::default(),
            members: std::cell::RefCell::default(),
        })
    }

    /// The final head, in full.
    pub fn head(&self) -> &str {
        &self.head
    }

    /// Whether something that ran at `entry_head` on a tree that was `dirty` or not, testing
    /// `paths` (the whole tree when empty), counts at the final head.
    pub fn check(&self, entry_head: &str, dirty: bool, paths: &[String]) -> Result<Validity> {
        if dirty && !self.allow_dirty {
            return Ok(Validity::Dirty);
        }
        let key = (entry_head.to_owned(), paths.to_vec());
        if let Some(found) = self.cache.borrow().get(&key) {
            return Ok(found.clone());
        }
        let found = self.compute(entry_head, paths)?;
        self.cache.borrow_mut().insert(key, found.clone());
        Ok(found)
    }

    fn compute(&self, entry_head: &str, paths: &[String]) -> Result<Validity> {
        let well_formed =
            entry_head.len() == 40 && entry_head.bytes().all(|b| b.is_ascii_hexdigit());
        if !well_formed || !self.repo.knows(entry_head) {
            return Ok(Validity::Unknown);
        }
        if entry_head == self.head {
            return Ok(Validity::Valid);
        }
        if !self.repo.is_ancestor(entry_head, &self.head)? {
            return Ok(Validity::NotAncestor);
        }
        let changed = self.repo.changed(entry_head, &self.head, paths)?;
        if !changed.is_empty() {
            return Ok(Validity::Stale(changed));
        }
        let built = self.build_changes(entry_head, paths)?;
        Ok(if built.is_empty() {
            Validity::Valid
        } else {
            Validity::Stale(built)
        })
    }

    /// What changed between `from` and the final head in the build inputs of `paths` (none to
    /// look at when the paths are the whole tree, which holds them): the tested crates' manifests,
    /// the fuzz targets' lockfile, what of the workspace's manifest and lockfile reaches them
    /// ([`build`]), and the manifests of the tree's other crates they are built with.
    fn build_changes(&self, from: &str, paths: &[String]) -> Result<Vec<String>> {
        if paths.is_empty() {
            return Ok(Vec::new());
        }
        let packages = self.repo.dirs(&self.head, "packages")?;
        let dirs = build::crate_dirs(paths, &packages);
        let mut out = Vec::new();
        let mut roots = BTreeSet::new();
        for dir in &dirs {
            if self.manifest_changed(from, dir)? {
                out.push(format!("{dir}/Cargo.toml (its build)"));
            }
            if dir != "fuzz" {
                let manifest = self.repo.show(&self.head, &format!("{dir}/Cargo.toml"))?;
                roots.extend(manifest.as_deref().and_then(build::package_name));
            }
        }
        if dirs.iter().any(|d| d == "fuzz") {
            let lock = [build::FUZZ_LOCK.to_owned()];
            out.extend(self.repo.changed(from, &self.head, &lock)?);
        }
        if roots.is_empty() {
            return Ok(out);
        }
        let read = |rev: &str, file: &str| -> Result<String> {
            Ok(self.repo.show(rev, file)?.unwrap_or_default())
        };
        let [manifest, lock] = build::WORKSPACE;
        let before = (read(from, manifest)?, read(from, lock)?);
        let after = (read(&self.head, manifest)?, read(&self.head, lock)?);
        let (changes, local) = build::workspace_changes(
            &build::Workspace {
                before: (&before.0, &before.1),
                after: (&after.0, &after.1),
            },
            &roots,
        );
        out.extend(changes);
        // The tree's crates the tested ones are built with: a feature of theirs reaches the
        // build without a line of the lockfile changing.
        let mut used = BTreeSet::new();
        for rev in [from, self.head.as_str()] {
            let members = self.member_dirs(rev)?;
            used.extend(local.iter().filter_map(|name| members.get(name).cloned()));
        }
        for dir in used.iter().filter(|d| !dirs.contains(d)) {
            if self.manifest_changed(from, dir)? {
                out.push(format!("{dir}/Cargo.toml (the build of a crate it uses)"));
            }
        }
        Ok(out)
    }

    /// Whether the manifest of the crate in `dir` changed between `from` and the final head in
    /// what reaches its build.
    fn manifest_changed(&self, from: &str, dir: &str) -> Result<bool> {
        let manifest = format!("{dir}/Cargo.toml");
        let before = self.repo.show(from, &manifest)?;
        let after = self.repo.show(&self.head, &manifest)?;
        Ok(build::manifest_changed(before.as_deref(), after.as_deref()))
    }

    /// The workspace's member directories at `rev`, by the package name of each one's manifest.
    fn member_dirs(&self, rev: &str) -> Result<BTreeMap<String, String>> {
        if let Some(found) = self.members.borrow().get(rev) {
            return Ok(found.clone());
        }
        let manifest = self
            .repo
            .show(rev, build::WORKSPACE[0])?
            .unwrap_or_default();
        let mut out = BTreeMap::new();
        for pattern in build::members(&manifest) {
            let dirs = match pattern.strip_suffix("/*") {
                Some(stem) => self
                    .repo
                    .dirs(rev, stem)?
                    .into_iter()
                    .map(|d| format!("{stem}/{d}"))
                    .collect(),
                None => vec![pattern],
            };
            for dir in dirs {
                let manifest = self.repo.show(rev, &format!("{dir}/Cargo.toml"))?;
                if let Some(name) = manifest.as_deref().and_then(build::package_name) {
                    out.insert(name, dir);
                }
            }
        }
        self.members
            .borrow_mut()
            .insert(rev.to_owned(), out.clone());
        Ok(out)
    }
}
