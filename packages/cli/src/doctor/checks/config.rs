//! Configuration and key files: the node's loaders, the log level, the run directory, the key file,
//! the configuration directory and stray key files. The loader gate lives here: no node loader runs
//! while others can write the configuration directory.

use std::path::PathBuf;

use heartwood_kernel::log::Level;

use crate::doctor::registry::CheckInput;
use crate::doctor::report::{Advice, Basis, Fact, Finding, Status, Untrusted, UntrustedSource};
use crate::doctor::report::{Status::Fail, Status::Pass, Status::Warn, keys};
use crate::doctor::seams::{FileFacts, FileKind, Meta, RoleState, Subject, subject_of};
use crate::doctor::view::{NodeDir, NodeFile, Reason, Source, Unknown};
use crate::seams::Role;

/// A finding whose facts are its advice's values.
fn found(
    input: &CheckInput<'_>,
    subject: Subject,
    (status, basis): (Status, Basis),
    key: &'static str,
    values: Vec<(&'static str, Fact)>,
) -> Finding {
    let mut finding = Finding::new(input.id(), subject, status, basis, Advice::key(key));
    finding.advice.values.clone_from(&values);
    finding.facts = values;
    finding
}

/// A file's path and permission mode, as facts.
fn path_mode(path: PathBuf, mode: u32) -> Vec<(&'static str, Fact)> {
    vec![("path", Fact::Path(path)), ("mode", Fact::Mode(mode))]
}

/// One finding for each role the check is about: `skipped` when the role is not on this host.
fn per_role(input: &CheckInput<'_>, judge: impl Fn(Role, Subject) -> Finding) -> Vec<Finding> {
    let presence = input.view.roles_present();
    let about = |role: &Role| input.entry.subjects.contains(&subject_of(*role));
    let one = |role: Role| match presence {
        Err(unknown) => Finding::unknown(input.id(), subject_of(role), &unknown),
        Ok(present) if !present.has(role) => {
            Finding::skipped(input.id(), subject_of(role), keys::SKIPPED_ABSENT)
        }
        Ok(_) => judge(role, subject_of(role)),
    };
    Role::ALL.into_iter().filter(about).map(one).collect()
}

/// The metadata that counts: a link's target's, when it has one.
fn meta(facts: &FileFacts) -> Meta {
    facts.target.unwrap_or(facts.own)
}

/// Whether users other than its own can replace what a directory holds: a group or other write bit
/// (a parent with the sticky bit keeps others from renaming or removing what is not theirs).
fn writable(facts: &FileFacts, parent: bool) -> bool {
    let mode = meta(facts).mode;
    mode & 0o022 != 0 && !(parent && mode & 0o1000 != 0)
}

/// The path of `file` in the scope, for a finding about a file that is not there.
fn path(input: &CheckInput<'_>, file: NodeFile) -> (&'static str, Fact) {
    let path = file.path(input.view.scope()).unwrap_or_default();
    ("path", Fact::Path(path))
}

/// `config.load`.
pub fn load(input: &CheckInput<'_>) -> Vec<Finding> {
    per_role(input, |role, subject| {
        let view = input.view;
        let loaded = match role == Role::Relay {
            true => view.relay_config().map(|f| f.warnings.clone()),
            false => view.forger_config().map(|_| Vec::new()),
        };
        let role = ("role", Fact::Word(subject.name()));
        let config = |status| (status, Basis::Config);
        match loaded {
            Err(unknown) if unknown.reason == Reason::LoaderRefused => {
                found(input, subject, config(Fail), "refused", vec![role])
            }
            Err(unknown) => Finding::unknown(input.id(), subject, &unknown),
            Ok(warnings) if warnings.is_empty() => {
                found(input, subject, config(Pass), "loads", vec![role])
            }
            Ok(warnings) => {
                let count = (
                    "count",
                    Fact::Int(warnings.len().try_into().unwrap_or(i64::MAX)),
                );
                let mut finding =
                    found(input, subject, config(Warn), "warnings", vec![role, count]);
                let (source, at_ms) = (UntrustedSource::ConfigWarning, None);
                let text = |text| Untrusted {
                    source,
                    at_ms,
                    text,
                };
                finding.untrusted = warnings.into_iter().map(text).collect();
                finding
            }
        }
    })
}

/// `config.log-level`.
pub fn log_level(input: &CheckInput<'_>) -> Vec<Finding> {
    per_role(input, |role, subject| {
        let level = match role == Role::Relay {
            true => input.view.relay_config().map(|facts| facts.file_log_level),
            false => input.view.forger_config().map(|facts| facts.file_log_level),
        };
        let level = match level {
            Err(unknown) => return Finding::unknown(input.id(), subject, &unknown),
            Ok(level) => level,
        };
        let (status, key) = match level >= Level::Info {
            true => (Pass, "detailed"),
            false => (Warn, "too-high"),
        };
        let values = vec![
            ("role", Fact::Word(role.name())),
            ("level", Fact::Word(level.name())),
        ];
        found(input, subject, (status, Basis::Config), key, values)
    })
}

/// `config.run-dir`.
pub fn run_dir(input: &CheckInput<'_>) -> Vec<Finding> {
    let finding =
        |status, key, values| found(input, Subject::Host, (status, Basis::System), key, values);
    vec![match input.view.file(NodeFile::RunDir) {
        Err(unknown) => Finding::unknown(input.id(), Subject::Host, &unknown),
        Ok(None) => finding(Pass, "absent", vec![path(input, NodeFile::RunDir)]),
        Ok(Some(facts)) => {
            let meta = meta(&facts);
            let values = path_mode(facts.path, meta.mode);
            match input.view.own_uid() {
                Ok(uid) if uid != meta.uid => finding(Fail, "other-user", values),
                _ if meta.mode & 0o077 != 0 => finding(Warn, "open", values),
                _ => finding(Pass, "private", values),
            }
        }
    }]
}

/// `keys.file`: the key file's metadata, never its contents.
pub fn keys_file(input: &CheckInput<'_>) -> Vec<Finding> {
    per_role(input, |_, subject| {
        let finding =
            |status, key, values| found(input, subject, (status, Basis::System), key, values);
        let facts = match input.view.file(NodeFile::KeyFile) {
            Err(unknown) => return Finding::unknown(input.id(), subject, &unknown),
            Ok(None) => return finding(Fail, "missing", vec![path(input, NodeFile::KeyFile)]),
            Ok(Some(facts)) => facts,
        };
        let meta = meta(&facts);
        let other_user = input.view.own_uid().is_ok_and(|uid| uid != meta.uid);
        let values = path_mode(facts.path, meta.mode);
        match meta.kind {
            FileKind::File if meta.mode & 0o077 != 0 => finding(Fail, "open", values),
            FileKind::File if other_user => finding(Fail, "other-user", values),
            FileKind::File if facts.own.kind == FileKind::Symlink => finding(Warn, "link", values),
            FileKind::File if meta.mode & 0o100 != 0 => finding(Warn, "exec", values),
            FileKind::File => finding(Pass, "private", values),
            _ => finding(Fail, "not-file", values),
        }
    })
}

/// `keys.dir`: the configuration directory and its parent, which the loader gate also reads.
pub fn keys_dir(input: &CheckInput<'_>) -> Vec<Finding> {
    let view = input.view;
    let finding =
        |status, key, values| found(input, Subject::Host, (status, Basis::System), key, values);
    let (dir, parent) = (NodeFile::Dir(NodeDir::Config), NodeFile::ConfigParent);
    let config = path(input, dir);
    vec![match (view.file(dir), view.file(parent)) {
        (Err(unknown), _) | (_, Err(unknown)) => {
            Finding::unknown(input.id(), Subject::Host, &unknown)
        }
        (Ok(dir), Ok(parent)) => {
            let mut dirs = [(dir, false), (parent, true)].into_iter();
            match dirs.find_map(|(f, parent)| f.filter(|f| writable(f, parent))) {
                Some(open) => finding(Fail, "open", path_mode(open.path.clone(), meta(&open).mode)),
                None => finding(Pass, "private", vec![config]),
            }
        }
    }]
}

/// `keys.stray`: a key file on a host where no forger of the instance has run.
pub fn keys_stray(input: &CheckInput<'_>) -> Vec<Finding> {
    per_role(input, |_, subject| {
        let finding =
            |status, key, values| found(input, subject, (status, Basis::System), key, values);
        let process = input.view.process(Role::Forger);
        let forger = process.map(|p| p.state != RoleState::Stopped || p.supervisor.is_some());
        match (input.view.file(NodeFile::KeyFile), forger) {
            (Ok(None), _) => finding(Pass, "none", Vec::new()),
            (Err(unknown), _) | (_, Err(unknown)) => {
                Finding::unknown(input.id(), subject, &unknown)
            }
            (Ok(Some(_)), Ok(true)) => finding(Pass, "forger", Vec::new()),
            (Ok(Some(facts)), Ok(false)) => {
                finding(Warn, "stray", vec![("path", Fact::Path(facts.path))])
            }
        }
    })
}

/// Whether the node's loaders may run, from the metadata of the configuration directory `dir`
/// and its parent: not while a user other than the one they belong to can write either, since such
/// a user could have planted a link to the key file there. The view calls it before every loader.
pub fn loader_gate(dir: Option<&FileFacts>, parent: Option<&FileFacts>) -> Result<(), Unknown> {
    let open = dir.is_some_and(|dir| writable(dir, false))
        || parent.is_some_and(|parent| writable(parent, true));
    match open {
        true => Err(Unknown::new(Source::Config, Reason::KeysDirUnsafe)),
        false => Ok(()),
    }
}
