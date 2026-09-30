//! The doctor's source rules, read from its own source files: files only through `DoctorFs`
//! (`sources/fs.rs`), no socket but the loopback GET of `heartwood_cli::http`, processes only
//! through `Probes` (`sources/probes.rs`), none of the process operations that start, stop or
//! signal a role, and the only matches on a role or a supervised status in `seams.rs`. Also: the
//! key-file list holds the forger's key file, and `Readable` refuses it by any spelling.

use std::fs;
use std::path::{Path, PathBuf};

use heartwood_cli::doctor::seams::{KEY_FILE_NAMES, Readable, ReadableRoots, RefusedPath};
use heartwood_kernel::paths::Paths;

/// A rule: the words no line may hold, and the files (relative to `src/doctor`) where they may.
struct Rule {
    what: &'static str,
    words: &'static [&'static str],
    allowed_in: &'static [&'static str],
}

const RULES: [Rule; 6] = [
    Rule {
        what: "files only through DoctorFs",
        words: &[
            "std::fs",
            "fs::",
            "File::",
            "OpenOptions",
            "read_dir",
            "read_to_string",
            "symlink_metadata",
        ],
        allowed_in: &["sources/fs.rs"],
    },
    Rule {
        what: "no socket but the loopback GET",
        words: &[
            "TcpStream",
            "TcpListener",
            "UdpSocket",
            "UnixStream",
            "UnixListener",
            "UnixDatagram",
            "std::net::Tcp",
        ],
        allowed_in: &[],
    },
    Rule {
        what: "processes only through Probes",
        words: &["std::process", "Command::", "Command", "spawn(", "kill("],
        allowed_in: &["sources/probes.rs"],
    },
    Rule {
        what: "nix only for the probes and the file flags and statvfs of DoctorFs",
        words: &["nix::"],
        allowed_in: &["sources/probes.rs", "sources/fs.rs"],
    },
    Rule {
        what: "no process operation",
        words: &[
            "ProcessOps",
            "Supervisors",
            "process::start",
            "process::stop",
            "process::kill",
            "process::restart",
            "signal_if_locked",
            "send_group",
        ],
        allowed_in: &[],
    },
    Rule {
        what: "roles and statuses matched in seams.rs only",
        words: &["SupervisedStatus::", "Role::Relay =>", "Role::Forger =>"],
        allowed_in: &["seams.rs"],
    },
];

/// Every Rust file under `dir`.
fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

/// Whether `line` holds `word` where it does not continue a longer name or path.
fn holds(line: &str, word: &str) -> bool {
    line.match_indices(word).any(|(at, _)| {
        let before = line.get(..at).unwrap_or_default();
        let joined =
            before.ends_with(|c: char| c.is_alphanumeric() || c == '_') || before.ends_with("::");
        !joined
    })
}

#[test]
fn the_doctor_reaches_files_sockets_and_processes_only_through_its_seams() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/doctor");
    let mut files = Vec::new();
    rust_files(&root, &mut files);
    files.sort();
    assert!(files.len() >= 30, "{files:?}");
    let mut broken = Vec::new();
    for file in &files {
        let name = file
            .strip_prefix(&root)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let text = fs::read_to_string(file).unwrap();
        for (number, line) in text.lines().enumerate() {
            let code = line.trim_start();
            if code.starts_with("//") {
                continue;
            }
            for rule in &RULES {
                if rule.allowed_in.contains(&name.as_str()) {
                    continue;
                }
                for word in rule.words {
                    if holds(code, word) {
                        broken.push(format!("{name}:{}: {} ({word})", number + 1, rule.what));
                    }
                }
            }
            if holds(code, "unsafe") {
                broken.push(format!("{name}:{}: unsafe", number + 1));
            }
        }
    }
    assert!(broken.is_empty(), "{broken:#?}");
}

#[test]
fn the_key_file_list_holds_the_forgers_key_file_and_readable_refuses_it() {
    assert!(KEY_FILE_NAMES.contains(&"delegates.json"));
    let root = Path::new("/nodes/node1");
    let paths = Paths {
        config: root.join("config"),
        data: root.join("data"),
        cache: root.join("cache"),
        log: root.join("log"),
        temp: root.join("temp"),
    };
    let roots = ReadableRoots::for_paths(&paths);
    for name in ["delegates.json", "Delegates.json", "DELEGATES.JSON"] {
        let path = root.join("config").join(name);
        assert_eq!(
            Readable::new(&path, &roots),
            Err(RefusedPath::KeyFileName),
            "{name}"
        );
    }
    let refused = [
        ("/etc/passwd", RefusedPath::OutsideRoots),
        // /proc and /sys are read through their fixed files alone.
        ("/proc/net/tcp", RefusedPath::OutsideRoots),
        ("/proc/1/environ", RefusedPath::OutsideRoots),
        ("config/app.json", RefusedPath::NotAbsolute),
        (
            "/nodes/node1/config/../../../etc/passwd",
            RefusedPath::DotComponent,
        ),
    ];
    for (path, why) in refused {
        assert_eq!(Readable::new(Path::new(path), &roots), Err(why), "{path}");
    }
    for path in [
        "/nodes/node1/config/app.json",
        "/nodes/node1/log/solar-testnet-relay-current.log",
        "/etc/ufw/ufw.conf",
        "/run/systemd/timesync/synchronized",
    ] {
        assert!(Readable::new(Path::new(path), &roots).is_ok(), "{path}");
    }
}
