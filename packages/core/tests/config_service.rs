//! `config` and `service` through the `heartwood` binary, in a scratch home: a configuration
//! published and given a forger key, the same commands in their colon spellings, the units
//! written and removed, and a forger started as a systemd unit starts it (`<role> run` with the
//! unit's own environment) named by `status` and kept by `stop`.

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::Duration;

use tokio::time::Instant;

/// The key the tests write.
const KEY: &str = "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210";

/// A scratch home and its five directories.
struct Node {
    root: PathBuf,
}

impl Node {
    fn new(test: &str) -> Node {
        let root = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join("core-config-service")
            .join(test);
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        Node { root }
    }

    fn config(&self) -> PathBuf {
        self.root.join("config")
    }

    /// `heartwood args` with the node's directories, its home and no service manager named.
    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_heartwood"));
        command
            .args(args)
            .current_dir(&self.root)
            .env("HOME", &self.root);
        command
            .env_remove("CORE_SERVICE_MANAGER")
            .env_remove("CORE_SERVICE_UNIT");
        for xdg in ["CONFIG", "DATA", "CACHE", "STATE"] {
            command.env_remove(format!("XDG_{xdg}_HOME"));
        }
        for (kind, dir) in [
            ("CONFIG", "config"),
            ("DATA", "data"),
            ("CACHE", "cache"),
            ("LOG", "log"),
            ("TEMP", "temp"),
        ] {
            command.env(format!("CORE_PATH_{kind}"), self.root.join(dir));
        }
        command
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command(args).output().unwrap()
    }
}

/// `head`, then `flags`, then `more`, as one argument list.
fn line_of<'a>(head: &[&'a str], flags: &[&'a str], more: &[&'a str]) -> Vec<&'a str> {
    [head, flags, more].concat()
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn mode(path: &Path) -> u32 {
    use std::os::unix::fs::MetadataExt;
    fs::metadata(path).unwrap().mode() & 0o777
}

/// A process killed when dropped.
struct Killed(Child);

impl Drop for Killed {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn config_publish_then_forger_private_key_through_the_binary() {
    let node = Node::new("config");
    let flags = ["--token", "t", "--network", "devnet"];
    let with = |words: &[&str]| -> Vec<String> {
        words
            .iter()
            .chain(&flags)
            .map(|w| (*w).to_owned())
            .collect()
    };
    let run = |words: &[&str]| {
        let line = with(words);
        node.run(&line.iter().map(String::as_str).collect::<Vec<_>>())
    };
    // The key file is not there yet: refused, naming it.
    let absent = run(&["config", "forger", "private-key", "--privateKey", KEY]);
    assert_eq!(absent.status.code(), Some(1));
    let wanted = node.config().join("delegates.json").display().to_string();
    assert!(
        text(&absent.stderr).contains(&wanted),
        "{}",
        text(&absent.stderr)
    );

    let published = run(&["config", "publish"]);
    assert!(published.status.success(), "{}", text(&published.stderr));
    let said = text(&published.stdout);
    assert!(said.contains("network generate --chain-only"), "{said}");
    for name in [
        ".env",
        "app.json",
        "peers.json",
        "delegates.json",
        "config.json",
    ] {
        assert!(node.config().join(name).is_file(), "{name}");
    }
    assert_eq!(mode(&node.config().join("delegates.json")), 0o600);
    // An existing configuration is refused with the reference's words, and kept.
    let again = run(&["config:publish"]);
    assert_eq!(again.status.code(), Some(1));
    assert_eq!(
        text(&again.stderr),
        "Please use the --force flag if you wish to overwrite the existing configuration\n"
    );
    assert!(run(&["config:publish", "--force"]).status.success());

    // The key goes in under both spellings, and the key file stays private.
    for line in [
        &["config", "forger", "private-key", "--privateKey", KEY][..],
        &[
            "config:forger:private-key",
            "--privateKey",
            &KEY.to_uppercase(),
        ],
        &[
            "config",
            "forger",
            "--method",
            "privateKey",
            "--privateKey",
            KEY,
        ],
    ] {
        let wrote = run(line);
        assert!(wrote.status.success(), "{line:?}: {}", text(&wrote.stderr));
        assert!(!text(&wrote.stdout).contains(KEY) && !text(&wrote.stderr).contains(KEY));
        let file = fs::read_to_string(node.config().join("delegates.json")).unwrap();
        assert_eq!(
            file,
            format!("{{\n    \"keys\": [\n        \"{KEY}\"\n    ]\n}}\n")
        );
        assert_eq!(mode(&node.config().join("delegates.json")), 0o600);
    }
    // The deferred method and a key that is not 64 hexadecimal characters change nothing.
    let bip39 = run(&["config:forger", "--method", "bip39", "--bip39", "a b c"]);
    assert_eq!(bip39.status.code(), Some(1));
    assert!(
        text(&bip39.stderr).contains("deferred"),
        "{}",
        text(&bip39.stderr)
    );
    let short = run(&["config", "forger", "private-key", "--privateKey", &KEY[1..]]);
    assert_eq!(short.status.code(), Some(1));
    assert!(
        text(&short.stderr).contains("64 hexadecimal"),
        "{}",
        text(&short.stderr)
    );
    assert!(!text(&short.stderr).contains(&KEY[1..]));
    let file = fs::read_to_string(node.config().join("delegates.json")).unwrap();
    assert!(file.contains(KEY));
}

#[test]
fn service_writes_and_removes_a_unit_and_prints_the_commands() {
    let node = Node::new("service");
    let (out, flags) = (
        node.root.join("units"),
        ["--token", "t", "--network", "devnet"],
    );
    let out = out.to_str().unwrap();
    let line = |head: &[&'static str], more: &[&str]| -> Vec<String> {
        head.iter()
            .chain(&flags)
            .chain(more)
            .map(|w| (*w).to_owned())
            .collect()
    };
    let run = |args: Vec<String>| node.run(&args.iter().map(String::as_str).collect::<Vec<_>>());
    let unit = Path::new(out).join("t-devnet-relay.service");
    let installed = run(line(
        &["service", "install", "--role", "relay"],
        &["--out", out],
    ));
    assert!(installed.status.success(), "{}", text(&installed.stderr));
    let said = text(&installed.stdout);
    assert!(
        said.contains("systemctl --user enable --now t-devnet-relay.service"),
        "{said}"
    );
    let unit_text = fs::read_to_string(&unit).unwrap();
    let program = env!("CARGO_BIN_EXE_heartwood");
    assert!(unit_text.contains(&format!(
        "ExecStart={program} relay run --token=t --network=devnet"
    )));
    assert!(unit_text.contains("Environment=CORE_SERVICE_MANAGER=systemd"));
    // The unit names the directories the command resolved: the moved data directory here.
    let data = node.root.join("data");
    let expected = format!("Environment=CORE_PATH_DATA={}", data.display());
    assert!(unit_text.lines().any(|l| l == expected), "{unit_text}");
    // And a data directory an XDG variable moves, which the service manager would not have.
    let xdg = node.root.join("xdg");
    let moved = node
        .command(&line_of(
            &["service", "install", "--role", "relay"],
            &flags,
            &["--out", out],
        ))
        .env_remove("CORE_PATH_DATA")
        .env("XDG_DATA_HOME", &xdg)
        .output()
        .unwrap();
    assert!(moved.status.success(), "{}", text(&moved.stderr));
    let unit_text = fs::read_to_string(&unit).unwrap();
    let expected = format!(
        "Environment=CORE_PATH_DATA={}",
        xdg.join("t-core/devnet").display()
    );
    assert!(unit_text.lines().any(|l| l == expected), "{unit_text}");
    // A system unit with no directory is printed, not written.
    let printed = run(line(
        &["service:install", "--role", "forger", "--system"],
        &[],
    ));
    assert!(printed.status.success(), "{}", text(&printed.stderr));
    let said = text(&printed.stdout);
    assert!(
        said.starts_with("[Unit]\n") && said.contains("WantedBy=multi-user.target"),
        "{said}"
    );
    assert!(!node.root.join(".config").exists());
    // A role that is no role is a usage error.
    assert!(
        !run(line(&["service", "install", "--role", "node"], &[]))
            .status
            .success()
    );
    let removed = run(line(
        &["service:uninstall", "--role", "relay"],
        &["--out", out],
    ));
    assert!(removed.status.success(), "{}", text(&removed.stderr));
    assert!(!unit.exists());
    assert!(
        !run(line(
            &["service", "uninstall", "--role", "relay"],
            &["--out", out]
        ))
        .status
        .success()
    );
}

/// The environment a unit's `Environment=` lines give its process, as systemd reads them: one
/// assignment per line, unquoted, escapes undone, `%n` the unit's `name` and `%%` a `%`.
fn unit_environment(unit: &str, name: &str) -> Vec<(String, String)> {
    let read = |pair: &str| {
        let pair = match pair.strip_prefix('"').and_then(|p| p.strip_suffix('"')) {
            Some(quoted) => {
                let (mut out, mut chars) = (String::new(), quoted.chars());
                while let Some(c) = chars.next() {
                    out.push(if c == '\\' { chars.next().unwrap() } else { c });
                }
                out
            }
            None => pair.to_owned(),
        };
        pair.replace("%n", name).replace("%%", "%")
    };
    let lines = unit.lines().filter_map(|l| l.strip_prefix("Environment="));
    lines
        .map(read)
        .filter_map(|pair| {
            let (k, v) = pair.split_once('=')?;
            Some((k.to_owned(), v.to_owned()))
        })
        .collect()
}

/// Wait until `done` holds, at most `limit`.
fn wait_until(limit: Duration, what: &str, mut done: impl FnMut() -> bool) {
    let start = Instant::now();
    while !done() {
        assert!(start.elapsed() < limit, "no {what} after {limit:?}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn a_forger_started_as_a_unit_starts_it_is_online_systemd_and_stop_refuses() {
    let node = Node::new("unit");
    let flags = ["--token", "t", "--network", "devnet"];
    // The commands of the guide's flow build the instance: the template, the crypto files and the
    // key; the forger waits for a relay at a port nothing listens on.
    let config = node.config();
    assert!(
        node.run(&[&["config", "publish"][..], &flags].concat())
            .status
            .success()
    );
    fs::create_dir_all(config.join("crypto")).unwrap();
    for file in [
        "network.json",
        "milestones.json",
        "genesisBlock.json",
        "exceptions.json",
    ] {
        fs::copy(
            common::corpus().join("crypto").join(file),
            config.join("crypto").join(file),
        )
        .unwrap();
    }
    for step in [
        &["env", "set", "--key", "CORE_P2P_PORT", "--value", "1"][..],
        &["config", "forger", "private-key", "--privateKey", KEY],
    ] {
        let done = node.run(&[step, &flags[..]].concat());
        assert!(done.status.success(), "{step:?}: {}", text(&done.stderr));
    }
    let running = |args: &[&str]| node.run(&[args, &flags[..]].concat());
    assert_eq!(mode(&config.join("delegates.json")), 0o600);

    // The unit's start: `forger run` with only the environment the unit gives and the service
    // manager's own (its home and PATH), none of the shell's directories.
    let units = node.root.join("units");
    let units = units.to_str().unwrap();
    let installed = running(&["service", "install", "--role", "forger", "--out", units]);
    assert!(installed.status.success(), "{}", text(&installed.stderr));
    let unit = fs::read_to_string(Path::new(units).join("t-devnet-forger.service")).unwrap();
    let log = fs::File::create(node.root.join("forger.log")).unwrap();
    let mut forger = Command::new(env!("CARGO_BIN_EXE_heartwood"));
    forger
        .args(["forger", "run", "--token", "t", "--network", "devnet"])
        .current_dir("/")
        .env_clear()
        .env("HOME", &node.root)
        .env("PATH", std::env::var_os("PATH").unwrap_or_default());
    for (name, value) in unit_environment(&unit, "t-devnet-forger.service") {
        forger.env(name, value);
    }
    let forger = forger
        .stdin(Stdio::null())
        .stdout(log.try_clone().unwrap())
        .stderr(log)
        .spawn()
        .unwrap();
    let forger = Killed(forger);
    let pid = node.root.join("data/run/forger.pid");
    wait_until(Duration::from_secs(20), "forger's pid file", || {
        pid.exists()
    });
    let record = fs::read_to_string(node.root.join("data/run/forger.manager.json")).unwrap();
    assert!(
        record.contains("\"systemd\"") && record.contains("t-devnet-forger.service"),
        "{record}"
    );
    assert_eq!(mode(&node.root.join("data/run/forger.manager.json")), 0o600);

    let status = running(&["forger", "status"]);
    let table = text(&status.stdout);
    assert!(
        table.contains("t-devnet-forger") && table.contains("online (systemd)"),
        "{table}"
    );
    for verb in ["stop", "restart"] {
        let refused = running(&["forger", verb]);
        assert_eq!(refused.status.code(), Some(1), "{verb}");
        let said = text(&refused.stderr);
        assert!(
            said.contains(&format!("systemctl --user {verb} t-devnet-forger.service")),
            "{said}"
        );
    }
    // The shell's commands see the unit's node: one data directory, one lock, one signing record.
    let second = running(&["forger", "start"]);
    assert_eq!(second.status.code(), Some(1), "{}", text(&second.stdout));
    let said = text(&second.stderr);
    assert!(said.contains("is already running"), "{said}");
    let default = node.root.join(".local/share/t-core/devnet");
    assert!(
        !default.exists(),
        "the unit's node used the default data directory"
    );
    // `top` lists it, from the entry its start wrote.
    let top = node.run(&["top", "--token", "t"]);
    let table = text(&top.stdout);
    assert!(
        top.status.success() && table.contains("online (systemd)"),
        "{table}"
    );
    // `log` names the journal.
    let said = text(&running(&["forger", "log"]).stderr);
    assert!(
        said.contains("journalctl --user -u t-devnet-forger.service"),
        "{said}"
    );

    // The node still runs: nothing was signalled.
    let mut forger = forger;
    assert!(forger.0.try_wait().unwrap().is_none());

    // The manager stops it as the unit says, with SIGINT; the record names nothing after.
    let signalled = Command::new("kill")
        .args(["-INT", &forger.0.id().to_string()])
        .status()
        .unwrap();
    assert!(signalled.success());
    wait_until(Duration::from_secs(30), "the forger's exit", || {
        forger.0.try_wait().unwrap().is_some()
    });
    let table = text(&running(&["forger", "status"]).stdout);
    assert!(table.contains("stopped"), "{table}");

    // The same node started by hand records no manager, and a record left before is removed.
    let by_hand = node
        .command(&["forger", "run", "--token", "t", "--network", "devnet"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let by_hand = Killed(by_hand);
    wait_until(Duration::from_secs(20), "forger's pid file", || {
        pid.exists()
    });
    wait_until(Duration::from_secs(5), "the old record's removal", || {
        !node.root.join("data/run/forger.manager.json").exists()
    });
    let table = text(&running(&["forger", "status"]).stdout);
    assert!(table.contains("online (unsupervised)"), "{table}");
    drop(by_hand);
}
