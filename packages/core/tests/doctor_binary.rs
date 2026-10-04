//! The `heartwood doctor` binary: on a stopped instance and on a forging devnet node it prints
//! every check once, in JSON or text, with the exit code its findings give; it exits with 3 when
//! it cannot run; `doctor mcp` over a pipe writes nothing but protocol and ends with its input;
//! and under `strace` it opens no key file, writes nothing in the node's directories and connects
//! only to loopback addresses (skipped with a message when `strace` is not installed).

mod doctor_support;

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use doctor_support::{AUX, ENV_CANARY, Node, heartwood, program};
use heartwood_kernel::Clock;
use serde_json::Value;

/// A node with both roles present (each role's lock file) and a private key file, and the
/// variables of its directories, home and user.
fn stopped(name: &str) -> (Node, Vec<(String, String)>) {
    let node = Node::new(name, "CORE_P2P_PORT=6102\nCORE_API_PORT=6103\n");
    let run = node.paths().data.join("run");
    std::fs::create_dir_all(&run).unwrap();
    node.chmod(&run, 0o700);
    std::fs::write(run.join("relay.lock"), "").unwrap();
    std::fs::write(run.join("forger.lock"), "").unwrap();
    let key = node.config().join(heartwood_forger::keys::FILE);
    std::fs::write(&key, format!("{{\"canary\": \"{ENV_CANARY}\"}}")).unwrap();
    node.chmod(&key, 0o600);
    let mut vars = node.process();
    vars.push(("HOME".into(), node.home().display().to_string()));
    vars.push(("USER".into(), "user".into()));
    (node, vars)
}

fn code(output: &Output) -> i32 {
    output.status.code().unwrap_or(-1)
}

fn report(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap()
}

/// The exit code a report's summary gives, with `--fail-on fail` or not.
fn expected(report: &Value, fail_only: bool) -> i32 {
    let count = |status: &str| report["summary"][status].as_u64().unwrap();
    match (count("fail"), count("warn")) {
        (fail, _) if fail > 0 => 2,
        (_, warn) if warn > 0 && !fail_only => 1,
        _ => 0,
    }
}

/// The checks a report has findings of: every check appears at least once.
fn checks(report: &Value) -> usize {
    let findings = report["findings"].as_array().unwrap().iter();
    let ids: std::collections::BTreeSet<&str> =
        findings.map(|f| f["id"].as_str().unwrap()).collect();
    ids.len()
}

/// Nothing secret in `output`.
fn clean(output: &Output) {
    for stream in [&output.stdout, &output.stderr] {
        let text = String::from_utf8_lossy(stream);
        assert!(!text.contains(ENV_CANARY) && !text.contains(AUX), "{text}");
    }
}

#[test]
fn on_a_stopped_instance_every_check_is_reported_with_its_exit_code() {
    let (_node, vars) = stopped("binary-stopped");
    let run = |more: &[&str]| {
        let args = [
            &["doctor", "--token", "solar", "--network", "testnet"],
            more,
        ]
        .concat();
        let output = heartwood(&args, &vars);
        clean(&output);
        output
    };
    let json = run(&["--json"]);
    let found = report(&json);
    assert_eq!(found["format"], "heartwood-doctor-report/1");
    assert_eq!(found["redaction"]["addresses"], true);
    let findings = found["findings"].as_array().unwrap();
    assert_eq!(checks(&found), 47);
    let counts = ["pass", "warn", "fail", "unknown", "skipped"];
    let total: u64 = counts
        .iter()
        .map(|s| found["summary"][s].as_u64().unwrap())
        .sum();
    assert_eq!(usize::try_from(total).unwrap(), findings.len());
    assert_eq!(code(&json), expected(&found, false));
    assert_eq!(
        code(&run(&["--json", "--fail-on", "fail"])),
        expected(&found, true)
    );
    let text = run(&[]);
    let printed = String::from_utf8_lossy(&text.stdout);
    assert!(
        printed.starts_with("heartwood doctor: ") && printed.contains("Summary: "),
        "{printed}"
    );
    assert_eq!(code(&text), code(&json));
    let one = report(&run(&["--json", "--check", "keys.file"]));
    assert_eq!(one["findings"].as_array().unwrap().len(), 1);
    let relay = report(&run(&["--json", "--role", "relay"]));
    for finding in relay["findings"].as_array().unwrap() {
        if finding["subject"] == "forger" {
            assert_eq!(finding["status"], "skipped", "{finding}");
        }
    }
}

#[test]
fn it_exits_with_3_when_it_cannot_run_and_its_lists_work() {
    let (node, vars) = stopped("binary-cannot-run");
    for args in [
        &["doctor", "--network", "testnet", "--bogus"][..],
        &["doctor", "--network", "testnet", "--window", "3m"],
        &["doctor", "explain", "no.such-check"],
        &["doctor", "--network", "testnet", "--check", "nope"],
    ] {
        assert_eq!(code(&heartwood(args, &vars)), 3, "{args:?}");
    }
    // No network named anywhere: no instance, and standard error names the file without the
    // home directory or the user's name.
    let home = node.home().display().to_string();
    let user = ("USER".to_owned(), "binary-cannot-run".to_owned());
    for args in [&["doctor", "--json"][..], &["doctor", "mcp"]] {
        let none = heartwood(args, &[("HOME".into(), home.clone()), user.clone()]);
        assert_eq!(code(&none), 3, "{args:?}");
        assert!(none.stdout.is_empty());
        let said = String::from_utf8_lossy(&none.stderr);
        let file = "~/.config/solar-core/config.json";
        assert!(said.contains("no network") && said.contains(file), "{said}");
        assert!(!said.contains(&home) && !said.contains(&user.1), "{said}");
    }
    assert_eq!(code(&heartwood(&["doctor", "--help"], &vars)), 0);
    let checks = heartwood(&["doctor", "checks", "--json"], &vars);
    assert_eq!(code(&checks), 0);
    assert_eq!(report(&checks)["checks"].as_array().unwrap().len(), 47);
    let explain = heartwood(&["doctor:explain", "keys.file"], &vars);
    assert_eq!(code(&explain), 0);
    assert!(String::from_utf8_lossy(&explain.stdout).starts_with("keys.file: "));
    let explain = heartwood(&["doctor", "explain", "disk.space"], &vars);
    let text = String::from_utf8_lossy(&explain.stdout);
    assert!(
        text.contains("\nWord (planning): the planning figure"),
        "{text}"
    );
}

#[test]
fn without_a_home_directory_it_exits_with_3_and_looks_up_no_name() {
    let (node, mut vars) = stopped("binary-no-home");
    vars.retain(|(name, _)| name != "HOME");
    let args = [
        "doctor",
        "--token",
        "solar",
        "--network",
        "testnet",
        "--json",
    ];
    let empty = [vars.clone(), vec![("HOME".into(), String::new())]].concat();
    for vars in [&vars, &empty] {
        let output = heartwood(&args, vars);
        assert_eq!(code(&output), 3);
        let said = String::from_utf8_lossy(&output.stderr);
        assert_eq!(said, "heartwood doctor: HOME is not set\n");
    }
    // The home is not looked up in the user database, through its daemon or its files.
    let log = node.root.join("..").join("binary-no-home.trace");
    let Some(output) = traced(&args, &vars, &log) else {
        return;
    };
    assert_eq!(code(&output), 3);
    let text = std::fs::read_to_string(&log).unwrap();
    for lookup in ["nscd", "/etc/passwd", "/etc/nsswitch.conf", "/var/lib/sss"] {
        assert!(!text.contains(lookup), "{lookup} in the trace");
    }
    let broken = broken(&log, &node.root, None, None);
    assert!(broken.is_empty(), "{broken:#?}");
}

/// `heartwood doctor mcp` on the stopped instance of `vars`, given `input` then the end of its
/// input; its output, after at most 10 s.
fn mcp(vars: &[(String, String)], input: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_heartwood"))
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .envs(vars.iter().map(|(k, v)| (k, v)))
        .args(["doctor", "mcp", "--token", "solar", "--network", "testnet"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let _ = stdin.write_all(input.as_bytes());
    drop(stdin);
    let clock = heartwood_kernel::SystemClock;
    let start = clock.now_ms();
    while child.try_wait().unwrap().is_none() {
        if clock.now_ms() - start > 10_000 {
            // Our own child, by its pid.
            let _ = child.kill();
            panic!("doctor mcp did not end with its input");
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    child.wait_with_output().unwrap()
}

#[test]
fn doctor_mcp_over_a_pipe_writes_only_protocol_and_ends_with_its_input() {
    let (_node, vars) = stopped("binary-mcp-end");
    let output = mcp(&vars, "");
    clean(&output);
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let message: Value = serde_json::from_str(line).unwrap();
        assert_eq!(message["jsonrpc"], "2.0", "{line}");
    }
}

#[test]
#[ignore = "integration"]
fn doctor_mcp_answers_a_client_over_a_pipe() {
    let (_node, vars) = stopped("binary-mcp");
    let input = concat!(
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"test","version":"1"}}}"#,
        "\n",
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        "\n",
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
        "\n",
    );
    let output = mcp(&vars, input);
    clean(&output);
    assert_eq!(code(&output), 0);
    let text = String::from_utf8_lossy(&output.stdout);
    let answers: Vec<Value> = text
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(answers.len(), 2, "{text}");
    assert_eq!(answers[0]["id"], 1);
    assert_eq!(answers[0]["result"]["protocolVersion"], "2025-11-25");
    assert_eq!(answers[1]["id"], 2);
    assert_eq!(answers[1]["result"]["tools"].as_array().unwrap().len(), 5);
}

/// `heartwood <args>` under `strace`, its trace in `log`; `None`, with a message, without strace.
fn traced(args: &[&str], vars: &[(String, String)], log: &Path) -> Option<Output> {
    let Some(strace) = program("strace") else {
        eprintln!("strace is not installed: the traced run is skipped");
        return None;
    };
    // `-v` prints each environment in full, so that a probe's can be read, not only counted.
    let trace = [
        "-f",
        "-qq",
        "-v",
        "-y",
        "-s",
        "512",
        "-e",
        "trace=%file,%network",
        "-o",
    ];
    let output = Command::new(strace)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .envs(vars.iter().map(|(k, v)| (k, v)))
        .args(trace)
        .arg(log)
        .arg(env!("CARGO_BIN_EXE_heartwood"))
        .args(args)
        .output()
        .unwrap();
    Some(output)
}

/// The paths outside the node's directories that the doctor's own process may open: `/proc`,
/// `/sys`, the firewall's and the clock's files, and what the program itself loads.
const SYSTEM_READS: [&str; 11] = [
    "/proc/",
    "/sys/",
    "/etc/ufw/ufw.conf",
    "/run/systemd/timesync/synchronized",
    "/dev/null",
    "/dev/urandom",
    "/etc/ld.so.cache",
    "/lib/",
    "/lib64/",
    "/usr/lib/",
    "/usr/lib64/",
];

/// Whether `path` is another process's environment or memory, or one of its threads':
/// `/proc/<pid>/environ`, `/proc/<pid>/mem`, `/proc/<pid>/task/<tid>/environ` or `.../mem`. No
/// read of the doctor's may reach them, although the rest of `/proc` is open to it.
fn process_secret(path: &str) -> bool {
    let Some(rest) = path.strip_prefix("/proc/") else {
        return false;
    };
    let parts: Vec<&str> = rest.split('/').collect();
    let secret = |name: &str| name == "environ" || name == "mem";
    let number = |part: &str| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit());
    match parts.as_slice() {
        [pid, name] => !pid.is_empty() && secret(name),
        [pid, "task", tid, name] => !pid.is_empty() && number(tid) && secret(name),
        _ => false,
    }
}

/// The environment a probe's `execve` line passes, as `strace -v` prints it: the text after its
/// argument list, up to the call's end.
fn exec_environment(line: &str) -> &str {
    let end = [") = ", " <unfinished"]
        .iter()
        .filter_map(|mark| line.rfind(mark))
        .min()
        .unwrap_or(line.len());
    let call = &line[..end];
    call.rsplit_once("], ")
        .map_or(call, |(_, environment)| environment)
}

/// The file a successful open of a trace line opened: the path its descriptor's decoration names
/// (`= 3</path>`), else the first quoted path.
fn opened(line: &str) -> Option<String> {
    let (_, result) = line.rsplit_once(" = ")?;
    if result.starts_with('-') {
        return None;
    }
    let decorated = result.split_once('<').map(|(_, rest)| rest);
    let decorated = decorated.map(|rest| rest.strip_suffix('>').unwrap_or(rest));
    let decorated = decorated.map(|path| path.split("<char ").next().unwrap_or(path));
    let quoted = || line.split('"').nth(1);
    decorated.or_else(quoted).map(str::to_owned)
}

/// The system calls of the trace at `log` that break the doctor's rules: an open, or any call
/// but a metadata read, of the key file; an open for writing, a creation, removal, rename or mode
/// change under `root`; a successful open by the doctor's own process of a file outside `root`
/// and [`SYSTEM_READS`], or any open by it of another process's environment or memory
/// ([`process_secret`]), failed or not;
/// a probe's `execve` with an environment other than `LANG=C` alone; a connection to an address that is not loopback (`api`, when given, is
/// the only IP address and port allowed), or to a local socket that is not the system bus's.
/// While a relay runs, `store` names its store: SQLite's read-only connection opens the store's
/// existing `-wal` and `-shm` read-write with the create flag, and never truncates them.
fn broken(log: &Path, root: &Path, api: Option<(&str, u16)>, store: Option<&Path>) -> Vec<String> {
    let text = std::fs::read_to_string(log).unwrap();
    broken_in(&text, root, api, store)
}

/// [`broken`] over the text of a trace.
fn broken_in(
    text: &str,
    root: &Path,
    api: Option<(&str, u16)>,
    store: Option<&Path>,
) -> Vec<String> {
    let root = root.display().to_string();
    // The first line is the doctor's own execve.
    let doctor = text
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().next());
    let program = env!("CARGO_BIN_EXE_heartwood");
    let inside = |path: &str| {
        if process_secret(path) {
            return false;
        }
        path.starts_with(&format!("{root}/"))
            || path == program
            || SYSTEM_READS
                .iter()
                .any(|allowed| match allowed.ends_with('/') {
                    true => path.starts_with(allowed),
                    false => path == *allowed,
                })
    };
    let metadata = [
        "newfstatat",
        "fstatat64",
        "lstat",
        "stat",
        "statx",
        "readlink",
        "readlinkat",
    ];
    let changes = [
        "unlink", "rename", "mkdir", "rmdir", "mknod", "chmod", "fchmod", "chown", "fchown",
        "lchown", "symlink", "link", "truncate", "utime",
    ];
    let mut broken = Vec::new();
    for line in text.lines() {
        let call = line.split_whitespace().nth(1).unwrap_or_default();
        let name = call.split('(').next().unwrap_or_default();
        if line.contains(heartwood_forger::keys::FILE) && !metadata.contains(&name) {
            broken.push(line.to_owned());
        }
        let writes = ["O_WRONLY", "O_RDWR", "O_CREAT", "O_TRUNC"];
        let opens = name.starts_with("open") || name == "creat";
        let sqlite = store.is_some_and(|store| {
            let store = store.display();
            let own = [format!("\"{store}-wal\""), format!("\"{store}-shm\"")];
            own.iter().any(|file| line.contains(file.as_str()))
                && line.contains("O_RDWR")
                && !["O_WRONLY", "O_TRUNC", "O_APPEND", "O_TMPFILE"]
                    .iter()
                    .any(|flag| line.contains(flag))
        });
        let written = writes.iter().any(|flag| line.contains(flag));
        if line.contains(&root) && opens && written && !sqlite {
            broken.push(line.to_owned());
        }
        let own = doctor.is_some_and(|pid| line.split_whitespace().next() == Some(pid));
        // Another process's environment or memory may not even be tried, whatever the result.
        let secret = line.split('"').nth(1).is_some_and(process_secret);
        if own && opens && (secret || opened(line).is_some_and(|path| !inside(&path))) {
            broken.push(line.to_owned());
        }
        // The doctor's children are its probes, each run with `LANG=C` alone.
        if !own && name == "execve" && exec_environment(line) != r#"["LANG=C"]"# {
            broken.push(line.to_owned());
        }
        if line.contains(&root) && changes.iter().any(|change| name.starts_with(change)) {
            broken.push(line.to_owned());
        }
        if name == "connect" {
            let inet = line.contains("AF_INET");
            let loopback = [
                "inet_addr(\"127.",
                "inet_pton(AF_INET6, \"::1\"",
                "\"::ffff:127.",
            ];
            let allowed = match (inet, api) {
                (true, Some((address, port))) => {
                    let address = format!("inet_addr(\"{address}\")");
                    line.contains(&address) && line.contains(&format!("htons({port})"))
                }
                (true, None) => loopback.iter().any(|l| line.contains(l)),
                (false, _) => ["/run/dbus/", "/var/run/dbus/", "/run/systemd/"]
                    .iter()
                    .any(|path| line.contains(path)),
            };
            if !allowed {
                broken.push(line.to_owned());
            }
        }
    }
    broken
}

#[test]
fn the_trace_rules_catch_a_read_outside_the_node_and_the_system_files() {
    let root = Path::new("/nodes/node1");
    let trace = [
        r#"4100 execve("/usr/local/bin/heartwood", ["heartwood", "doctor"], 0x7ffd /* 3 vars */) = 0"#,
        r#"4100 openat(AT_FDCWD, "/nodes/node1/config/app.json", O_RDONLY|O_CLOEXEC) = 3</nodes/node1/config/app.json>"#,
        r#"4100 openat(AT_FDCWD, "/proc/self/status", O_RDONLY|O_CLOEXEC) = 3</proc/4100/status>"#,
        r#"4100 openat(AT_FDCWD, "/dev/null", O_RDWR|O_CLOEXEC) = 3</dev/null<char 1:3>>"#,
        r#"4100 openat(AT_FDCWD, "/users/op/.config/solar-core/config.json", O_RDONLY|O_CLOEXEC) = -1 ENOENT (No such file or directory)"#,
        r#"4101 openat(AT_FDCWD, "/etc/systemd/system.conf", O_RDONLY|O_CLOEXEC) = 3</etc/systemd/system.conf>"#,
        r#"4101 execve("/usr/bin/timedatectl", ["/usr/bin/timedatectl", "show", "--property=NTPSynchronized", "--property=NTP"], ["LANG=C"]) = 0"#,
        r#"4102 execve("/usr/bin/chronyc", ["/usr/bin/chronyc", "-n", "tracking"], ["LANG=C"] <unfinished ...>"#,
    ];
    let clean = trace.join("\n");
    assert_eq!(broken_in(&clean, root, None, None), Vec::<String>::new());
    let outside = [
        r#"4100 openat(AT_FDCWD, "/users/op/.ssh/id_ed25519", O_RDONLY|O_CLOEXEC) = 4</users/op/.ssh/id_ed25519>"#,
        r#"4100 openat(AT_FDCWD, "/etc/shadow", O_RDONLY|O_CLOEXEC) = 4</etc/shadow>"#,
        r#"4100 openat(AT_FDCWD, "/etc/passwd", O_RDONLY|O_CLOEXEC) = 4</etc/passwd>"#,
        r#"4100 openat(AT_FDCWD, "/nodes/node1-other/config/app.json", O_RDONLY|O_CLOEXEC) = 4</nodes/node1-other/config/app.json>"#,
        r#"4100 openat(AT_FDCWD, "/proc/4242/environ", O_RDONLY|O_CLOEXEC) = 4</proc/4242/environ>"#,
        r#"4100 openat(AT_FDCWD, "/proc/4242/task/4243/mem", O_RDONLY|O_CLOEXEC) = 4</proc/4242/task/4243/mem>"#,
        r#"4100 openat(AT_FDCWD, "/proc/1/environ", O_RDONLY|O_CLOEXEC) = -1 EACCES (Permission denied)"#,
        r#"4100 openat(AT_FDCWD, "/proc/4242/mem", O_RDONLY|O_CLOEXEC <unfinished ...>"#,
        r#"4101 execve("/usr/bin/systemctl", ["/usr/bin/systemctl", "is-active", "ufw", "nftables", "firewalld"], ["LANG=C", "HOME=/users/op"]) = 0"#,
        r#"4101 execve("/usr/bin/systemctl", ["/usr/bin/systemctl", "is-active", "ufw", "nftables", "firewalld"], 0x7ffd /* 1 var */) = 0"#,
    ];
    for line in outside {
        let text = format!("{clean}\n{line}");
        assert_eq!(broken_in(&text, root, None, None), [line], "{line}");
    }
}

#[test]
fn under_strace_it_opens_no_key_file_writes_nothing_and_connects_only_to_loopback() {
    let (node, vars) = stopped("binary-strace");
    // A link with an innocent name, where the relay's log is read, pointing at the key file.
    let log_dir = node.paths().log;
    std::fs::create_dir_all(&log_dir).unwrap();
    let key = node.config().join(heartwood_forger::keys::FILE);
    std::os::unix::fs::symlink(&key, log_dir.join("solar-testnet-relay-current.log")).unwrap();
    let log = node.root.join("..").join("binary-strace.trace");
    let args = [
        "doctor",
        "--token",
        "solar",
        "--network",
        "testnet",
        "--json",
    ];
    let Some(output) = traced(&args, &vars, &log) else {
        return;
    };
    clean(&output);
    assert!(report(&output)["findings"].as_array().is_some());
    let broken = broken(&log, &node.root, None, None);
    assert!(broken.is_empty(), "{broken:#?}");
    let text = std::fs::read_to_string(&log).unwrap();
    assert!(text.contains("execve("), "the trace is empty");
}

/// `heartwood devnet <args> --dir <root>`, which must succeed.
fn devnet(root: &Path, args: &[&str]) {
    let output = Command::new(env!("CARGO_BIN_EXE_heartwood"))
        .arg("devnet")
        .args(args)
        .arg("--dir")
        .arg(root)
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "devnet {args:?}: {text}");
}

/// Stops the devnet of its root when dropped, the test failed or not.
struct Stop<'a>(&'a Path);

impl Drop for Stop<'_> {
    fn drop(&mut self) {
        let _ = Command::new(env!("CARGO_BIN_EXE_heartwood"))
            .args(["devnet", "stop", "--dir"])
            .arg(self.0)
            .output();
    }
}

#[test]
#[ignore = "devnet"]
fn on_a_forging_devnet_node_it_reports_and_touches_no_key_under_strace() {
    let prefix = std::env::var("HEARTWOOD_DEVNET_PREFIX").unwrap_or_else(|_| "127.24.0".into());
    let root = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("doctor")
        .join("binary-devnet");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("home")).unwrap();
    let init = [
        "init",
        "--validators",
        "1",
        "--address-prefix",
        &prefix,
        "--epoch-delay",
        "5",
    ];
    devnet(&root, &init);
    let node = root.join("node1");
    // Whatever the umask, no other user can write the configuration directory or its parent,
    // so the doctor runs the node's loaders (keys.dir).
    for dir in [node.clone(), node.join("config")] {
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let _stop = Stop(&root);
    devnet(&root, &["start"]);
    devnet(&root, &["wait", "--height", "3", "--timeout", "150"]);
    let dirs = ["CONFIG", "DATA", "CACHE", "LOG", "TEMP"];
    let mut vars: Vec<(String, String)> = (dirs.iter())
        .map(|kind| {
            (
                format!("CORE_PATH_{kind}"),
                node.join(kind.to_lowercase()).display().to_string(),
            )
        })
        .collect();
    vars.push(("HOME".into(), root.join("home").display().to_string()));
    let args = [
        "doctor",
        "--token",
        "node1",
        "--network",
        "devnet",
        "--json",
    ];
    let output = heartwood(&args, &vars);
    let found = report(&output);
    assert_eq!(checks(&found), 47);
    assert_eq!(code(&output), expected(&found, false));
    // The loaders ran: a run that read nothing would pass the trace's rules without showing them.
    let keys_dir = found["findings"].as_array().unwrap().iter();
    let keys_dir: Vec<&Value> = keys_dir.filter(|f| f["id"] == "keys.dir").collect();
    let passed = !keys_dir.is_empty() && keys_dir.iter().all(|f| f["status"] == "pass");
    assert!(passed, "{keys_dir:?}");
    let log = root.join("doctor.trace");
    if let Some(output) = traced(&args, &vars, &log) {
        assert_eq!(checks(&report(&output)), 47);
        let api = format!("{prefix}.11");
        let store = node.join("data").join("database").join("blockchain.db");
        let broken = broken(&log, &node, Some((&api, 6003)), Some(&store));
        assert!(broken.is_empty(), "{broken:#?}");
        let text = std::fs::read_to_string(&log).unwrap();
        let to_api = format!("inet_addr(\"{api}\")");
        let asked = |line: &&str| {
            line.contains("connect(") && line.contains(&to_api) && line.contains("htons(6003)")
        };
        let asked = text.lines().filter(asked).count();
        assert!(asked > 0, "the doctor never asked its relay's API");
    }
    devnet(&root, &["stop"]);
}
