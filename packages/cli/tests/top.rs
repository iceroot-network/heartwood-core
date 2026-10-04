//! `top` and the instance index it reads: the entries the supervisor writes and removes through
//! the writer that refuses a link, the rows of the instances whose supervisor holds its lock, and
//! the entries that are ignored or refused.

use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};

use heartwood_cli::instance_index::{self, IndexError, data_root, entry_path, index_dir};
use heartwood_cli::seams::{Instance, Role, SupervisedStatus};
use heartwood_cli::supervisor::signals::take_lock;
use heartwood_cli::supervisor::state::SupervisorState;
use heartwood_cli::top::{NONE_RUNNING, top};
use heartwood_kernel::paths::Paths;

/// A fresh directory for one test, under the target's temporary directory.
fn scratch(test: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("cli-top")
        .join(test);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn instance(root: &Path, token: &str, network: &str, role: Role) -> Instance {
    let at = |name: &str| root.join(format!("{token}-{network}")).join(name);
    Instance {
        token: token.into(),
        network: network.into(),
        role,
        paths: Paths {
            config: at("config"),
            data: at("data"),
            cache: at("cache"),
            log: at("log"),
            temp: at("temp"),
        },
    }
}

/// `instance` as a supervisor that runs it leaves it: its entry, and its state and lock.
fn running(root: &Path, instance: &Instance) -> std::fs::File {
    instance_index::write(root, instance).unwrap();
    heartwood_cli::supervisor::run_dir(instance).unwrap();
    let lock = take_lock(&instance.supervisor_lock_path())
        .unwrap()
        .unwrap();
    let state = SupervisorState {
        name: instance.name(),
        status: SupervisedStatus::Online,
        supervisor_pid: std::process::id(),
        pid: Some(std::process::id()),
        argv: vec!["heartwood".into()],
        started_at_ms: None,
        restarts: 3,
        unstable: 0,
        last_exit: None,
        version: "4.3.1-heartwood.0.1.0".into(),
    };
    state.write(&instance.state_path()).unwrap();
    lock
}

#[test]
fn the_data_root_is_the_tokens_directory_without_a_network() {
    let home = Path::new("/srv/someone");
    let env = |name: &str| match name {
        "XDG_DATA_HOME" => Some("/x/data".to_owned()),
        "CORE_PATH_DATA" => Some("/elsewhere".to_owned()),
        _ => None,
    };
    // CORE_PATH_DATA moves one network's data only, never the index.
    assert_eq!(
        data_root("node1", &env, home),
        Path::new("/x/data/node1-core")
    );
    let bare = |_: &str| None;
    assert_eq!(
        data_root("solar", &bare, home),
        Path::new("/srv/someone/.local/share/solar-core")
    );
}

#[test]
fn an_entry_is_written_read_back_and_removed() {
    let root = scratch("roundtrip");
    let relay = instance(&root, "node1", "devnet", Role::Relay);
    instance_index::write(&root, &relay).unwrap();
    let path = entry_path(&root, &relay);
    assert_eq!(path, root.join("instances/node1-devnet-relay.json"));
    let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!((mode(&path), mode(&index_dir(&root))), (0o600, 0o700));
    assert_eq!(
        instance_index::list(&root, "node1").unwrap(),
        vec![relay.clone()]
    );
    // Written again over itself, and no temporary file is left.
    instance_index::write(&root, &relay).unwrap();
    assert_eq!(fs::read_dir(index_dir(&root)).unwrap().count(), 1);
    instance_index::remove(&root, &relay).unwrap();
    assert!(!path.exists());
    // Removing what is not there is no error, and nor is a root without an index.
    instance_index::remove(&root, &relay).unwrap();
    assert!(
        instance_index::list(&root.join("none"), "node1")
            .unwrap()
            .is_empty()
    );
}

#[test]
fn the_list_holds_the_tokens_entries_only_in_name_order() {
    let root = scratch("order");
    let all = [
        instance(&root, "node1", "testnet", Role::Forger),
        instance(&root, "node1", "devnet", Role::Relay),
        instance(&root, "node1", "devnet", Role::Forger),
        instance(&root, "node1-b", "devnet", Role::Relay),
        instance(&root, "node2", "devnet", Role::Relay),
    ];
    for one in &all {
        instance_index::write(&root, one).unwrap();
    }
    let names: Vec<String> = instance_index::list(&root, "node1")
        .unwrap()
        .iter()
        .map(Instance::name)
        .collect();
    assert_eq!(
        names,
        [
            "node1-devnet-forger",
            "node1-devnet-relay",
            "node1-testnet-forger"
        ]
    );
}

#[test]
fn a_link_is_refused_for_an_entry_and_for_the_index() {
    let root = scratch("links");
    let relay = instance(&root, "node1", "devnet", Role::Relay);
    let elsewhere = root.join("elsewhere");
    fs::create_dir_all(&elsewhere).unwrap();
    // The index directory is a link.
    symlink(&elsewhere, index_dir(&root)).unwrap();
    assert!(matches!(
        instance_index::write(&root, &relay),
        Err(IndexError::Io { .. })
    ));
    assert!(instance_index::list(&root, "node1").is_err());
    assert!(fs::read_dir(&elsewhere).unwrap().next().is_none());
    fs::remove_file(index_dir(&root)).unwrap();
    // An entry is a link to a file: it is never read, and a write replaces the link.
    instance_index::write(&root, &relay).unwrap();
    let path = entry_path(&root, &relay);
    let target = elsewhere.join("target.json");
    fs::write(&target, "{}").unwrap();
    fs::remove_file(&path).unwrap();
    symlink(&target, &path).unwrap();
    assert!(instance_index::list(&root, "node1").is_err());
    instance_index::write(&root, &relay).unwrap();
    assert_eq!(fs::read_to_string(&target).unwrap(), "{}");
    assert!(!fs::symlink_metadata(&path).unwrap().is_symlink());
    assert_eq!(instance_index::list(&root, "node1").unwrap().len(), 1);
    // The temporary file's place is a link too: the write is refused, and nothing is written
    // through the link.
    let temp = index_dir(&root).join("node1-devnet-relay.json.tmp");
    symlink(&target, &temp).unwrap();
    assert!(instance_index::write(&root, &relay).is_err());
    assert_eq!(fs::read_to_string(&target).unwrap(), "{}");
}

#[test]
fn an_entry_that_is_not_valid_is_named_in_the_error() {
    let root = scratch("invalid");
    let relay = instance(&root, "node1", "devnet", Role::Relay);
    instance_index::write(&root, &relay).unwrap();
    let path = entry_path(&root, &relay);
    let good = fs::read_to_string(&path).unwrap();
    let cases = [
        "not json".to_owned(),
        "{}".to_owned(),
        good.replace("devnet-relay\"", "devnet-forger\""),
        good.replace("\"relay\"", "\"seed\""),
        good.replace("/config", "config")
            .replace(&root.display().to_string(), "."),
    ];
    for bad in cases {
        fs::write(&path, &bad).unwrap();
        match instance_index::list(&root, "node1") {
            Err(IndexError::Entry { path: named, .. }) => assert_eq!(named, path, "{bad}"),
            other => panic!("{bad}: {other:?}"),
        }
    }
    // A directory in an entry's place is not a regular file.
    fs::remove_file(&path).unwrap();
    fs::create_dir(&path).unwrap();
    assert!(instance_index::list(&root, "node1").is_err());
}

#[test]
fn top_lists_the_instances_whose_supervisor_holds_its_lock() {
    let root = scratch("rows");
    let relay = instance(&root, "node1", "devnet", Role::Relay);
    let forger = instance(&root, "node1", "testnet", Role::Forger);
    let gone = instance(&root, "node1", "devnet", Role::Forger);
    let other = instance(&root, "node2", "devnet", Role::Relay);
    let _held = [
        running(&root, &relay),
        running(&root, &forger),
        running(&root, &other),
    ];
    // An entry whose supervisor is gone: its state file says online, and its lock is free.
    let _ = running(&root, &gone);
    let rows = top("node1", &root).unwrap();
    let seen: Vec<_> = rows
        .iter()
        .map(|r| (r.name.as_str(), r.status, r.pid, r.restarts))
        .collect();
    let own = Some(std::process::id());
    assert_eq!(
        seen,
        [
            ("node1-devnet-relay", SupervisedStatus::Online, own, 3),
            ("node1-testnet-forger", SupervisedStatus::Online, own, 3),
        ]
    );
    let table = heartwood_cli::process_table::render(&rows);
    assert!(table.starts_with("ID"), "{table}");
    assert!(table.lines().nth(1).unwrap().contains("node1-devnet-relay"));
    assert_eq!(top("node2", &root).unwrap().len(), 1);
    assert!(top("node3", &root).unwrap().is_empty());
}

#[test]
fn top_is_empty_without_an_index_or_supervisors() {
    let root = scratch("none");
    assert!(top("node1", &root).unwrap().is_empty());
    let relay = instance(&root, "node1", "devnet", Role::Relay);
    instance_index::write(&root, &relay).unwrap();
    assert!(top("node1", &root).unwrap().is_empty());
    assert_eq!(NONE_RUNNING, "No processes are running");
}

#[test]
fn top_refuses_a_link_in_the_index() {
    let root = scratch("top-links");
    let relay = instance(&root, "node1", "devnet", Role::Relay);
    let _held = running(&root, &relay);
    let path = entry_path(&root, &relay);
    let copy = root.join("copy.json");
    fs::rename(&path, &copy).unwrap();
    symlink(&copy, &path).unwrap();
    assert!(top("node1", &root).is_err());
}

/// The supervisor writes the entry when it starts and removes it when it stops (feature
/// `test-child`, which stands for the node and `heartwood supervise`).
#[cfg(feature = "test-child")]
mod with_child {
    use std::ffi::OsString;
    use std::thread;
    use std::time::{Duration, Instant};

    use heartwood_cli::process;

    use super::*;

    const CHILD: &str = env!("CARGO_BIN_EXE_test-child");

    #[test]
    fn a_supervisor_writes_its_entry_at_the_start_and_removes_it_at_the_stop() {
        let root = scratch("supervised");
        let data = root.join("xdg");
        let relay = instance(&root, "node1", "devnet", Role::Relay);
        let argv: Vec<OsString> = [CHILD, "run"].map(OsString::from).into();
        let env = vec![("XDG_DATA_HOME".into(), data.clone().into())];
        process::start(&relay, &argv, &env, true).unwrap();
        let index = data.join("node1-core");
        let deadline = Instant::now() + Duration::from_secs(10);
        while top("node1", &index).unwrap().is_empty() {
            assert!(Instant::now() < deadline, "the instance is not listed");
            thread::sleep(Duration::from_millis(20));
        }
        let rows = top("node1", &index).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "node1-devnet-relay");
        assert_eq!(
            instance_index::list(&index, "node1").unwrap(),
            vec![relay.clone()]
        );
        process::stop(&relay).unwrap();
        assert!(instance_index::list(&index, "node1").unwrap().is_empty());
        assert!(top("node1", &index).unwrap().is_empty());
    }
}
