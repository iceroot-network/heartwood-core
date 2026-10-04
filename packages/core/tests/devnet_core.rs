//! A three-node development devnet whose nodes each run one `core` process (its relay and its
//! forger together), over this tree's binary: `heartwood devnet init --validators 3 --core` forges
//! a round of 54 blocks, the chain is consistent across the three relays, and every process stops.
//! Run with `make binary-tests` or `cargo test -- --ignored devnet_core`, with the address prefix
//! of the worktree's loopback block in `HEARTWOOD_DEVNET_PREFIX` (127.15.0 without it).
//! `status --resources` adds each core process tree's figures to the status, in its table and,
//! with `--json`, in its JSON (`core_role.rs` covers the role itself on a one-node devnet).

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// The devnet's directory, empty.
fn root() -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("devnet-core");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// `heartwood devnet <args> --dir <root>`, which must succeed.
fn devnet(root: &Path, args: &[&str]) -> Output {
    let output = Command::new(env!("CARGO_BIN_EXE_heartwood"))
        .arg("devnet")
        .args(args)
        .arg("--dir")
        .arg(root)
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "devnet {args:?}: {text}");
    output
}

/// The files under `dir` (any depth) whose names end with `suffix`.
fn files_ending(dir: &Path, suffix: &str) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            found.extend(files_ending(&path, suffix));
        } else if path.to_string_lossy().ends_with(suffix) {
            found.push(path);
        }
    }
    found
}

/// Stops the devnet when dropped, whether the test passed or not.
struct Stop<'a>(&'a Path);

impl Drop for Stop<'_> {
    fn drop(&mut self) {
        let _ = Command::new(env!("CARGO_BIN_EXE_heartwood"))
            .args(["devnet", "stop", "--kill", "--dir"])
            .arg(self.0)
            .output();
    }
}

#[test]
#[ignore = "devnet"]
fn three_core_nodes_forge_a_round_and_stop() {
    let prefix = std::env::var("HEARTWOOD_DEVNET_PREFIX").unwrap_or_else(|_| "127.15.0".into());
    let root = root();
    let init = [
        "init",
        "--validators",
        "3",
        "--core",
        "--address-prefix",
        &prefix,
        "--epoch-delay",
        "5",
    ];
    devnet(&root, &init);
    let _stop = Stop(&root);
    devnet(&root, &["start"]);
    // One round: 53 delegates and the genesis block, eight-second blocks from an epoch 5 s away,
    // with a margin for the start.
    devnet(&root, &["wait", "--height", "54", "--timeout", "600"]);
    let status = devnet(&root, &["status"]);
    let text = String::from_utf8_lossy(&status.stdout);
    assert!(text.contains("chain consistent at height"), "{text}");
    // Every relay of the devnet was compared, not only the ones that happened to answer.
    let counts = text
        .split_once(" (")
        .and_then(|(_, rest)| rest.split_once(" relays)"));
    let counts = counts.and_then(|(counts, _)| counts.split_once(" of "));
    assert!(
        matches!(counts, Some((a, b)) if a == b && a == "3"),
        "{text}"
    );
    // `--resources` adds each process tree's figures to the table, and to the JSON with `--json`.
    let with = devnet(&root, &["status", "--resources"]);
    let text = String::from_utf8_lossy(&with.stdout);
    assert!(text.contains("chain consistent at height"), "{text}");
    let trees = text
        .lines()
        .filter(|line| line.contains("/core: rss "))
        .count();
    assert_eq!(trees, 3, "{text}");
    let json = devnet(&root, &["status", "--json", "--resources"]);
    let json: serde_json::Value = serde_json::from_slice(&json.stdout).expect("the JSON status");
    assert_eq!(json["nodes"].as_array().map(Vec::len), Some(3), "{json}");
    let trees = json["resources"].as_object().expect("the resources");
    let names: Vec<&str> = trees.keys().map(String::as_str).collect();
    assert_eq!(names, ["node1/core", "node2/core", "node3/core"], "{json}");
    assert!(
        trees.values().all(|tree| tree["rss"].as_u64() > Some(0)),
        "{json}"
    );
    // Each node ran one core process, with no relay or forger process of its own.
    assert_eq!(files_ending(&root, "-core-current.log").len(), 3);
    assert!(files_ending(&root, "-relay-current.log").is_empty());
    assert!(files_ending(&root, "-forger-current.log").is_empty());
    devnet(&root, &["stop"]);
}
