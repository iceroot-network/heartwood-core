//! The `heartwood doctor` binary: on a stopped instance and on a forging devnet node it prints
//! every check once, in JSON or text, with the exit code its findings give; it exits with 3 when
//! it cannot run; `doctor mcp` over a pipe writes nothing but protocol and ends with its input;
//! and under `strace` it opens no key file, writes nothing in the node's directories and connects
//! only to loopback addresses (skipped with a message when `strace` is not installed).

mod doctor_support;

use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use doctor_support::{AUX, ENV_CANARY, Node, heartwood, program};
use heartwood_kernel::Clock;
use serde_json::Value;

/// A node with both roles present (a relay's lock file, a private key file), and the variables of
/// its directories, home and user.
fn stopped(name: &str) -> (Node, Vec<(String, String)>) {
    let node = Node::new(name, "CORE_P2P_PORT=6102\nCORE_API_PORT=6103\n");
    let run = node.paths().data.join("run");
    std::fs::create_dir_all(&run).unwrap();
    node.chmod(&run, 0o700);
    std::fs::write(run.join("relay.lock"), "").unwrap();
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
    // No network named anywhere: no instance.
    let home = vec![("HOME".into(), node.home().display().to_string())];
    let none = heartwood(&["doctor", "--json"], &home);
    assert_eq!(code(&none), 3);
    assert!(none.stdout.is_empty());
    assert_eq!(code(&heartwood(&["doctor", "--help"], &vars)), 0);
    let checks = heartwood(&["doctor", "checks", "--json"], &vars);
    assert_eq!(code(&checks), 0);
    assert_eq!(report(&checks)["checks"].as_array().unwrap().len(), 47);
    let explain = heartwood(&["doctor:explain", "keys.file"], &vars);
    assert_eq!(code(&explain), 0);
    assert!(String::from_utf8_lossy(&explain.stdout).starts_with("keys.file: "));
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
    let trace = [
        "-f",
        "-qq",
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

/// The system calls of the trace at `log` that break the doctor's rules: an open, or any call
/// but a metadata read, of the key file; an open for writing, a creation, removal, rename or mode
/// change under `root`; a connection to an address that is not loopback (`api`, when given, is
/// the only IP address and port allowed), or to a local socket that is not the system bus's.
fn broken(log: &Path, root: &Path, api: Option<(&str, u16)>) -> Vec<String> {
    let text = std::fs::read_to_string(log).unwrap();
    let root = root.display().to_string();
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
        if line.contains(&root) && opens && writes.iter().any(|flag| line.contains(flag)) {
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
    let broken = broken(&log, &node.root, None);
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
    let _stop = Stop(&root);
    devnet(&root, &["start"]);
    devnet(&root, &["wait", "--height", "3", "--timeout", "150"]);
    let node = root.join("node1");
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
    let log = root.join("doctor.trace");
    if let Some(output) = traced(&args, &vars, &log) {
        assert_eq!(checks(&report(&output)), 47);
        let api = format!("{prefix}.11");
        let broken = broken(&log, &node, Some((&api, 6003)));
        assert!(broken.is_empty(), "{broken:#?}");
    }
    devnet(&root, &["stop"]);
}
