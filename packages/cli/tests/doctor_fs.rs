//! The doctor's files: `Readable` refuses every key-file name in any letter case and every path
//! outside the readable roots; `HostFs` opens without following a final link and refuses, unread,
//! a descriptor with a key file's device and inode (a hard link, a link swapped in after the name
//! check, a key file that is itself a link); metadata, listings and `statvfs`; the signing
//! record's parser. That `KeyFile` yields no bytes is a compile-fail doc test on the type.

use std::fs;
use std::io::ErrorKind;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::path::{Path, PathBuf};

use heartwood_cli::doctor::seams::{
    DoctorFs, FileKind, FsError, KEY_FILE_NAMES, KeyFile, Readable, ReadableRoots, RefusedPath,
};
use heartwood_cli::doctor::sources::ParseError;
use heartwood_cli::doctor::sources::fs::{
    HostFs, READABLE_FILES, SIGNING_RECORD_FORMAT, parse_signing_record,
};
use heartwood_kernel::paths::Paths;

/// What the planted key file holds; no read may ever return it.
const SECRET: &[u8] = b"{\"secrets\":[\"canary words that must never be read\"]}";

/// A node's five directories under a fresh root, with a key file in its configuration directory.
struct Node {
    root: PathBuf,
    paths: Paths,
}

impl Node {
    fn new(test: &str) -> Node {
        let root = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join("doctor-fs")
            .join(test);
        if root.exists() {
            let _ = fs::set_permissions(root.join("config"), fs::Permissions::from_mode(0o700));
            fs::remove_dir_all(&root).unwrap();
        }
        let dir = |name: &str| root.join(name);
        let paths = Paths {
            config: dir("config"),
            data: dir("data"),
            cache: dir("cache"),
            log: dir("log"),
            temp: dir("temp"),
        };
        for dir in [
            &paths.config,
            &paths.data,
            &paths.cache,
            &paths.log,
            &paths.temp,
        ] {
            fs::create_dir_all(dir).unwrap();
        }
        let node = Node { root, paths };
        fs::write(node.key(), SECRET).unwrap();
        fs::set_permissions(node.key(), fs::Permissions::from_mode(0o600)).unwrap();
        node
    }

    fn key(&self) -> PathBuf {
        self.paths.config.join("delegates.json")
    }

    fn roots(&self) -> ReadableRoots {
        ReadableRoots::for_paths(&self.paths)
    }

    fn fs(&self) -> HostFs {
        HostFs::new(vec![KeyFile::new(self.key())])
    }

    fn readable(&self, path: &Path) -> Readable {
        Readable::new(path, &self.roots()).unwrap()
    }
}

#[test]
fn every_key_file_name_is_refused_in_any_letter_case_and_directory() {
    let node = Node::new("names");
    let roots = node.roots();
    let spellings = [
        "delegates.json",
        "DELEGATES.JSON",
        "Delegates.json",
        "dElEgAtEs.JsOn",
    ];
    let dirs = [
        &node.paths.config,
        &node.paths.data,
        &node.paths.cache,
        &node.paths.log,
        &node.paths.temp,
    ];
    for name in KEY_FILE_NAMES.iter().copied().chain(spellings) {
        for dir in dirs
            .iter()
            .map(|dir| dir.as_path())
            .chain([Path::new("/proc")])
        {
            let path = dir.join(name);
            assert_eq!(
                Readable::new(&path, &roots),
                Err(RefusedPath::KeyFileName),
                "{path:?}"
            );
        }
    }
    let near = [
        "delegates.json.bak",
        "delegates.jsonx",
        "old-delegates.json",
    ];
    for name in near {
        assert!(
            Readable::new(&node.paths.config.join(name), &roots).is_ok(),
            "{name}"
        );
    }
}

#[test]
fn paths_outside_the_readable_roots_are_refused() {
    let node = Node::new("roots");
    let roots = node.roots();
    let config2 = format!("{}2/app.json", node.paths.config.display());
    let parent = node.root.join("notes.txt");
    let outside = [
        "/etc/passwd",
        "/etc/ufw/ufw.conf.bak",
        "/etc/ufw",
        "/run/systemd/timesync",
        "/run/systemd/timesync/synchronized.old",
        "/procfs/net/tcp",
        "/system/x",
        config2.as_str(),
        parent.to_str().unwrap(),
    ];
    for path in outside {
        assert_eq!(
            Readable::new(Path::new(path), &roots),
            Err(RefusedPath::OutsideRoots),
            "{path}"
        );
    }
    let dotted = node.paths.log.join("../config/app.json");
    assert_eq!(
        Readable::new(&dotted, &roots),
        Err(RefusedPath::DotComponent)
    );
    let current = node.paths.log.join("./x.log");
    assert!(
        Readable::new(&current, &roots).is_ok(),
        "a . inside a path is dropped by Path"
    );
    assert_eq!(
        Readable::new(Path::new("log/x.log"), &roots),
        Err(RefusedPath::NotAbsolute)
    );
    for file in READABLE_FILES {
        assert!(Readable::new(Path::new(file), &roots).is_ok(), "{file}");
    }
    let refused = FsError::Refused(RefusedPath::OutsideRoots);
    assert_eq!(
        refused.to_string(),
        "refused: the path is outside the instance's directories"
    );
}

#[test]
fn a_plain_file_is_read_whole_or_by_its_tail() {
    let node = Node::new("plain");
    let fs_ = node.fs();
    let file = node.paths.data.join("record.json");
    fs::write(&file, b"0123456789").unwrap();
    let readable = node.readable(&file);
    assert_eq!(fs_.read(&readable, 10).unwrap(), b"0123456789");
    assert_eq!(fs_.read(&readable, 9), Err(FsError::TooLarge { max: 9 }));
    let tail = fs_.tail(&readable, 4).unwrap();
    assert_eq!(
        (tail.bytes.as_slice(), tail.start, tail.len),
        (&b"6789"[..], 6, 10)
    );
    let whole = fs_.tail(&readable, 100).unwrap();
    assert_eq!(
        (whole.bytes.as_slice(), whole.start, whole.len),
        (&b"0123456789"[..], 0, 10)
    );
    let empty = fs_.tail(&readable, 0).unwrap();
    assert_eq!((empty.bytes.len(), empty.start, empty.len), (0, 10, 10));
    let missing = node.readable(&node.paths.data.join("missing.json"));
    assert_eq!(
        fs_.read(&missing, 10),
        Err(FsError::Io(ErrorKind::NotFound))
    );
    let proc = node.readable(Path::new("/proc/self/status"));
    let status = fs_.read(&proc, 1024 * 1024).unwrap();
    assert!(
        status.starts_with(b"Name:"),
        "a /proc file reads to its end although its size is 0"
    );
}

#[test]
fn an_innocently_named_symbolic_link_to_a_key_file_is_not_followed() {
    let node = Node::new("symlink");
    let fs_ = node.fs();
    let link = node.paths.log.join("solar-testnet-relay-current.log");
    symlink(node.key(), &link).unwrap();
    let readable = node.readable(&link);
    for result in [
        fs_.read(&readable, 1 << 20).map(drop),
        fs_.tail(&readable, 1 << 20).map(drop),
    ] {
        let error = result.unwrap_err();
        assert!(
            matches!(error, FsError::Io(kind) if kind != ErrorKind::NotFound),
            "{error:?}"
        );
    }
    let relative = node.paths.data.join("notes.json");
    symlink("../config/delegates.json", &relative).unwrap();
    assert!(fs_.read(&node.readable(&relative), 1 << 20).is_err());
}

#[test]
fn a_hard_link_to_a_key_file_is_refused_by_device_and_inode() {
    let node = Node::new("hardlink");
    let fs_ = node.fs();
    let link = node.paths.log.join("solar-testnet-forger-current.log");
    fs::hard_link(node.key(), &link).unwrap();
    let readable = node.readable(&link);
    assert_eq!(fs_.read(&readable, 1 << 20), Err(FsError::KeyFile));
    assert_eq!(fs_.tail(&readable, 1 << 20), Err(FsError::KeyFile));
    assert_eq!(FsError::KeyFile.to_string(), "the file is a key file");
}

#[test]
fn a_link_swapped_in_after_the_name_check_is_refused_on_the_open_descriptor() {
    let node = Node::new("swap");
    let fs_ = node.fs();
    let file = node.paths.data.join("forger/signing-record.json");
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(&file, b"{}").unwrap();
    let readable = node.readable(&file);
    assert_eq!(fs_.read(&readable, 1024).unwrap(), b"{}");

    let staged = node.paths.data.join("forger/staged");
    fs::hard_link(node.key(), &staged).unwrap();
    fs::rename(&staged, &file).unwrap();
    assert_eq!(fs_.read(&readable, 1 << 20), Err(FsError::KeyFile));
    assert_eq!(fs_.tail(&readable, 1 << 20), Err(FsError::KeyFile));

    symlink(node.key(), &staged).unwrap();
    fs::rename(&staged, &file).unwrap();
    assert!(matches!(fs_.read(&readable, 1 << 20), Err(FsError::Io(_))));
}

#[test]
fn a_key_file_that_is_itself_a_link_protects_its_target() {
    let node = Node::new("keylink");
    let target = node.paths.data.join("keys-store.json");
    fs::rename(node.key(), &target).unwrap();
    symlink(&target, node.key()).unwrap();
    let fs_ = node.fs();
    assert_eq!(
        fs_.read(&node.readable(&target), 1 << 20),
        Err(FsError::KeyFile)
    );
    let facts = KeyFile::new(node.key()).facts(&fs_).unwrap();
    assert_eq!(facts.own.kind, FileKind::Symlink);
    assert_eq!(facts.target.map(|meta| meta.kind), Some(FileKind::File));
    assert_eq!(facts.link_target, Some(target));
}

#[test]
fn a_key_file_whose_identity_cannot_be_read_refuses_every_open() {
    let node = Node::new("unreadable");
    let file = node.paths.data.join("plain.json");
    fs::write(&file, b"{}").unwrap();
    let absent = HostFs::new(vec![KeyFile::new(node.paths.config.join("DELEGATES.json"))]);
    assert_eq!(absent.read(&node.readable(&file), 16).unwrap(), b"{}");
    if fs::metadata(&file).unwrap().uid() == 0 {
        return;
    }
    fs::set_permissions(&node.paths.config, fs::Permissions::from_mode(0o000)).unwrap();
    let read = node.fs().read(&node.readable(&file), 16);
    fs::set_permissions(&node.paths.config, fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(read, Err(FsError::Io(ErrorKind::PermissionDenied)));
}

#[test]
fn pipes_and_directories_are_not_read() {
    let node = Node::new("special");
    let fs_ = node.fs();
    let pipe = node.paths.log.join("solar-testnet-relay-current.log");
    nix::unistd::mkfifo(&pipe, nix::sys::stat::Mode::from_bits_truncate(0o600)).unwrap();
    let invalid = Err(FsError::Io(ErrorKind::InvalidInput));
    assert_eq!(fs_.read(&node.readable(&pipe), 16).map(drop), invalid);
    assert_eq!(
        fs_.read(&node.readable(&node.paths.log), 16).map(drop),
        invalid
    );
}

#[test]
fn metadata_listings_and_space_come_without_opening_a_key_file() {
    let node = Node::new("meta");
    let fs_ = node.fs();
    let facts = KeyFile::new(node.key()).facts(&fs_).unwrap();
    assert_eq!((facts.own.kind, facts.own.mode), (FileKind::File, 0o600));
    assert_eq!(facts.own.size, u64::try_from(SECRET.len()).unwrap());
    assert!(facts.own.nlink == 1 && facts.target.is_none() && facts.link_target.is_none());
    assert!(facts.own.modified_ms > 1_700_000_000_000);
    let dangling = node.paths.config.join("peers.json");
    symlink("gone.json", &dangling).unwrap();
    let facts = fs_.facts(&dangling).unwrap();
    assert_eq!((facts.own.kind, facts.target), (FileKind::Symlink, None));
    assert_eq!(facts.link_target, Some(PathBuf::from("gone.json")));
    let missing = fs_.facts(&node.paths.config.join("app.json")).unwrap_err();
    assert_eq!(missing.kind(), ErrorKind::NotFound);
    let run = node.paths.data.join("run");
    assert_eq!(fs_.facts(&run).unwrap_err().kind(), ErrorKind::NotFound);

    fs::write(node.paths.config.join("app.json"), b"{}").unwrap();
    let names = fs_.list(&node.readable(&node.paths.config)).unwrap();
    assert_eq!(names, ["app.json", "delegates.json", "peers.json"]);
    let linked = node.paths.temp.join("config-link");
    symlink(&node.paths.config, &linked).unwrap();
    let not_dir = Err(FsError::Io(ErrorKind::NotADirectory));
    assert_eq!(fs_.list(&node.readable(&linked)), not_dir);
    assert_eq!(
        fs_.list(&node.readable(&node.paths.config.join("app.json"))),
        not_dir
    );

    let stats = fs_.stats(&node.paths.data).unwrap();
    assert!(stats.total_bytes > 0 && stats.available_bytes <= stats.total_bytes);
    assert!(stats.available_inodes <= stats.total_inodes);
    let gone = fs_.stats(&node.paths.data.join("missing"));
    assert_eq!(gone, Err(FsError::Io(ErrorKind::NotFound)));
}

const NETHASH: &str = "7b9a7c6a462a0f2b0b1c7d3bd8a3e3b8e2d5b8c5a1f5d9e0c7b6a5f4e3d2c1b0";
const KEY_A: &str = "02a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8f90";
const KEY_B: &str = "03f0e1d2c3b4a5968778695a4b3c2d1e0ff0e1d2c3b4a5968778695a4b3c2d1e0f";
const BLOCK: &str = "c0ffee00c0ffee00c0ffee00c0ffee00c0ffee00c0ffee00c0ffee00c0ffee00";

fn record(keys: &str) -> String {
    format!(r#"{{"format":"{SIGNING_RECORD_FORMAT}","nethash":"{NETHASH}","keys":{{{keys}}}}}"#)
}

fn entry(key: &str, height: &str, slot: &str) -> String {
    format!(r#""{key}":{{"height":{height},"slot":{slot},"id":"{BLOCK}","bytes":"00ff00ff"}}"#)
}

#[test]
fn the_signing_record_keeps_public_chain_data_and_drops_the_blocks_bytes() {
    let text = record(&format!(
        "{},{}",
        entry(KEY_B, "12", "34"),
        entry(KEY_A, "11", "-2")
    ));
    let facts = parse_signing_record(text.as_bytes()).unwrap();
    assert!(facts.known_format);
    assert_eq!(facts.nethash, NETHASH);
    let keys: Vec<&str> = facts
        .entries
        .iter()
        .map(|entry| entry.public_key.as_str())
        .collect();
    assert_eq!(keys, [KEY_A, KEY_B]);
    let b = &facts.entries[1];
    assert_eq!((b.height, b.slot, b.block_id.as_str()), (12, 34, BLOCK));
    assert_eq!(facts.entries[0].slot, -2);
    assert!(!format!("{facts:?}").contains("00ff00ff"));

    let upper = record(&entry(&KEY_A.to_ascii_uppercase(), "1", "1"))
        .replace(BLOCK, &BLOCK.to_ascii_uppercase());
    let facts = parse_signing_record(upper.as_bytes()).unwrap();
    assert_eq!(
        (
            facts.entries[0].public_key.as_str(),
            facts.entries[0].block_id.as_str()
        ),
        (KEY_A, BLOCK)
    );
    let empty = parse_signing_record(record("").as_bytes()).unwrap();
    assert!(empty.known_format && empty.entries.is_empty());
}

#[test]
fn a_signing_record_of_another_format_or_network_is_marked_not_guessed() {
    let other = r#"{"format":"heartwood-signing-record/2","nethash":"x","keys":{}}"#;
    let facts = parse_signing_record(other.as_bytes()).unwrap();
    assert!(!facts.known_format && facts.nethash.is_empty() && facts.entries.is_empty());
    for bare in [&b"{}"[..], b"[1,2]", b"null"] {
        assert!(!parse_signing_record(bare).unwrap().known_format);
    }
    let odd_hash = record("").replace(NETHASH, "not a nethash");
    let facts = parse_signing_record(odd_hash.as_bytes()).unwrap();
    assert!(facts.known_format && facts.nethash.is_empty());
}

#[test]
fn a_signing_record_the_forger_would_refuse_is_malformed() {
    let refused = [
        String::from("not json"),
        record("").replace(r#","keys":{}"#, ""),
        record(&entry("02ab", "1", "1")),
        record(&entry(KEY_A, "4294967296", "1")),
        record(&entry(KEY_A, "-1", "1")),
        record(&entry(KEY_A, "1", "1.5")),
        record(&entry(KEY_A, "1", "1")).replace(BLOCK, "ab"),
        record(&entry(KEY_A, "1", "1")).replace(r#","bytes":"00ff00ff""#, ""),
        format!("{}{}", "[".repeat(100_000), "]".repeat(100_000)),
    ];
    for text in refused {
        let parsed = parse_signing_record(text.as_bytes());
        assert_eq!(
            parsed,
            Err(ParseError::Malformed("signing record")),
            "{text:.80}"
        );
    }
    assert!(parse_signing_record(&[0xff, 0x00, 0x7b]).is_err());
}
