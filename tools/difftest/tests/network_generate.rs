//! `difftest network generate` as the gates run it: a directory of vectors with no genesis
//! record compares nothing, and that is a failed check, strict or not; the committed genesis
//! vectors pass under `--strict`.

use std::path::{Path, PathBuf};
use std::process::Command;

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// `difftest network generate <dir> [--strict]`: its exit status.
fn generate(dir: &str, strict: bool) -> (Option<i32>, String) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_difftest"));
    command
        .args(["network", "generate"])
        .arg(workspace().join(dir));
    if strict {
        command.arg("--strict");
    }
    let output = command.output().expect("the difftest binary runs");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    (output.status.code(), text)
}

#[test]
fn a_vector_set_without_genesis_records_fails_the_check() {
    // The forger's class holds no genesis record.
    for strict in [false, true] {
        let (code, text) = generate("packages/forger/tests/vectors", strict);
        assert_eq!(code, Some(1), "strict {strict}: {text}");
        assert!(text.contains("genesis.generate"), "{text}");
    }
}

#[test]
fn the_committed_genesis_vectors_pass_strictly() {
    let (code, text) = generate("packages/crypto/tests/vectors", true);
    assert_eq!(code, Some(0), "{text}");
}
