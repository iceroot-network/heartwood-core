//! The supervisor with the test child (feature `test-child`): an immediate restart after a run of
//! 1 s or more, `errored` after 5 unstable runs, SIGINT then SIGKILL after 30 s (a slow test),
//! signals only while a lock is held, the state file's transitions, a kill during a restart, the
//! node's environment, the second supervisor's refusal, and the capture files with their rotation
//! at a small limit.

#![cfg(feature = "test-child")]

use std::ffi::OsString;
use std::fs;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use heartwood_cli::process;
use heartwood_cli::seams::{Instance, Role, SupervisedStatus};
use heartwood_cli::supervisor::logs::{Capture, KEEP};
use heartwood_cli::supervisor::signals::{is_locked, send, signal_if_locked, take_lock};
use heartwood_cli::supervisor::state::SupervisorState;
use heartwood_kernel::paths::Paths;
use nix::sys::signal::Signal;

const CHILD: &str = env!("CARGO_BIN_EXE_test-child");

/// A fresh directory for one test, under the target's temporary directory.
fn scratch(test: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("cli-supervisor")
        .join(test);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// A relay instance whose directories are under a fresh directory; its processes are killed
/// when it is dropped, so that a failed test leaves none behind.
struct Node(Instance);

impl Node {
    fn new(test: &str) -> Node {
        let root = scratch(test);
        Node(Instance {
            token: "t".into(),
            network: "devnet".into(),
            role: Role::Relay,
            paths: Paths {
                config: root.join("config"),
                data: root.join("data"),
                cache: root.join("cache"),
                log: root.join("log"),
                temp: root.join("temp"),
            },
        })
    }

    fn state(&self) -> SupervisorState {
        SupervisorState::read(&self.0.state_path())
            .unwrap()
            .unwrap()
    }

    fn out(&self) -> String {
        fs::read_to_string(self.0.out_log_path()).unwrap_or_default()
    }

    fn start(&self, args: &[&str], env: &[(&str, &str)]) {
        let env = env.iter().map(|(k, v)| (k.into(), v.into())).collect();
        process::start(&self.0, &argv(args), &env, true).unwrap();
    }

    fn online(&self) -> u32 {
        wait_for("the node online", Duration::from_secs(10), || {
            SupervisorState::read(&self.0.state_path())
                .unwrap()
                .is_some_and(|s| s.status == SupervisedStatus::Online && s.pid.is_some())
        });
        self.state().pid.unwrap()
    }
}

impl Drop for Node {
    fn drop(&mut self) {
        let _ = process::kill(&self.0);
    }
}

fn argv(args: &[&str]) -> Vec<OsString> {
    std::iter::once(CHILD)
        .chain(args.iter().copied())
        .map(OsString::from)
        .collect()
}

fn wait_for(what: &str, timeout: Duration, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + timeout;
    while !done() {
        assert!(
            Instant::now() < deadline,
            "{what} did not happen within {timeout:?}"
        );
        thread::sleep(Duration::from_millis(20));
    }
}

/// The node's command line for [`kill_during_a_restart`]: it takes its lock 800 ms after its
/// start, as a node that loads its configuration and builds its runtime first.
const LATE_LOCK: [&str; 4] = ["relay", "run", "--lock-after", "800"];

/// Crash `node`'s node (run with [`LATE_LOCK`]), then kill the instance while the node its
/// supervisor started again has not taken its lock yet: no node may run on.
fn kill_during_a_restart(node: &Node) {
    let first = node.online();
    let lock = node.0.lock_path();
    wait_for("the node's lock", Duration::from_secs(10), || {
        is_locked(&lock).unwrap()
    });
    assert!(send(first, Signal::SIGKILL).unwrap());
    wait_for("the node started again", Duration::from_secs(10), || {
        let state = node.state();
        state.status == SupervisedStatus::Online && state.pid.is_some_and(|pid| pid != first)
    });
    let second = node.state().pid.unwrap();
    assert!(!is_locked(&lock).unwrap(), "the node took its lock at once");
    process::kill(&node.0).unwrap();
    // Past the moment the node would have taken its lock.
    thread::sleep(Duration::from_millis(1200));
    assert!(
        !is_locked(&lock).unwrap(),
        "the node {second} outlived the kill"
    );
    let row = process::status(&node.0).unwrap();
    assert_eq!((row.status, row.pid), (SupervisedStatus::Stopped, None));
}

fn count(text: &str, needle: &str) -> usize {
    text.matches(needle).count()
}

/// The statuses the supervisor recorded, in order, from its lines in the output capture.
fn transitions(out: &str) -> Vec<&str> {
    out.lines()
        .filter_map(|line| line.strip_prefix("supervisor: status "))
        .collect()
}

#[test]
fn a_node_that_ran_a_second_or_more_is_restarted_at_once() {
    let node = Node::new("restart");
    let began = Instant::now();
    node.start(&["exit", "3", "1100"], &[]);
    // The third run is up: its status is recorded as online with its pid, and its first lines
    // have reached both captures (the status is written before the node prints anything).
    let errors = || fs::read_to_string(node.0.error_log_path()).unwrap_or_default();
    wait_for("two restarts", Duration::from_secs(10), || {
        let state = node.state();
        state.restarts >= 2
            && state.status == SupervisedStatus::Online
            && state.pid.is_some()
            && count(&node.out(), "test-child: exit 3 1100") >= 3
            && count(&errors(), "test-child: error line") >= 3
    });
    // Two runs of 1.1 s each and the start of the third: no back-off between them.
    assert!(
        began.elapsed() < Duration::from_secs(3),
        "{:?}",
        began.elapsed()
    );
    let state = node.state();
    assert_eq!(state.unstable, 0);
    assert_eq!(state.last_exit.as_deref(), Some("exit status: 3"));
    assert_eq!(state.status, SupervisedStatus::Online);
    assert!(count(&node.out(), "test-child: exit 3 1100") >= 3);
    assert_eq!(
        count(&node.out(), "supervisor: restarting the node"),
        state.restarts as usize
    );
    let errors = errors();
    assert!(count(&errors, "test-child: error line") >= 3);
    assert!(!errors.contains("supervisor:"));
    process::stop(&node.0).unwrap();
    assert_eq!(node.state().status, SupervisedStatus::Stopped);
}

#[test]
fn a_run_is_timed_to_the_moment_its_node_exited_not_to_the_next_poll() {
    let node = Node::new("uptime");
    node.start(&["exit", "1", "970"], &[]);
    wait_for("three exits", Duration::from_secs(15), || {
        count(&node.out(), "supervisor: the node exited") >= 3
    });
    process::stop(&node.0).ok();
    let out = node.out();
    let uptimes: Vec<u64> = out
        .lines()
        .filter_map(|line| line.strip_suffix(" ms")?.rsplit(' ').next()?.parse().ok())
        .collect();
    assert!(uptimes.len() >= 3, "{out}");
    assert!(uptimes[..3].iter().all(|&ms| ms < 1_000), "{uptimes:?}");
    assert!(node.state().unstable >= 3);
}

#[test]
fn five_unstable_runs_in_the_first_seconds_mark_the_role_errored() {
    let node = Node::new("errored");
    node.start(&["exit", "1"], &[]);
    wait_for("the supervisor's exit", Duration::from_secs(10), || {
        !is_locked(&node.0.supervisor_lock_path()).unwrap()
    });
    let state = node.state();
    assert_eq!(state.status, SupervisedStatus::Errored);
    assert_eq!((state.restarts, state.unstable, state.pid), (4, 5, None));
    let out = node.out();
    assert_eq!(count(&out, "test-child: exit 1"), 5);
    assert!(out.contains("supervisor: the node exited within 1 s five times in its first 5 s"));
    assert_eq!(transitions(&out).last(), Some(&"errored"));
    let row = process::status(&node.0).unwrap();
    assert_eq!((row.status, row.pid), (SupervisedStatus::Errored, None));
    // An errored role can be started again.
    node.start(&["run"], &[]);
    node.online();
    process::stop(&node.0).unwrap();
}

#[test]
fn a_stop_sends_sigint_and_the_state_file_follows_every_change() {
    let node = Node::new("stop");
    node.start(&["run"], &[]);
    let pid = node.online();
    let supervisor = node.state().supervisor_pid;
    assert_ne!(pid, supervisor);
    process::stop(&node.0).unwrap();
    let state = node.state();
    assert_eq!((state.status, state.pid), (SupervisedStatus::Stopped, None));
    assert_eq!(state.last_exit.as_deref(), Some("signal: 2 (SIGINT)"));
    assert_eq!(state.argv, vec![CHILD.to_owned(), "run".to_owned()]);
    assert!(!is_locked(&node.0.supervisor_lock_path()).unwrap());
    assert!(!send(pid, Signal::SIGCONT).unwrap(), "the node still runs");
    let out = node.out();
    assert_eq!(
        transitions(&out),
        ["launching", "online", "stopping", "stopped"]
    );
    assert!(out.contains(&format!("supervisor: SIGTERM: SIGINT to the node {pid}")));
}

#[test]
fn a_daemon_that_cannot_start_says_why() {
    let node = Node::new("daemon-error");
    // A directory where the capture's file should be: the supervisor cannot open it.
    fs::create_dir_all(node.0.error_log_path()).unwrap();
    let env = Vec::new();
    let failed = process::start(&node.0, &argv(&["run"]), &env, true).unwrap_err();
    let text = failed.to_string();
    assert!(text.contains("error.log"), "{text}");
}

#[test]
fn a_stop_signals_the_pid_the_held_lock_names_not_the_one_a_state_file_left() {
    let node = Node::new("stale-pid");
    node.start(&["run"], &[]);
    node.online();
    let real = node.state();
    // Another process, standing where a reused pid would be.
    let mut bystander = Command::new("sleep").arg("60").spawn().unwrap();
    let mut stale = real.clone();
    stale.supervisor_pid = bystander.id();
    stale.write(&node.0.state_path()).unwrap();
    process::stop(&node.0).unwrap();
    let alive = bystander.try_wait().unwrap().is_none();
    let _ = bystander.kill();
    let _ = bystander.wait();
    assert!(alive, "the stop signalled a process the lock does not name");
    assert!(!is_locked(&node.0.supervisor_lock_path()).unwrap());
}

#[test]
fn a_terminal_interrupt_reaches_the_node_once_through_the_supervisor() {
    use heartwood_cli::supervisor::SuperviseArgs;
    use nix::sys::signal::killpg;
    use nix::unistd::Pid;
    use std::os::unix::process::CommandExt;
    let node = Node::new("foreground-interrupt");
    fs::create_dir_all(&node.0.paths.data).unwrap();
    // A foreground `start`: the supervisor without `--daemon`, in a process group of its own that
    // stands for the terminal's, whose SIGINT goes to every process of the group.
    let child = argv(&["graceful-sigint"]);
    let mut supervisor = Command::new(CHILD)
        .args(SuperviseArgs::command_line(&node.0, &child, false))
        .envs(process::path_env(&node.0))
        .stdin(Stdio::null())
        .process_group(0)
        .spawn()
        .unwrap();
    wait_for("the node's first line", Duration::from_secs(10), || {
        node.out().contains("test-child: graceful-sigint")
    });
    let group = Pid::from_raw(i32::try_from(supervisor.id()).unwrap());
    killpg(group, Signal::SIGINT).unwrap();
    let status = supervisor.wait().unwrap();
    assert!(status.success(), "{status:?}");
    let state = node.state();
    assert_eq!(state.status, SupervisedStatus::Stopped);
    assert_eq!(state.last_exit.as_deref(), Some("exit status: 0"));
}

#[test]
fn a_kill_during_a_restart_ends_a_daemons_node_that_has_not_taken_its_lock_yet() {
    let node = Node::new("kill-in-restart");
    node.start(&LATE_LOCK, &[]);
    kill_during_a_restart(&node);
}

#[test]
fn a_kill_during_a_restart_ends_a_foreground_supervisors_node_that_has_not_taken_its_lock_yet() {
    use heartwood_cli::supervisor::SuperviseArgs;
    use std::os::unix::process::CommandExt;
    let node = Node::new("kill-in-restart-foreground");
    let child = argv(&LATE_LOCK);
    // The supervisor without `--daemon`, in a process group of its own as a terminal's job.
    let mut supervisor = Command::new(CHILD)
        .args(SuperviseArgs::command_line(&node.0, &child, false))
        .envs(process::path_env(&node.0))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .process_group(0)
        .spawn()
        .unwrap();
    kill_during_a_restart(&node);
    assert_eq!(
        supervisor.wait().unwrap().signal(),
        Some(Signal::SIGKILL as i32)
    );
}

#[test]
fn the_node_runs_with_the_callers_environment_and_the_instance_directories() {
    let node = Node::new("environment");
    node.start(
        &["env", "HW_TEST_VAR", "CORE_PATH_DATA", "CORE_PATH_LOG"],
        &[
            ("HW_TEST_VAR", "from the caller"),
            ("CORE_PATH_DATA", "/elsewhere"),
        ],
    );
    node.online();
    wait_for("the environment lines", Duration::from_secs(5), || {
        node.out().contains("CORE_PATH_LOG=")
    });
    let out = node.out();
    assert!(out.contains("HW_TEST_VAR=from the caller\n"));
    let data = node.0.paths.data.display();
    assert!(out.contains(&format!("CORE_PATH_DATA={data}\n")), "{out}");
    assert!(out.contains(&format!("CORE_PATH_LOG={}\n", node.0.paths.log.display())));
    process::stop(&node.0).unwrap();
}

#[test]
fn a_second_supervisor_of_one_instance_refuses_to_run() {
    let node = Node::new("second");
    node.start(&["run"], &[]);
    node.online();
    let second = Command::new(CHILD)
        .args([
            "supervise",
            "--role",
            "relay",
            "--token",
            "t",
            "--network",
            "devnet",
        ])
        .args(["--", CHILD, "run"])
        .envs(process::path_env(&node.0))
        .stdout(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(second.status.code(), Some(1));
    let message = String::from_utf8_lossy(&second.stderr);
    assert!(
        message.contains("t-devnet-relay is already running"),
        "{message}"
    );
    assert_eq!(node.state().status, SupervisedStatus::Online);
    process::stop(&node.0).unwrap();
}

#[test]
fn signals_go_only_to_a_process_whose_lock_is_held() {
    let dir = scratch("signals");
    let lock = dir.join("relay.lock");
    let mut child = Command::new(CHILD)
        .arg("run")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    assert!(!signal_if_locked(child.id(), Signal::SIGINT, &lock).unwrap());
    fs::write(&lock, "").unwrap();
    assert!(!is_locked(&lock).unwrap());
    assert!(!signal_if_locked(child.id(), Signal::SIGINT, &lock).unwrap());
    thread::sleep(Duration::from_millis(200));
    assert!(
        child.try_wait().unwrap().is_none(),
        "signalled without its lock"
    );

    let held = take_lock(&lock).unwrap().unwrap();
    assert!(is_locked(&lock).unwrap());
    assert!(take_lock(&lock).unwrap().is_none());
    assert!(signal_if_locked(child.id(), Signal::SIGINT, &lock).unwrap());
    assert_eq!(child.wait().unwrap().signal(), Some(Signal::SIGINT as i32));
    drop(held);
    assert!(!is_locked(&lock).unwrap());
    assert!(take_lock(&lock).unwrap().is_some());
}

/// Slow: the 30 s of the stop rule.
#[test]
fn a_node_that_ignores_sigint_is_killed_30_seconds_later() {
    let node = Node::new("sigkill");
    node.start(&["ignore-sigint"], &[]);
    node.online();
    let began = Instant::now();
    process::stop(&node.0).unwrap();
    let took = began.elapsed();
    assert!(took >= Duration::from_secs(30), "{took:?}");
    assert!(took < Duration::from_secs(34), "{took:?}");
    let state = node.state();
    assert_eq!(state.status, SupervisedStatus::Stopped);
    assert_eq!(state.last_exit.as_deref(), Some("signal: 9 (SIGKILL)"));
    assert!(node.out().contains("did not stop within 30 s: SIGKILL"));
}

#[test]
fn captures_rotate_at_their_limit_and_keep_seven_files() {
    let dir = scratch("rotation");
    let path = dir.join("t-devnet-relay-out.log");
    let mut capture = Capture::open(&path, 100).unwrap();
    // 30 bytes a line: three lines fit in 100 bytes, the fourth starts a new file.
    let line = |i: usize| format!("line {i:05} of the node output\n");
    assert_eq!(line(0).len(), 30);
    for i in 0..40 {
        capture.write(line(i).as_bytes()).unwrap();
    }
    assert_eq!(fs::read_to_string(&path).unwrap(), line(39));
    for k in 1..=KEEP {
        let rotated = fs::read_to_string(Capture::rotated(&path, k)).unwrap();
        let first = 39 - 3 * k;
        assert_eq!(
            rotated,
            [first, first + 1, first + 2].map(line).concat(),
            "{k}"
        );
    }
    assert!(!Capture::rotated(&path, KEEP + 1).exists());

    // A capture opened again appends and counts what the file holds.
    drop(capture);
    let mut again = Capture::open(&path, 100).unwrap();
    again.write(line(40).as_bytes()).unwrap();
    again.write(line(41).as_bytes()).unwrap();
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        [39, 40, 41].map(line).concat()
    );
    again.write(line(42).as_bytes()).unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), line(42));

    // A write larger than the limit goes whole into a file of its own.
    let big = "x".repeat(250);
    again.write(big.as_bytes()).unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), big);
    assert_eq!(
        fs::read_to_string(Capture::rotated(&path, 1)).unwrap(),
        line(42)
    );
}
