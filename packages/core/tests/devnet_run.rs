//! `heartwood devnet run`: the scenario runner takes the command's place, so the runner's exit
//! status is the command's and a signal sent to the command reaches the runner.

#![cfg(feature = "devnet")]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::Command;

use heartwood_cli::devnet::run::RUNNER;

#[test]
fn the_runner_replaces_the_command() {
    let dir = std::env::temp_dir().join(format!("hw-devnet-run-exec-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let record = dir.join("pid.txt");
    // A fake runner that records its own process id and exits 7.
    let runner = dir.join(RUNNER);
    let script = format!("#!/bin/sh\necho $$ > '{}'\nexit 7\n", record.display());
    fs::write(&runner, script).unwrap();
    fs::set_permissions(&runner, fs::Permissions::from_mode(0o755)).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_heartwood"))
        .args(["devnet", "--dir"])
        .arg(dir.join("devnet"))
        .args(["run", "scenario.json"])
        .env("PATH", &dir)
        .spawn()
        .unwrap();
    let pid = child.id();
    let status = child.wait().unwrap();
    assert_eq!(status.code(), Some(7));
    let recorded: u32 = fs::read_to_string(&record).unwrap().trim().parse().unwrap();
    assert_eq!(recorded, pid, "the runner ran in the command's own process");
    fs::remove_dir_all(&dir).unwrap();
}
