mod doctor_support;

use doctor_support::Node;
use heartwood_cli::doctor::HostProcesses;
use heartwood_cli::doctor::seams::{NodeView, NodeViewError, ProcessView, WindowStart};
use heartwood_cli::{ProcessError, Role};

/// A state file as a supervisor writes it.
const STATE: &str = r#"{
  "name": "node1-devnet-relay",
  "status": "online",
  "supervisorPid": 41,
  "pid": 42,
  "argv": ["heartwood", "relay", "run"],
  "startedAt": 1700000000000,
  "restarts": 2,
  "unstable": 0,
  "lastExit": null,
  "version": "1.0.0"
}
"#;

#[test]
fn linked_run_files_are_refused_at_their_open_and_their_targets_left_unchanged() {
    let node = Node::new("run-file-links", "");
    let relay = node.instance(Role::Relay);
    std::fs::create_dir_all(relay.lock_path().parent().unwrap()).unwrap();

    // A held lock behind a link at the lock path.
    let lock_target = node.root.join("lock-target");
    std::fs::write(&lock_target, "untouched").unwrap();
    let lock = std::fs::File::open(&lock_target).unwrap();
    lock.lock().unwrap();
    std::os::unix::fs::symlink(&lock_target, relay.lock_path()).unwrap();
    assert_eq!(
        node.view()
            .store_window(&relay, &[], WindowStart::Height(1)),
        Err(NodeViewError::RelayNotRunning)
    );
    assert!(HostProcesses.status(&relay).is_err());
    // Left unchanged: these checks show nothing was written, not that nothing was read.
    assert_eq!(std::fs::read_to_string(&lock_target).unwrap(), "untouched");

    // The same valid state is read through a regular file, and refused through a link.
    std::fs::write(relay.state_path(), STATE).unwrap();
    let read = HostProcesses.supervisor(&relay).unwrap().unwrap();
    assert_eq!(
        (read.started_at_ms, read.restarts),
        (Some(1_700_000_000_000), 2)
    );
    assert_eq!(read.version, "1.0.0");
    std::fs::remove_file(relay.state_path()).unwrap();
    let state_target = node.root.join("state-target");
    std::fs::write(&state_target, STATE).unwrap();
    std::os::unix::fs::symlink(&state_target, relay.state_path()).unwrap();
    match HostProcesses.supervisor(&relay) {
        Err(ProcessError::Io { path, .. }) => assert_eq!(path, relay.state_path()),
        other => panic!("a linked state file was not refused at its open: {other:?}"),
    }
    assert_eq!(std::fs::read_to_string(&state_target).unwrap(), STATE);
}
