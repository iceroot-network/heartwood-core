//! The signals, in a child process (this test binary run again as `signal_child`): the first
//! SIGINT or SIGTERM starts the graceful shutdown, and a second one exits at once with the status
//! of a process ended by that signal.

use std::io::{BufRead, BufReader, Lines};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::time::Duration;

/// The variable that makes `signal_child` act, with its mode.
const CHILD: &str = "HEARTWOOD_SIGNAL_CHILD";

/// The child: it waits for the first signal, says so, then either exits 0 at once (`quick`) or
/// takes a long shutdown (`slow`) that only a second signal cuts short.
#[test]
fn signal_child() {
    let Ok(mode) = std::env::var(CHILD) else {
        return;
    };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let shutdown = tokio::spawn(heartwood_core::signals::shutdown_requested("relay"));
        tokio::time::sleep(Duration::from_millis(200)).await;
        println!("ready");
        shutdown.await.unwrap();
        println!("graceful");
        if mode == "slow" {
            tokio::time::sleep(Duration::from_secs(60)).await;
        }
    });
}

fn spawn(mode: &str) -> (Child, Lines<BufReader<ChildStdout>>) {
    let exe = std::env::current_exe().unwrap();
    let mut child = Command::new(exe)
        .args(["--exact", "signal_child", "--nocapture", "--test-threads=1"])
        .env(CHILD, mode)
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let lines = BufReader::new(child.stdout.take().unwrap()).lines();
    (child, lines)
}

fn wait_for(lines: &mut Lines<BufReader<ChildStdout>>, expected: &str) {
    for line in lines.by_ref() {
        if line.unwrap().ends_with(expected) {
            return;
        }
    }
    panic!("the child ended before it printed {expected}");
}

fn signal(child: &Child, name: &str) {
    let status = Command::new("kill")
        .args([&format!("-{name}"), &child.id().to_string()])
        .status()
        .unwrap();
    assert!(status.success());
}

#[test]
fn the_first_signal_starts_the_graceful_shutdown() {
    for name in ["INT", "TERM"] {
        let (mut child, mut lines) = spawn("quick");
        wait_for(&mut lines, "ready");
        signal(&child, name);
        wait_for(&mut lines, "graceful");
        assert_eq!(child.wait().unwrap().code(), Some(0), "{name}");
    }
}

#[test]
fn a_second_signal_exits_at_once() {
    for (name, code) in [("INT", 130), ("TERM", 143)] {
        let (mut child, mut lines) = spawn("slow");
        wait_for(&mut lines, "ready");
        signal(&child, name);
        wait_for(&mut lines, "graceful");
        signal(&child, name);
        assert_eq!(child.wait().unwrap().code(), Some(code), "{name}");
    }
}
