//! A forging devnet of this tree's binary (run with `make devnet-smoke`, which passes the address
//! prefix of the worktree's loopback block in `HEARTWOOD_DEVNET_PREFIX`): `heartwood devnet init
//! --validators 1` forges 20 blocks, then a five-node devnet forges a round, each chain consistent
//! across its relays, and every process stops.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// The devnet's directory for `name`, empty.
fn root(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("devnet-smoke")
        .join(name);
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

/// A devnet of `validators` nodes forging to `height`, then stopped; stopped as well when the
/// test fails on the way.
fn forge(name: &str, validators: u32, height: u32) {
    let prefix = std::env::var("HEARTWOOD_DEVNET_PREFIX").unwrap_or_else(|_| "127.10.0".into());
    let root = root(name);
    let count = validators.to_string();
    devnet(
        &root,
        &[
            "init",
            "--validators",
            &count,
            "--address-prefix",
            &prefix,
            "--epoch-delay",
            "5",
        ],
    );
    struct Stop<'a>(&'a Path);
    impl Drop for Stop<'_> {
        fn drop(&mut self) {
            let _ = Command::new(env!("CARGO_BIN_EXE_heartwood"))
                .args(["devnet", "stop", "--kill", "--dir"])
                .arg(self.0)
                .output();
        }
    }
    let _stop = Stop(&root);
    devnet(&root, &["start"]);
    // Eight-second blocks from an epoch 5 s away, with a margin for the start.
    let timeout = (u64::from(height) * 8 + 120).to_string();
    devnet(
        &root,
        &[
            "wait",
            "--height",
            &height.to_string(),
            "--timeout",
            &timeout,
        ],
    );
    let status = devnet(&root, &["status"]);
    let text = String::from_utf8_lossy(&status.stdout);
    assert!(text.contains("chain consistent at height"), "{text}");
    // Every relay of the devnet was compared, not only the ones that happened to answer.
    let counts = text
        .split_once(" (")
        .and_then(|(_, rest)| rest.split_once(" relays)"));
    let counts = counts.and_then(|(counts, _)| counts.split_once(" of "));
    assert!(matches!(counts, Some((a, b)) if a == b), "{text}");
    devnet(&root, &["stop"]);
}

#[test]
#[ignore = "devnet"]
fn one_node_forges_twenty_blocks_then_five_nodes_forge_a_round() {
    forge("one", 1, 20);
    forge("five", 5, 54);
}
