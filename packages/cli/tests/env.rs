//! `env get`, `list`, `set` and `paths` over an instance's `<config>/.env`: the reading with the
//! kernel's parser, the rewrite as `envfile` writes it, the refusal of a link in the file's place,
//! the flags as the reference's argument parser reads them, and the tables.

use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};

use heartwood_cli::env::{
    EnvError, flag_text, get, get_text, list, list_text, parse, paths_text, set, set_text,
    stringify,
};
use heartwood_cli::seams::{Instance, Role};
use heartwood_kernel::paths::Paths;

fn scratch(test: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("cli-env")
        .join(test);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// An instance whose configuration directory holds `env` as its `.env`, when there is one.
fn with_env(test: &str, env: Option<&str>) -> Instance {
    let root = scratch(test);
    let config = root.join("config");
    fs::create_dir_all(&config).unwrap();
    if let Some(text) = env {
        fs::write(config.join(".env"), text).unwrap();
    }
    Instance {
        token: "node1".into(),
        network: "devnet".into(),
        role: Role::Relay,
        paths: Paths {
            config,
            data: root.join("data"),
            cache: root.join("cache"),
            log: root.join("log"),
            temp: root.join("temp"),
        },
    }
}

fn file(instance: &Instance) -> PathBuf {
    instance.paths.config.join(".env")
}

fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
    list.iter()
        .map(|(k, v)| ((*k).into(), (*v).into()))
        .collect()
}

#[test]
fn a_file_is_read_with_the_kernels_parser() {
    let text = "# a comment\nA=1\nB: two\nC=\"quoted\"\nD=true\n\n10=x\n";
    let read = parse(text);
    // The array-index key comes first; the node drops a quote pair and keeps digits and words.
    assert_eq!(
        read,
        pairs(&[
            ("10", "x"),
            ("A", "1"),
            ("B", "two"),
            ("C", "quoted"),
            ("D", "true")
        ])
    );
    let instance = with_env("read", Some(text));
    assert_eq!(list(&instance).unwrap(), read);
    assert_eq!(get(&instance, "B").unwrap().as_deref(), Some("two"));
    assert_eq!(get(&instance, "missing").unwrap(), None);
}

#[test]
fn a_missing_file_is_named_as_the_reference_names_it() {
    let instance = with_env("absent", None);
    let message = format!("No environment file found at {}", file(&instance).display());
    for got in [
        get(&instance, "A").map(drop),
        list(&instance).map(drop),
        set(&instance, "A", "1"),
    ] {
        assert_eq!(got.unwrap_err().to_string(), message);
    }
    // No configuration directory at all is the same answer.
    let mut gone = with_env("no-config", None);
    gone.paths.config = gone.paths.config.join("more");
    assert!(matches!(list(&gone), Err(EnvError::NoFile { .. })));
    assert!(!file(&gone).exists());
}

#[test]
fn set_writes_every_variable_as_envfile_does() {
    let instance = with_env("set", Some("# top\nb=1\n\n# note\na=2\n10=3\n"));
    set(&instance, "c", "4").unwrap();
    // Comments and blank lines are gone, the array index comes first and no line break ends it.
    assert_eq!(
        fs::read_to_string(file(&instance)).unwrap(),
        "10=3\nb=1\na=2\nc=4"
    );
    set(&instance, "b", "changed").unwrap();
    assert_eq!(
        fs::read_to_string(file(&instance)).unwrap(),
        "10=3\nb=changed\na=2\nc=4"
    );
    // Only the file is left in the directory.
    let names: Vec<_> = fs::read_dir(&instance.paths.config)
        .unwrap()
        .flatten()
        .collect();
    assert_eq!(names.len(), 1);
}

#[test]
fn set_keeps_the_files_mode_and_the_stringify_rules() {
    let instance = with_env("mode", Some("A=1\n"));
    fs::set_permissions(file(&instance), fs::Permissions::from_mode(0o640)).unwrap();
    set(&instance, "__proto__", "x").unwrap();
    set(&instance, "B", "2").unwrap();
    let mode = fs::metadata(file(&instance)).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o640);
    assert_eq!(fs::read_to_string(file(&instance)).unwrap(), "A=1\nB=2");
    assert_eq!(
        stringify(&pairs(&[("", "x"), ("A", "1"), ("A", "2")])),
        "A=2"
    );
    assert_eq!(stringify(&pairs(&[("A", "\n")])), "A=");
    assert_eq!(stringify(&[]), "");
}

#[test]
fn set_refuses_an_empty_key_or_value() {
    let instance = with_env("empty", Some("A=1\n"));
    for (key, value) in [("", "x"), ("A", "")] {
        assert!(
            matches!(set(&instance, key, value), Err(EnvError::Empty(_))),
            "{key:?}"
        );
    }
    assert_eq!(fs::read_to_string(file(&instance)).unwrap(), "A=1\n");
}

#[test]
fn a_link_in_the_files_place_is_refused_and_never_written_through() {
    let instance = with_env("link", None);
    let target = instance.paths.config.join("real");
    fs::write(&target, "A=1\n").unwrap();
    symlink(&target, file(&instance)).unwrap();
    assert!(matches!(get(&instance, "A"), Err(EnvError::Io { .. })));
    assert!(matches!(list(&instance), Err(EnvError::Io { .. })));
    assert!(matches!(set(&instance, "B", "2"), Err(EnvError::Io { .. })));
    assert_eq!(fs::read_to_string(&target).unwrap(), "A=1\n");
    // A link where the temporary file goes is refused as well, and nothing is written through it.
    let plain = with_env("link-temp", Some("A=1\n"));
    let outside = plain.paths.config.join("outside");
    fs::write(&outside, "kept").unwrap();
    symlink(&outside, plain.paths.config.join(".env.tmp")).unwrap();
    assert!(set(&plain, "B", "2").is_err());
    assert_eq!(fs::read_to_string(&outside).unwrap(), "kept");
    assert_eq!(fs::read_to_string(file(&plain)).unwrap(), "A=1\n");
}

#[test]
fn a_linked_configuration_directory_is_followed() {
    let instance = with_env("linked-config", Some("A=1\n"));
    let real = instance.paths.config.clone();
    let mut linked = instance.clone();
    linked.paths.config = real.with_file_name("link");
    symlink(&real, &linked.paths.config).unwrap();
    assert_eq!(get(&linked, "A").unwrap().as_deref(), Some("1"));
    set(&linked, "B", "2").unwrap();
    assert_eq!(fs::read_to_string(real.join(".env")).unwrap(), "A=1\nB=2");
}

#[test]
fn the_flags_read_as_the_reference_argument_parser_reads_them() {
    for (given, read) in [
        ("plain", "plain"),
        ("42", "42"),
        ("-5", "-5"),
        ("007", "007"),
        ("0x10", "16"),
        ("0X1f", "31"),
        ("0x", "0x"),
        ("1e3", "1000"),
        ("1E3", "1E3"),
        (".5", "0.5"),
        ("5.", "5"),
        ("1.50", "1.5"),
        ("1e21", "1e21"),
        ("1e15", "1000000000000000"),
        ("9007199254740993", "9007199254740993"),
        ("9007199254740991", "9007199254740991"),
        ("1e400", "1e400"),
        ("1-2", "1-2"),
        ("0.5", "0.5"),
        ("00.5", "00.5"),
        ("--x", "--x"),
        ("a b", "a b"),
        ("0", "0"),
        ("-0", "0"),
        ("+5", "+5"),
        ("0.0", "0"),
        ("1e-7", "1e-7"),
        ("-.5", "-0.5"),
        ("0e5", "0e5"),
        ("08.5", "08.5"),
        ("Infinity", "Infinity"),
        ("NaN", "NaN"),
        ("1_000", "1_000"),
        (" 5", " 5"),
    ] {
        assert_eq!(flag_text(given), read, "{given}");
    }
}

#[test]
fn the_texts_of_the_commands() {
    let instance = with_env("text", Some("A=1\nLONGER_KEY=two words\nEMPTY=\n"));
    assert_eq!(get_text(&instance, "A").unwrap(), "1\n");
    // The reference validates `--key` as text before it reads the file: a key its argument
    // parser reads as a number, or an empty one, is refused whatever the file holds.
    let absent = with_env("text-absent", None);
    for (key, message) in [
        ("0x10", "\"key\" must be a string"),
        ("42", "\"key\" must be a string"),
        ("", "\"key\" is not allowed to be empty"),
    ] {
        for on in [&instance, &absent] {
            assert_eq!(get_text(on, key).unwrap_err().to_string(), message, "{key}");
        }
    }
    assert_eq!(
        get_text(&instance, "007").unwrap_err().to_string(),
        "The \"007\" doesn't exist"
    );
    // The node's parser leaves out a variable whose value is empty, so it does not exist.
    assert_eq!(
        get_text(&instance, "EMPTY").unwrap_err().to_string(),
        "The \"EMPTY\" doesn't exist"
    );
    assert_eq!(
        list_text(&instance).unwrap(),
        "Key         Value\nA           1\nLONGER_KEY  two words\n"
    );
    assert_eq!(set_text(&instance, "N", "0x10").unwrap(), "");
    assert_eq!(get(&instance, "N").unwrap().as_deref(), Some("16"));
    let table = paths_text(&instance).unwrap();
    let rows: Vec<&str> = table.lines().collect();
    assert_eq!(rows.len(), 6);
    assert_eq!(rows[0], "Type    Path");
    for (row, kind) in rows[1..]
        .iter()
        .zip(["data", "config", "cache", "log", "temp"])
    {
        assert!(row.starts_with(kind), "{row}");
    }
    // The reference's order: env-paths' keys, data first.
    assert!(rows[1].ends_with(&instance.paths.data.display().to_string()));
    assert!(rows[2].ends_with(&instance.paths.config.display().to_string()));
}
