//! A relay's directories for the tests: the committed one-node corpus's crypto files, a relay
//! profile as the devnet generator writes it, a `.env`, `peers.json`, and the process variables
//! that point every `CORE_PATH_*` into a scratch directory.

#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};

use heartwood_core::commands::relay::{RelayCommand, RelayRun};
use heartwood_core::commands::{self, Command};
use serde_json::{Value, json};

/// An address of the loopback block these tests' processes listen on: the three octets of
/// `HEARTWOOD_TEST_LOOPBACK` (such as `127.24.0`) when it is set, else `default`, and `last`.
pub fn loopback(default: &str, last: u8) -> String {
    let block = std::env::var("HEARTWOOD_TEST_LOOPBACK").unwrap_or_default();
    let block = if block.is_empty() { default } else { &block };
    format!("{block}.{last}")
}

/// The committed one-node corpus.
pub fn corpus() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/difftest/corpus/txgen-one-node")
}

/// The relay profile's plugins as the devnet generator writes them.
pub fn relay_plugins() -> Value {
    let package = |name: &str| json!({"package": format!("@solar-network/{name}")});
    json!([
        package("logger"),
        package("state"),
        package("database"),
        package("transactions"),
        package("pool"),
        {"package": "@solar-network/p2p", "options": {"remoteAccess": ["127.0.0.1"], "dns": [], "ntp": []}},
        package("blockchain"),
        package("api"),
        package("webhooks"),
    ])
}

/// A relay's scratch directory.
pub struct Setup {
    /// The scratch root.
    pub root: PathBuf,
}

impl Setup {
    /// A fresh relay named `name`, with `env` as its `.env` text.
    pub fn new(name: &str, env: &str) -> Setup {
        let root =
            std::env::temp_dir().join(format!("heartwood-core-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let setup = Setup { root };
        fs::create_dir_all(setup.config().join("crypto")).unwrap();
        fs::create_dir_all(setup.home()).unwrap();
        for file in [
            "network.json",
            "milestones.json",
            "genesisBlock.json",
            "exceptions.json",
        ] {
            fs::copy(
                corpus().join("crypto").join(file),
                setup.config().join("crypto").join(file),
            )
            .unwrap();
        }
        setup.write(".env", env);
        setup.app(relay_plugins());
        setup.write(
            "peers.json",
            r#"{"list": [{"ip": "192.0.2.1", "port": 4002}]}"#,
        );
        setup
    }

    /// The configuration directory.
    pub fn config(&self) -> PathBuf {
        self.root.join("config")
    }

    /// The home directory.
    pub fn home(&self) -> PathBuf {
        self.root.join("home")
    }

    /// Write `text` to the configuration file `file`.
    pub fn write(&self, file: &str, text: &str) {
        fs::write(self.config().join(file), text).unwrap();
    }

    /// Write an `app.json` whose relay profile has `plugins`.
    pub fn app(&self, plugins: Value) {
        let app = json!({"core": {"plugins": []}, "relay": {"plugins": plugins}, "forger": {"plugins": []}});
        self.write("app.json", &app.to_string());
    }

    /// The process variables: every directory in the scratch root, and `extra`.
    pub fn process(&self, extra: &[(&str, &str)]) -> Vec<(String, String)> {
        let mut vars: Vec<(String, String)> = ["CONFIG", "DATA", "CACHE", "LOG", "TEMP"]
            .iter()
            .map(|kind| {
                let dir = if *kind == "CONFIG" {
                    self.config()
                } else {
                    self.root.join(kind.to_lowercase())
                };
                (format!("CORE_PATH_{kind}"), dir.display().to_string())
            })
            .collect();
        vars.extend(
            extra
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned())),
        );
        vars
    }
}

impl Drop for Setup {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// The flags of `heartwood relay run <flags>`.
pub fn run(flags: &str) -> RelayRun {
    let line = format!("heartwood relay run {flags}");
    let args = line.split_whitespace().map(Into::into).collect();
    match commands::parse(args).unwrap() {
        Command::Relay(RelayCommand::Run(run)) => run,
        other => panic!("{other:?}"),
    }
}
