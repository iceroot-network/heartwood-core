//! The key files and the loader gate: the doctor's key-file list is the node's; while other users
//! can write the configuration directory or its parent, no node loader runs and the configuration
//! checks report unknown; once the directory is private, the loaders run. Every file's metadata
//! here is the real filesystem's.

mod doctor_support;

use doctor_support::{MetadataWorld, Node, Recording};
use heartwood_cli::Role;
use heartwood_cli::doctor::report::{Finding, Report, Status};
use heartwood_cli::doctor::seams::{KEY_FILE_NAMES, NodeView, Readable, ReadableRoots, Subject};
use heartwood_cli::doctor::view::NodeFile;
use heartwood_cli::doctor::{DoctorOptions, DoctorScope};

#[test]
fn the_key_file_list_is_the_forgers_key_file() {
    // In this release the forger's key file is the only file the node loads keys from.
    assert_eq!(KEY_FILE_NAMES, [heartwood_forger::keys::FILE]);
    let node = Node::new("keys-list", "");
    let forger = node.instance(Role::Forger);
    let scope = DoctorScope::of(&forger, None, None);
    let key = heartwood_forger::keys::path(&node.config());
    assert_eq!(NodeFile::KeyFile.path(&scope), Some(key.clone()));
    assert_eq!(node.view().forger(&forger).unwrap().key_file, key);
    // The doctor may read no path of the list, in any case of its name.
    let roots = ReadableRoots::for_paths(&node.paths());
    assert!(Readable::new(&key, &roots).is_err());
    let upper = node
        .config()
        .join(heartwood_forger::keys::FILE.to_uppercase());
    assert!(Readable::new(&upper, &roots).is_err());
    // The signing record holds no key: its bytes are read and dropped at parse time.
    assert!(!KEY_FILE_NAMES.contains(&heartwood_forger::signing_record::FILE));
}

/// A node with both roles present: a relay's lock file and a forger's key file of `mode`.
fn node(name: &str, key_mode: u32) -> Node {
    let node = Node::new(name, "CORE_P2P_PORT=6102\n");
    let run = node.paths().data.join("run");
    std::fs::create_dir_all(&run).unwrap();
    node.chmod(&run, 0o700);
    std::fs::write(run.join("relay.lock"), "").unwrap();
    let key = node.config().join(heartwood_forger::keys::FILE);
    std::fs::write(&key, "{}").unwrap();
    node.chmod(&key, key_mode);
    node
}

/// The doctor's run on `node` over the real files' metadata; the node view's calls.
fn run(node: &Node) -> (Report, Vec<&'static str>) {
    let view = node.view();
    let recording = Recording::new(&view);
    let world = MetadataWorld { node: &recording };
    let scope = DoctorScope::of(&node.instance(Role::Relay), Some(node.home()), None);
    let report = heartwood_cli::doctor::run(&world, &scope, &DoctorOptions::default());
    let calls = recording.calls.borrow().clone();
    (report, calls)
}

fn finding<'r>(report: &'r Report, id: &str, subject: Subject) -> &'r Finding {
    let found = report
        .findings
        .iter()
        .find(|f| f.id.as_str() == id && f.subject == subject);
    found.unwrap_or_else(|| panic!("no {id} about {subject:?}"))
}

#[test]
fn while_others_can_write_the_configuration_directory_no_loader_runs() {
    let node = node("keys-gate", 0o600);
    let (config, parent) = (node.config(), node.root.clone());
    for (dir, mode) in [
        (&config, 0o770),
        (&config, 0o702),
        (&parent, 0o777),
        (&parent, 0o775),
    ] {
        node.chmod(dir, mode);
        let (report, calls) = run(&node);
        assert!(calls.is_empty(), "{mode:o}: {calls:?}");
        let keys_dir = finding(&report, "keys.dir", Subject::Host);
        assert_eq!(
            (keys_dir.status, keys_dir.advice.key),
            (Status::Fail, "open")
        );
        for subject in [Subject::Relay, Subject::Forger] {
            for id in ["config.load", "config.log-level"] {
                let found = finding(&report, id, subject);
                assert_eq!(found.status, Status::Unknown, "{id}");
                let reason = found
                    .advice
                    .values
                    .iter()
                    .find(|(name, _)| *name == "reason");
                let reason = format!("{:?}", reason.unwrap().1);
                assert!(reason.contains("writable by others"), "{reason}");
            }
        }
        // The key file's own check reads metadata only, and still runs.
        assert_eq!(
            finding(&report, "keys.file", Subject::Forger).status,
            Status::Pass
        );
        node.chmod(dir, 0o700);
    }
    // A sticky parent that others can write, such as a shared temporary directory, is safe.
    node.chmod(&parent, 0o1777);
    let (report, calls) = run(&node);
    node.chmod(&parent, 0o755);
    assert!(
        calls.contains(&"relay") && calls.contains(&"forger"),
        "{calls:?}"
    );
    assert_eq!(
        finding(&report, "keys.dir", Subject::Host).status,
        Status::Pass
    );
    for subject in [Subject::Relay, Subject::Forger] {
        let load = finding(&report, "config.load", subject);
        assert_eq!((load.status, load.advice.key), (Status::Pass, "loads"));
        assert_eq!(
            finding(&report, "config.log-level", subject).status,
            Status::Pass
        );
    }
}

#[test]
fn the_key_file_check_reads_the_real_files_metadata() {
    for (mode, status, key) in [
        (0o600, Status::Pass, "private"),
        (0o400, Status::Pass, "private"),
        (0o700, Status::Warn, "exec"),
        (0o640, Status::Fail, "open"),
        (0o620, Status::Fail, "open"),
        (0o604, Status::Fail, "open"),
    ] {
        let node = node(&format!("keys-mode-{mode:o}"), mode);
        let (report, _) = run(&node);
        let found = finding(&report, "keys.file", Subject::Forger);
        assert_eq!((found.status, found.advice.key), (status, key), "{mode:o}");
        node.chmod(&node.config().join(heartwood_forger::keys::FILE), 0o600);
    }
    // A link with an innocent name is judged by its target and warned about.
    let node = node("keys-link", 0o600);
    let key = node.config().join(heartwood_forger::keys::FILE);
    let elsewhere = node.root.join("kept.json");
    std::fs::rename(&key, &elsewhere).unwrap();
    std::os::unix::fs::symlink(&elsewhere, &key).unwrap();
    let (report, _) = run(&node);
    let found = finding(&report, "keys.file", Subject::Forger);
    assert_eq!((found.status, found.advice.key), (Status::Warn, "link"));
    // A relay-only host with a key file: a stray key.
    let stray = finding(&report, "keys.stray", Subject::Relay);
    assert_eq!((stray.status, stray.advice.key), (Status::Warn, "stray"));
}
