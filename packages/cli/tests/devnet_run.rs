//! `devnet run`: the scenario runner is exec'd by an explicit argv, and its absence is a typed
//! error.

#![cfg(feature = "devnet")]

use std::ffi::{OsStr, OsString};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use heartwood_cli::devnet::run::{BUILT_RUNNER, RUNNER, command, find, locate};
use heartwood_cli::errors::DevnetError;

/// A directory holding a fake runner that records its argv, one argument per line, and exits 7.
fn fake_runner(name: &str) -> (PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join(format!("hw-devnet-run-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let record = dir.join("argv.txt");
    let script = format!(
        "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nexit 7\n",
        record.display()
    );
    let runner = dir.join(RUNNER);
    fs::write(&runner, script).unwrap();
    fs::set_permissions(&runner, fs::Permissions::from_mode(0o755)).unwrap();
    (runner, record)
}

#[test]
fn execs_the_runner_by_an_explicit_argv() {
    let (runner, record) = fake_runner("argv");
    let scenario = Path::new("a scenario; touch pwned $(id).json");
    let args = [OsString::from("--level"), OsString::from("l2")];
    let status = command(&runner, Path::new("/tmp/dn root"), scenario, &args)
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(7));
    let argv = fs::read_to_string(record).unwrap();
    let want = "scenario\nrun\n--backend\nheartwood\n--devnet-root\n/tmp/dn root\n\
                a scenario; touch pwned $(id).json\n--level\nl2\n";
    assert_eq!(argv, want);
}

#[test]
fn finds_the_runner_in_the_first_directory_that_has_it() {
    let (runner, _) = fake_runner("find");
    let dir = runner.parent().unwrap().to_path_buf();
    let empty = std::env::temp_dir().join("hw-devnet-run-empty-dir");
    assert_eq!(find([empty.clone(), dir]), Ok(runner));
    assert_eq!(find([empty]), Err(DevnetError::RunnerMissing(RUNNER)));
}

#[test]
fn says_so_when_the_harness_is_not_installed() {
    let error = find([]).unwrap_err();
    assert_eq!(error, DevnetError::RunnerMissing(RUNNER));
    assert!(error.to_string().contains("not installed"));
}

#[test]
fn takes_the_harness_build_next_to_the_binary_but_not_from_path() {
    let (runner, _) = fake_runner("built");
    let dir = runner.parent().unwrap().to_path_buf();
    let built = dir.join(BUILT_RUNNER);
    fs::rename(&runner, &built).unwrap();
    assert_eq!(locate(Some(&dir), OsStr::new("")), Ok(built.clone()));
    let path = std::env::join_paths([&dir]).unwrap();
    assert_eq!(locate(None, &path), Err(DevnetError::RunnerMissing(RUNNER)));
    // The installed name comes first, from the directories of PATH too.
    let (installed, _) = fake_runner("installed");
    let path = std::env::join_paths([installed.parent().unwrap()]).unwrap();
    assert_eq!(locate(Some(&dir), &path), Ok(installed));
}
