//! `top` and the `env` commands through the `heartwood` binary, in a scratch home: the answer of
//! `top` with nothing running, and `env set` then `env get`, `env list` and `env paths` over an
//! instance's `.env`, in their spaced and colon spellings.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// A fresh home for one test, under the target's temporary directory.
fn scratch(test: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("core-top-env")
        .join(test);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// `heartwood args` with `home` as its home, its XDG directories under it, and no `CORE_PATH_*`
/// but `CORE_PATH_CONFIG` when `config` is given.
fn heartwood(home: &Path, config: Option<&Path>, args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_heartwood"));
    command.args(args).current_dir(home).env("HOME", home);
    for (name, dir) in [
        ("XDG_DATA_HOME", "data"),
        ("XDG_CONFIG_HOME", "config"),
        ("XDG_CACHE_HOME", "cache"),
        ("XDG_STATE_HOME", "state"),
    ] {
        command.env(name, home.join(dir));
    }
    for kind in ["CONFIG", "DATA", "CACHE", "LOG", "TEMP"] {
        command.env_remove(format!("CORE_PATH_{kind}"));
    }
    if let Some(config) = config {
        command.env("CORE_PATH_CONFIG", config);
    }
    command.output().unwrap()
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[test]
fn top_says_no_processes_are_running_and_fails() {
    let home = scratch("top");
    for line in [&["top", "--token", "node1"][..], &["top"]] {
        let out = heartwood(&home, None, line);
        assert_eq!(out.status.code(), Some(1), "{line:?}");
        assert_eq!(text(&out.stderr), "No processes are running\n", "{line:?}");
        assert!(out.stdout.is_empty());
    }
}

#[test]
fn env_set_then_get_list_and_paths() {
    let home = scratch("env");
    let config = home.join("instance-config");
    fs::create_dir_all(&config).unwrap();
    let file = config.join(".env");
    let flags = ["--token", "node1", "--network", "devnet"];
    let run = |words: &[&str]| {
        let line: Vec<&str> = words.iter().chain(&flags).copied().collect();
        heartwood(&home, Some(&config), &line)
    };
    // No file yet: the reference's words, and nothing is written.
    let absent = run(&["env", "list"]);
    assert_eq!(absent.status.code(), Some(1));
    let wanted = format!("No environment file found at {}\n", file.display());
    assert_eq!(text(&absent.stderr), wanted);
    assert!(!file.exists());

    fs::write(&file, "# a comment\nB=2\n\nA=1\n").unwrap();
    for set in [
        &["env", "set", "--key", "C", "--value", "0x10"][..],
        &["env:set", "--key", "B", "--value", "two words"],
    ] {
        let out = run(set);
        assert!(out.status.success(), "{set:?}: {}", text(&out.stderr));
        assert!(out.stdout.is_empty());
    }
    assert_eq!(fs::read_to_string(&file).unwrap(), "B=two words\nA=1\nC=16");
    for get in [
        &["env", "get", "--key", "C"][..],
        &["env:get", "--key", "C"],
    ] {
        let out = run(get);
        assert!(out.status.success(), "{get:?}: {}", text(&out.stderr));
        assert_eq!(text(&out.stdout), "16\n");
    }
    let missing = run(&["env", "get", "--key", "D"]);
    assert_eq!(missing.status.code(), Some(1));
    assert_eq!(text(&missing.stderr), "The \"D\" doesn't exist\n");
    let numeric = run(&["env", "get", "--key", "42"]);
    assert_eq!(text(&numeric.stderr), "\"key\" must be a string\n");
    let list = run(&["env:list"]);
    assert_eq!(
        text(&list.stdout),
        "Key  Value\nB    two words\nA    1\nC    16\n"
    );
    let paths = run(&["env", "paths"]);
    let kinds: Vec<String> = text(&paths.stdout)
        .lines()
        .map(|row| row.split_whitespace().next().unwrap_or_default().to_owned())
        .collect();
    assert_eq!(kinds, ["Type", "data", "config", "cache", "log", "temp"]);
    let config_row = text(&paths.stdout)
        .lines()
        .nth(2)
        .unwrap_or_default()
        .to_owned();
    assert!(
        config_row.ends_with(&config.display().to_string()),
        "{config_row}"
    );
}

#[test]
fn env_set_accepts_negative_words_and_reports_missing_arguments() {
    let home = scratch("negative-and-missing");
    fs::write(home.join(".env"), "A=1\n").unwrap();
    let run = |args: &[&str]| {
        let args: Vec<_> = args
            .iter()
            .copied()
            .chain(["--network", "devnet"])
            .collect();
        heartwood(&home, Some(&home), &args)
    };
    for (args, missing) in [
        (vec!["env", "set", "--key", "B"], "value"),
        (vec!["env", "set", "--value", "-5"], "key"),
        (vec!["env", "set"], "key"),
    ] {
        let out = run(&args);
        assert_eq!(out.status.code(), Some(1), "{}", text(&out.stderr));
        assert_eq!(text(&out.stderr), format!("\"{missing}\" is required\n"));
        assert_eq!(fs::read_to_string(home.join(".env")).unwrap(), "A=1\n");
    }
    for key in ["B", "-5"] {
        let out = run(&["env", "set", "--key", key, "--value", "-5"]);
        assert!(out.status.success(), "{}", text(&out.stderr));
    }
    assert_eq!(
        fs::read_to_string(home.join(".env")).unwrap(),
        "A=1\nB=-5\n-5=-5"
    );
}
