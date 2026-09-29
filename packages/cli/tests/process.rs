//! The role commands `start`, `stop`, `restart`, `status` and `log` with their refusals, the
//! devnet's process operations over them (a node that holds its lock, killed as a crash is), a
//! node without a supervisor, `log`'s tail, `status`'s uptime from a corrupt start time, and the
//! process table on Linux.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use heartwood_cli::process::{Supervisors, path_env, tail};
use heartwood_cli::process_table::{self, render};
use heartwood_cli::seams::{Instance, Role, StatusRow, SupervisedStatus};
use heartwood_cli::supervisor::signals::{is_locked, take_lock, take_lock_in};
use heartwood_cli::supervisor::state::SupervisorState;
use heartwood_cli::supervisor::{RunDir, run_dir};
use heartwood_kernel::paths::Paths;
use heartwood_kernel::{Clock, SystemClock};

/// A fresh directory for one test, under the target's temporary directory.
fn scratch(test: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("cli-process")
        .join(test);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn instance(root: &Path, role: Role) -> Instance {
    Instance {
        token: "node1".into(),
        network: "devnet".into(),
        role,
        paths: Paths {
            config: root.join("config"),
            data: root.join("data"),
            cache: root.join("cache"),
            log: root.join("log"),
            temp: root.join("temp"),
        },
    }
}

#[test]
fn tail_gives_the_last_lines_of_a_file() {
    let dir = scratch("tail");
    let path = dir.join("out.log");
    let lines: Vec<String> = (1..=20).map(|i| format!("line {i}\n")).collect();
    fs::write(&path, lines.concat()).unwrap();
    let (last, len) = tail(&path, 15).unwrap();
    assert_eq!(String::from_utf8(last).unwrap(), lines[5..].concat());
    assert_eq!(len, fs::metadata(&path).unwrap().len());
    assert_eq!(tail(&path, 100).unwrap().0, lines.concat().into_bytes());
    assert_eq!(tail(&path, 1).unwrap().0, b"line 20\n");
    assert!(tail(&path, 0).unwrap().0.is_empty());
    fs::write(&path, "a\nb\nc").unwrap();
    assert_eq!(tail(&path, 2).unwrap().0, b"b\nc");
    fs::write(&path, "").unwrap();
    assert_eq!(tail(&path, 15).unwrap(), (Vec::new(), 0));
    // A file far longer than the window is read from its end only.
    let long = format!("{}\n{}", "x".repeat(3 * 1024 * 1024), lines.concat());
    fs::write(&path, long).unwrap();
    assert_eq!(tail(&path, 3).unwrap().0, lines[17..].concat().into_bytes());
}

#[test]
fn the_table_has_the_references_columns_and_restarts() {
    let running = StatusRow {
        pid: Some(4242),
        name: "node1-devnet-relay".into(),
        version: "4.3.1-heartwood.0.1.0".into(),
        status: SupervisedStatus::Online,
        uptime: Some(Duration::from_secs(3_725)),
        restarts: 2,
        cpu_percent: Some(1.26),
        rss_bytes: Some(52_300_000),
    };
    let stopped = StatusRow {
        pid: None,
        name: "node1-devnet-forger".into(),
        version: "4.3.1-heartwood.0.1.0".into(),
        status: SupervisedStatus::Stopped,
        uptime: None,
        restarts: 0,
        cpu_percent: None,
        rss_bytes: None,
    };
    let table = render(&[running.clone(), stopped]);
    let lines: Vec<&str> = table.lines().collect();
    fn cells(line: &str) -> Vec<&str> {
        let cells = line.split("  ").filter(|c| !c.is_empty());
        cells.map(str::trim).collect()
    }
    assert_eq!(
        cells(lines[0]),
        [
            "ID", "Name", "Version", "Status", "Uptime", "Restarts", "CPU", "RAM"
        ]
    );
    assert_eq!(
        cells(lines[1]),
        [
            "4242",
            "node1-devnet-relay",
            "4.3.1-heartwood.0.1.0",
            "online",
            "1h 2m",
            "2",
            "1.3%",
            "52.3 MB"
        ]
    );
    assert_eq!(
        cells(lines[2]),
        [
            "-",
            "node1-devnet-forger",
            "4.3.1-heartwood.0.1.0",
            "stopped",
            "-",
            "0",
            "-",
            "-"
        ]
    );
    let column = |line: &str, text: &str| line.find(text).unwrap();
    assert_eq!(column(lines[0], "Status"), column(lines[1], "online"));
    assert_eq!(column(lines[0], "Status"), column(lines[2], "stopped"));
    for (uptime, rss, shown) in [
        (59, 512, ["59s", "512 B"]),
        (61, 1_500, ["1m 1s", "1.5 kB"]),
        (90_061, 7_200_000_000, ["1d 1h", "7.2 GB"]),
    ] {
        let row = StatusRow {
            uptime: Some(Duration::from_secs(uptime)),
            rss_bytes: Some(rss),
            ..running.clone()
        };
        let table = render(&[row]);
        let line = table.lines().nth(1).unwrap();
        assert!(shown.iter().all(|s| line.contains(s)), "{line}");
    }
}

#[test]
fn the_process_table_reads_proc_on_linux() {
    let own = std::process::id();
    let sample = process_table::sample(own).unwrap();
    assert!(sample.rss_bytes > 1024 * 1024, "{sample:?}");
    assert!(
        sample.cpu_percent.is_finite() && sample.cpu_percent >= 0.0,
        "{sample:?}"
    );
    let from_proc = process_table::from_proc(own).unwrap();
    assert!(from_proc.rss_bytes > 0);
    // No process can have a pid above the kernel's largest.
    assert!(process_table::sample(4_194_305).is_err());
    if Path::new("/bin/ps").exists() {
        assert!(process_table::from_ps(own).unwrap().rss_bytes > 0);
        assert!(process_table::from_ps(4_194_305).is_err());
    }
}

#[test]
fn status_gives_no_uptime_for_a_start_time_it_cannot_subtract() {
    let root = scratch("uptime");
    let relay = instance(&root, Role::Relay);
    heartwood_cli::supervisor::run_dir(&relay).unwrap();
    // This test holds the supervisor's lock, and the state names its own process as the node.
    let _lock = take_lock(&relay.supervisor_lock_path()).unwrap().unwrap();
    let own = std::process::id();
    let now = SystemClock.now_ms();
    for (started, shown) in [
        (i64::MIN, false),
        (i64::MAX, false),
        (now + 60_000, false),
        (now - 5_000, true),
    ] {
        let state = SupervisorState {
            name: relay.name(),
            status: SupervisedStatus::Online,
            supervisor_pid: own,
            pid: Some(own),
            argv: vec!["heartwood".into(), "relay".into(), "run".into()],
            started_at_ms: Some(started),
            restarts: 0,
            unstable: 0,
            last_exit: None,
            version: "4.3.1-heartwood.0.1.0".into(),
        };
        state.write(&relay.state_path()).unwrap();
        let row = heartwood_cli::process::status(&relay).unwrap();
        assert_eq!((row.status, row.pid), (SupervisedStatus::Online, Some(own)));
        assert_eq!(row.uptime.is_some(), shown, "started at {started}");
    }
}

#[test]
fn the_devnet_runs_each_role_as_heartwood_role_run_with_its_directories() {
    let root = scratch("node-argv");
    let forger = instance(&root, Role::Forger);
    let ops = Supervisors {
        binary: "/opt/heartwood".into(),
    };
    let argv: Vec<String> = ops
        .node_argv(&forger)
        .iter()
        .map(|a| a.to_str().unwrap().to_owned())
        .collect();
    assert_eq!(
        argv,
        [
            "/opt/heartwood",
            "forger",
            "run",
            "--token",
            "node1",
            "--network",
            "devnet"
        ]
    );
    let env: Vec<(String, PathBuf)> = path_env(&forger)
        .into_iter()
        .map(|(k, v)| (k.into_string().unwrap(), v.into()))
        .collect();
    assert_eq!(
        env,
        ["CONFIG", "DATA", "CACHE", "LOG", "TEMP"]
            .iter()
            .zip(["config", "data", "cache", "log", "temp"])
            .map(|(kind, dir)| (format!("CORE_PATH_{kind}"), root.join(dir)))
            .collect::<Vec<_>>()
    );
}

#[test]
fn the_run_files_are_made_in_the_directory_that_was_checked() {
    let root = scratch("swapped-run-dir");
    let relay = instance(&root, Role::Relay);
    let held = RunDir::open(&relay).unwrap();
    let run = run_dir(&relay).unwrap();
    // The checked directory is swapped for a link to another directory.
    let (checked, elsewhere) = (root.join("checked"), root.join("elsewhere"));
    fs::create_dir(&elsewhere).unwrap();
    fs::rename(&run, &checked).unwrap();
    std::os::unix::fs::symlink(&elsewhere, &run).unwrap();
    let state = SupervisorState {
        name: "node1-devnet-relay".into(),
        status: SupervisedStatus::Online,
        supervisor_pid: 41,
        pid: Some(42),
        argv: vec!["heartwood".into()],
        started_at_ms: None,
        restarts: 0,
        unstable: 0,
        last_exit: None,
        version: "4.3.1-heartwood.0.1.0".into(),
    };
    // By path, no file is made through the link.
    assert!(take_lock(&relay.lock_path()).is_err());
    assert!(state.write(&relay.state_path()).is_err());
    assert!(is_locked(&relay.lock_path()).is_err());
    // By the directory held, the files are made in the directory that was checked.
    let lock = take_lock_in(&held, &relay.lock_path()).unwrap().unwrap();
    state.write_in(&held, &relay.state_path()).unwrap();
    assert!(checked.join("relay.lock").is_file() && checked.join("relay.state.json").is_file());
    drop(lock);
    let landed: Vec<_> = fs::read_dir(&elsewhere).unwrap().collect();
    assert!(
        landed.is_empty(),
        "files landed in the link's target: {landed:?}"
    );
}

#[test]
fn a_lock_left_as_a_link_is_refused_and_its_target_left_alone() {
    let root = scratch("lock-link");
    let relay = instance(&root, Role::Relay);
    let run = run_dir(&relay).unwrap();
    let target = root.join("target");
    fs::write(&target, "kept").unwrap();
    std::os::unix::fs::symlink(&target, run.join("relay.lock")).unwrap();
    assert!(take_lock(&relay.lock_path()).is_err());
    assert_eq!(fs::read_to_string(&target).unwrap(), "kept");
}

#[test]
fn the_run_directory_is_made_private_a_wider_one_narrowed_and_a_file_refused() {
    use std::os::unix::fs::PermissionsExt;
    use std::sync::Barrier;
    use std::time::Instant;

    let root = scratch("run-dir");
    let relay = instance(&root, Role::Relay);
    let run = relay.run_dir();
    let mode = |path: &Path| {
        let metadata = fs::symlink_metadata(path);
        metadata.map(|m| m.permissions().mode() & 0o777).ok()
    };
    // A watcher that reads the directory's mode as soon as it is there sees 0700 only: the
    // directory is never there with a wider mode, which a creation then a change could show.
    for _ in 0..100 {
        let _ = fs::remove_dir(&run);
        let started = Barrier::new(2);
        let seen = std::thread::scope(|scope| {
            let watcher = scope.spawn(|| {
                started.wait();
                let deadline = Instant::now() + Duration::from_secs(10);
                while Instant::now() < deadline {
                    if let Some(seen) = mode(&run) {
                        return Some(seen);
                    }
                    std::hint::spin_loop();
                }
                None
            });
            started.wait();
            let made = heartwood_cli::supervisor::run_dir(&relay);
            (made, watcher.join().unwrap())
        });
        assert_eq!(seen.0.unwrap(), run);
        assert_eq!(seen.1, Some(0o700));
    }
    fs::set_permissions(&run, fs::Permissions::from_mode(0o755)).unwrap();
    heartwood_cli::supervisor::run_dir(&relay).unwrap();
    assert_eq!(mode(&run), Some(0o700));
    fs::remove_dir(&run).unwrap();
    fs::write(&run, "").unwrap();
    let refused = heartwood_cli::supervisor::run_dir(&relay);
    assert!(
        matches!(&refused, Err(heartwood_cli::ProcessError::Io { path, .. }) if *path == run),
        "{refused:?}"
    );
    // A link to a directory is refused, and the directory it names is left alone.
    fs::remove_file(&run).unwrap();
    let target = root.join("elsewhere");
    fs::create_dir(&target).unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o755)).unwrap();
    std::os::unix::fs::symlink(&target, &run).unwrap();
    let linked = heartwood_cli::supervisor::run_dir(&relay);
    assert!(
        matches!(&linked, Err(heartwood_cli::ProcessError::Io { path, .. }) if *path == run),
        "{linked:?}"
    );
    assert_eq!(mode(&target), Some(0o755));
}

/// The commands over the test child (feature `test-child`), which stands for the node and, as
/// `test-child supervise`, for `heartwood supervise`.
#[cfg(feature = "test-child")]
mod with_child {
    use std::ffi::{CString, OsString};
    use std::os::fd::AsRawFd;
    use std::os::unix::ffi::OsStrExt;
    use std::process::Command;
    use std::thread;
    use std::time::Instant;

    use nix::spawn::{PosixSpawnAttr, PosixSpawnFileActions, PosixSpawnFlags, posix_spawnp};
    use nix::sys::signal::SigSet;
    use nix::sys::signal::Signal::SIGINT;
    use nix::sys::wait::{WaitStatus, waitpid};
    use nix::unistd::Pid;

    use heartwood_cli::seams::{Env, ProcessOps};
    use heartwood_cli::supervisor::VERSION;
    use heartwood_cli::supervisor::signals::is_locked;
    use heartwood_cli::{ProcessError, process};

    use super::*;

    const CHILD: &str = env!("CARGO_BIN_EXE_test-child");

    /// Kills the instance's processes when dropped, so that a failed test leaves none behind.
    struct Cleanup<'a>(&'a Instance);

    impl Drop for Cleanup<'_> {
        fn drop(&mut self) {
            let _ = process::kill(self.0);
        }
    }

    fn argv(args: &[&str]) -> Vec<OsString> {
        std::iter::once(CHILD)
            .chain(args.iter().copied())
            .map(OsString::from)
            .collect()
    }

    fn env(pairs: &[(&str, &str)]) -> Env {
        pairs.iter().map(|(k, v)| (k.into(), v.into())).collect()
    }

    fn wait_for(what: &str, mut done: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !done() {
            assert!(Instant::now() < deadline, "{what} did not happen");
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn online(instance: &Instance) -> StatusRow {
        wait_for("the node online", || {
            let row = process::status(instance).unwrap();
            row.status == SupervisedStatus::Online && row.pid.is_some()
        });
        process::status(instance).unwrap()
    }

    #[test]
    fn start_refuses_a_running_role_and_stop_restart_and_log_refuse_a_stopped_one() {
        let root = scratch("refusals");
        let relay = instance(&root, Role::Relay);
        let _cleanup = Cleanup(&relay);
        let row = process::status(&relay).unwrap();
        assert_eq!(
            (row.status, row.pid, row.restarts, row.version.as_str()),
            (SupervisedStatus::Stopped, None, 0, VERSION)
        );
        let not_running = |result| matches!(result, Err(ProcessError::NotRunning { name }) if name == "node1-devnet-relay");
        assert!(not_running(process::stop(&relay)));
        assert!(not_running(process::restart(&relay, &env(&[]))));
        assert!(not_running(process::log(&relay, 15, false, false)));
        assert!(not_running(process::kill(&relay)));

        process::start(&relay, &argv(&["run"]), &env(&[]), true).unwrap();
        let row = online(&relay);
        assert_eq!(row.name, "node1-devnet-relay");
        assert!(row.uptime.is_some() && row.cpu_percent.is_some() && row.rss_bytes.is_some());
        let again = process::start(&relay, &argv(&["run"]), &env(&[]), true);
        assert!(matches!(again, Err(ProcessError::AlreadyRunning { .. })));
        process::log(&relay, 15, false, false).unwrap();
        process::log(&relay, 15, true, false).unwrap();
        process::stop(&relay).unwrap();
        assert_eq!(
            process::status(&relay).unwrap().status,
            SupervisedStatus::Stopped
        );
        assert!(not_running(process::stop(&relay)));
        // The captures stay readable after the stop.
        process::log(&relay, 15, false, false).unwrap();

        let run_dir = relay.run_dir();
        let mode =
            std::os::unix::fs::PermissionsExt::mode(&fs::metadata(&run_dir).unwrap().permissions());
        assert_eq!(mode & 0o777, 0o700);
    }

    #[test]
    fn the_devnet_starts_a_node_that_holds_its_lock_and_kills_it_as_a_crash_does() {
        let root = scratch("devnet-ops");
        let relay = instance(&root, Role::Relay);
        let _cleanup = Cleanup(&relay);
        let ops = Supervisors {
            binary: CHILD.into(),
        };
        let ops: &dyn ProcessOps = &ops;
        ops.start(&relay, &env(&[("HW_DEVNET", "1")])).unwrap();
        let row = online(&relay);
        wait_for("the node's lock", || is_locked(&relay.lock_path()).unwrap());
        let pid = row.pid.unwrap();
        let written: u32 = fs::read_to_string(relay.pid_path())
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(written, pid);
        assert_eq!(ops.status(&relay).unwrap().pid, Some(pid));
        let state = SupervisorState::read(&relay.state_path()).unwrap().unwrap();
        assert_eq!(
            state.argv[1..],
            ["relay", "run", "--token", "node1", "--network", "devnet"]
        );

        ops.kill(&relay).unwrap();
        assert!(!is_locked(&relay.supervisor_lock_path()).unwrap());
        assert!(!is_locked(&relay.lock_path()).unwrap());
        thread::sleep(Duration::from_millis(1500));
        let row = ops.status(&relay).unwrap();
        assert_eq!((row.status, row.pid), (SupervisedStatus::Stopped, None));
        let state = SupervisorState::read(&relay.state_path()).unwrap().unwrap();
        assert_eq!(state.last_exit.as_deref(), Some("signal: 9 (SIGKILL)"));
        assert!(matches!(
            ops.kill(&relay),
            Err(ProcessError::NotRunning { .. })
        ));
        assert!(matches!(
            ops.stop(&relay),
            Err(ProcessError::NotRunning { .. })
        ));

        // Started again, it stops gracefully.
        ops.start(&relay, &env(&[])).unwrap();
        online(&relay);
        ops.stop(&relay).unwrap();
        assert!(!is_locked(&relay.lock_path()).unwrap());
    }

    #[test]
    fn restart_runs_the_recorded_command_line_with_the_callers_environment() {
        let root = scratch("restart");
        let forger = instance(&root, Role::Forger);
        let _cleanup = Cleanup(&forger);
        let args = argv(&["env", "HW_TEST_VAR"]);
        process::start(&forger, &args, &env(&[("HW_TEST_VAR", "first")]), true).unwrap();
        online(&forger);
        let before = SupervisorState::read(&forger.state_path())
            .unwrap()
            .unwrap();
        process::restart(&forger, &env(&[("HW_TEST_VAR", "second")])).unwrap();
        online(&forger);
        let after = SupervisorState::read(&forger.state_path())
            .unwrap()
            .unwrap();
        assert_ne!(after.supervisor_pid, before.supervisor_pid);
        assert_eq!(after.argv, before.argv);
        wait_for("the second run's line", || {
            fs::read_to_string(forger.out_log_path())
                .unwrap()
                .contains("HW_TEST_VAR=second")
        });
        let out = fs::read_to_string(forger.out_log_path()).unwrap();
        assert!(out.contains("HW_TEST_VAR=first\n") && out.contains("HW_TEST_VAR=second\n"));
        process::stop(&forger).unwrap();
    }

    /// Where `SIGINT` is ignored, as a background job of a non-interactive shell has it and
    /// every child inherits it: the variable that tells the test below it runs under such a shell.
    const IGNORED: &str = "HW_TEST_SIGINT_IGNORED";

    /// The test child started as a node without a supervisor, its output at `/dev/null` and its
    /// `SIGINT` at the default whatever the caller ignores: a `Command` leaves an ignored signal
    /// ignored, and `pre_exec` is unsafe, which the workspace forbids.
    fn spawn_unsupervised(relay: &Instance) -> Pid {
        let args = [CHILD, "relay", "run"].map(|arg| CString::new(arg).unwrap());
        let paths = path_env(relay);
        let inherited = std::env::vars_os().filter(|(key, _)| !paths.iter().any(|(k, _)| k == key));
        let pair = |(key, value): (OsString, OsString)| {
            CString::new([key.as_bytes(), b"=", value.as_bytes()].concat()).unwrap()
        };
        let env: Vec<CString> = inherited.chain(paths.clone()).map(pair).collect();
        let mut actions = PosixSpawnFileActions::init().unwrap();
        let null = fs::OpenOptions::new()
            .write(true)
            .open("/dev/null")
            .unwrap();
        for fd in [1, 2] {
            actions.add_dup2(null.as_raw_fd(), fd).unwrap();
        }
        let mut attributes = PosixSpawnAttr::init().unwrap();
        attributes.set_sigdefault(&SigSet::from(SIGINT)).unwrap();
        attributes
            .set_flags(PosixSpawnFlags::POSIX_SPAWN_SETSIGDEF)
            .unwrap();
        posix_spawnp(&args[0], &actions, &attributes, &args, &env).unwrap()
    }

    /// A node started without a supervisor, which `stop` ends with `SIGINT`, named after
    /// `scratch_name`.
    fn an_unsupervised_node_is_stopped_by_sigint(scratch_name: &str) {
        let root = scratch(scratch_name);
        let relay = instance(&root, Role::Relay);
        let _cleanup = Cleanup(&relay);
        fs::create_dir_all(relay.run_dir()).unwrap();
        let node = spawn_unsupervised(&relay);
        wait_for("the node's lock", || is_locked(&relay.lock_path()).unwrap());
        wait_for("the pid file", || relay.pid_path().exists());
        let row = process::status(&relay).unwrap();
        assert_eq!(
            (row.status, row.pid),
            (
                SupervisedStatus::OnlineUnsupervised,
                Some(node.as_raw() as u32)
            )
        );
        assert!(row.rss_bytes.is_some() && row.uptime.is_none());
        let refused = process::start(&relay, &argv(&["run"]), &env(&[]), true);
        assert!(matches!(refused, Err(ProcessError::AlreadyRunning { .. })));
        process::stop(&relay).unwrap();
        let ended = waitpid(node, None).unwrap();
        assert!(
            matches!(ended, WaitStatus::Signaled(_, SIGINT, _)),
            "{ended:?}"
        );
        assert_eq!(
            process::status(&relay).unwrap().status,
            SupervisedStatus::Stopped
        );
    }

    #[test]
    fn a_node_without_a_supervisor_is_online_unsupervised_and_stop_signals_it() {
        an_unsupervised_node_is_stopped_by_sigint("unsupervised");
    }

    #[test]
    fn stop_ends_a_node_by_sigint_although_the_caller_ignores_it() {
        let name = "with_child::stop_ends_a_node_by_sigint_although_the_caller_ignores_it";
        if std::env::var_os(IGNORED).is_none() {
            // Run again, alone, under a shell that ignores SIGINT before it starts the test.
            let shell = "trap '' INT; exec \"$0\" \"$@\"";
            let run = Command::new("sh")
                .args(["-c", shell])
                .arg(std::env::current_exe().unwrap())
                .args([name, "--exact"])
                .env(IGNORED, "1")
                .output()
                .unwrap();
            let out = String::from_utf8_lossy(&run.stdout);
            let err = String::from_utf8_lossy(&run.stderr);
            assert!(
                run.status.success() && out.contains("1 passed"),
                "{out}{err}"
            );
            return;
        }
        let status = fs::read_to_string("/proc/self/status").unwrap();
        let ignored = status.lines().find_map(|line| line.strip_prefix("SigIgn:"));
        let ignored = u64::from_str_radix(ignored.unwrap().trim(), 16).unwrap();
        assert_ne!(
            ignored & (1 << 1),
            0,
            "SIGINT is not ignored under the shell"
        );
        an_unsupervised_node_is_stopped_by_sigint("unsupervised-ignored");
    }

    #[test]
    fn a_supervisor_keeps_to_the_run_directory_it_checked() {
        let root = scratch("supervisor-swap");
        let relay = instance(&root, Role::Relay);
        let _cleanup = Cleanup(&relay);
        process::start(&relay, &argv(&["run"]), &env(&[]), true).unwrap();
        online(&relay);
        let state = SupervisorState::read(&relay.state_path()).unwrap().unwrap();
        // The directory is swapped for a link to another while the supervisor runs.
        let (run, checked) = (relay.run_dir(), root.join("checked"));
        let elsewhere = root.join("elsewhere");
        fs::create_dir(&elsewhere).unwrap();
        fs::rename(&run, &checked).unwrap();
        std::os::unix::fs::symlink(&elsewhere, &run).unwrap();
        let supervisor = nix::unistd::Pid::from_raw(state.supervisor_pid.try_into().unwrap());
        nix::sys::signal::kill(supervisor, nix::sys::signal::SIGTERM).unwrap();
        let lock = checked.join("relay.supervisor.lock");
        wait_for("the supervisor's stop", || !is_locked(&lock).unwrap());
        let stopped = SupervisorState::read(&checked.join("relay.state.json"));
        assert_eq!(stopped.unwrap().unwrap().status, SupervisedStatus::Stopped);
        let landed: Vec<_> = fs::read_dir(&elsewhere).unwrap().collect();
        assert!(
            landed.is_empty(),
            "files landed in the link's target: {landed:?}"
        );
    }

    #[test]
    fn of_two_daemon_starts_at_once_only_the_one_whose_supervisor_holds_the_lock_succeeds() {
        let root = scratch("two-starts");
        let relay = instance(&root, Role::Relay);
        let _cleanup = Cleanup(&relay);
        for _ in 0..3 {
            let barrier = std::sync::Barrier::new(2);
            let results: Vec<_> = thread::scope(|scope| {
                let start = || {
                    barrier.wait();
                    process::start(&relay, &argv(&["run"]), &env(&[]), true)
                };
                let starts = [scope.spawn(start), scope.spawn(start)];
                starts.map(|start| start.join().unwrap())
            })
            .into();
            let started = results.iter().filter(|result| result.is_ok()).count();
            assert_eq!(started, 1, "{results:?}");
            assert!(
                (results.iter())
                    .any(|result| matches!(result, Err(ProcessError::AlreadyRunning { .. }))),
                "{results:?}"
            );
            online(&relay);
            process::stop(&relay).unwrap();
        }
    }

    #[test]
    fn a_foreground_start_returns_when_its_supervisor_does() {
        let root = scratch("foreground");
        let relay = instance(&root, Role::Relay);
        let _cleanup = Cleanup(&relay);
        let began = Instant::now();
        let result = process::start(&relay, &argv(&["exit", "1"]), &env(&[]), false);
        assert!(
            matches!(result, Err(ProcessError::Spawn { .. })),
            "{result:?}"
        );
        assert!(began.elapsed() < Duration::from_secs(10));
        let row = process::status(&relay).unwrap();
        assert_eq!((row.status, row.restarts), (SupervisedStatus::Errored, 4));
    }
}
