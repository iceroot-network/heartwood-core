//! Installing the logger for the process: the file output, and a second installation refused.

use std::fs;

use heartwood_logger::rotation::current_name;
use heartwood_logger::{LoggerError, LoggerOptions, init};

#[test]
fn init_writes_the_file_and_installs_once() {
    let dir = std::env::temp_dir().join(format!("heartwood-logger-init-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let options = LoggerOptions {
        console: None,
        ..LoggerOptions::default()
    };
    let guard = init(&options, &dir, "root-testnet-core").unwrap();
    heartwood_kernel::info!("installed");
    heartwood_kernel::debug!("in the file only");
    assert!(matches!(
        init(&options, &dir, "root-testnet-core"),
        Err(LoggerError::AlreadyInstalled)
    ));
    drop(guard);
    let text = fs::read_to_string(dir.join(current_name("root-testnet-core"))).unwrap();
    let lines: Vec<&str> = text
        .lines()
        .map(|line| line.split_once("] ").unwrap().1)
        .collect();
    assert_eq!(lines, ["INFO: installed", "DEBUG: in the file only"]);
    fs::remove_dir_all(&dir).unwrap();
}
