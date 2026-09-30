//! `pool clear` and `state clear` against the relay's start: the marker a command writes is the
//! file the relay reads (whatever `CORE_PATH_TEMP` says and whatever umask made the directory),
//! the state clear replays the chain and keeps the online record, the pool clear empties the
//! pool's storage, and the binary asks, refuses and stays silent as the commands say.

mod common;

use std::fs;
use std::io::Write;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use common::{Setup, run};
use heartwood_blockchain::{ReplayReason, RestorePath};
use heartwood_cli::instance::{InstanceFlags, resolve};
use heartwood_cli::recovery::{Confirm, Marker, request};
use heartwood_cli::seams::{Instance, Role};
use heartwood_core::app::{open_pool, restored};
use heartwood_core::config::{Package, RelayConfig};
use heartwood_core::options;
use heartwood_crypto::TransactionId;
use heartwood_crypto::identities::PublicKeyBytes;
use heartwood_pool::{DelegateVersions, Storage, StoredTransaction};

fn load(setup: &Setup, extra: &[(&str, &str)]) -> RelayConfig {
    let (flags, vars) = (run("--network testnet"), setup.process(extra));
    RelayConfig::load(&flags, &vars, &setup.home(), &setup.root).unwrap()
}

/// The instance a command resolves for `setup`'s relay: the same variables, the same names.
fn command_instance(setup: &Setup) -> Instance {
    let vars = setup.process(&[]);
    let env = move |name: &str| vars.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone());
    let flags = InstanceFlags {
        token: Some("solar".to_owned()),
        network: Some("testnet".to_owned()),
    };
    resolve(Role::Relay, &flags, &env, &setup.home(), &setup.root).unwrap()
}

struct Yes;

impl Confirm for Yes {
    fn confirm(&mut self, _: &str) -> Option<bool> {
        Some(true)
    }
}

/// The run directory a relay's process lock makes, whose owner is the markers' user.
fn locked(config: &RelayConfig) {
    fs::create_dir_all(config.paths.data.join("run")).unwrap();
}

fn stored_row(byte: u8) -> StoredTransaction {
    StoredTransaction {
        height: 1,
        id: TransactionId::from_array([byte; 32]),
        recipient_id: String::new(),
        sender_id: "sender".into(),
        serialised: vec![1, 2, 3],
    }
}

fn pool_storage(config: &RelayConfig) -> PathBuf {
    pool_options(config).storage
}

fn rows(config: &RelayConfig) -> usize {
    let storage = Storage::open(&pool_storage(config), None).unwrap();
    storage.all().unwrap().len()
}

fn pool_options(config: &RelayConfig) -> heartwood_pool::Options {
    let entry = config.entry(Package::Pool);
    options::pool(&entry, &config.env, &config.paths.data, 1).unwrap()
}

/// `path` made with a umask of 002, as a child process does (no test sees the umask).
fn mkdir_umask_002(path: &Path) {
    let made = Command::new("sh")
        .args(["-c", "umask 002 && mkdir -p \"$1\"", "sh"])
        .arg(path)
        .status()
        .unwrap();
    assert!(made.success());
}

#[test]
fn the_marker_a_command_writes_is_the_one_the_relay_reads() {
    let setup = Setup::new("clear-paths", "");
    let config = load(&setup, &[]);
    let instance = command_instance(&setup);
    assert_eq!(instance.paths.temp, config.paths.temp);
    assert_eq!(instance.paths.data, config.paths.data);
    let state = request(&instance, Marker::ClearSavedStates, true, &mut Yes).unwrap();
    let pool = request(&instance, Marker::ClearPool, false, &mut Yes).unwrap();
    assert_eq!(state, config.replay_marker());
    assert_eq!(pool, config.pool_marker());
}

#[test]
fn a_state_clear_replays_from_the_genesis_block_and_keeps_the_online_record() {
    let setup = Setup::new("clear-state", "");
    let config = load(&setup, &[]);
    locked(&config);
    let (mut store, first) = restored(&config).unwrap();
    assert_eq!(first.path, RestorePath::FirstStart);
    let mut record = DelegateVersions::new();
    record.set(PublicKeyBytes::from_array([2; 33]), 3, "4.3.1".into());
    record.set(PublicKeyBytes::from_array([3; 33]), 4, "4.3.2".into());
    store.save_online(&record).unwrap();
    drop(store);
    request(
        &command_instance(&setup),
        Marker::ClearSavedStates,
        true,
        &mut Yes,
    )
    .unwrap();
    let (_, replayed) = restored(&config).unwrap();
    assert_eq!(replayed.path, RestorePath::Replay(ReplayReason::Requested));
    assert_eq!(replayed.online, record);
    assert!(!config.replay_marker().exists());
    // The next start is an ordinary one, and the record is still there.
    let (_, again) = restored(&config).unwrap();
    assert_eq!(again.path, RestorePath::Table);
    assert_eq!(again.online, record);
}

#[test]
fn only_the_reset_variable_empties_the_online_record() {
    let setup = Setup::new("clear-online-reset", "");
    let config = load(&setup, &[]);
    locked(&config);
    let (mut store, _) = restored(&config).unwrap();
    let mut record = DelegateVersions::new();
    record.set(PublicKeyBytes::from_array([2; 33]), 3, "4.3.1".into());
    store.save_online(&record).unwrap();
    drop(store);
    assert_eq!(restored(&config).unwrap().1.online, record);
    let reset = load(&setup, &[("CORE_RESET_DATABASE", "1")]);
    assert_eq!(restored(&reset).unwrap().1.online, DelegateVersions::new());
}

#[test]
fn a_pool_clear_empties_the_pool_storage_once() {
    let setup = Setup::new("clear-pool", "");
    let config = load(&setup, &[]);
    locked(&config);
    let mut storage = Storage::open(&pool_storage(&config), None).unwrap();
    storage.flush().unwrap();
    storage.add(&stored_row(7)).unwrap();
    drop(storage);
    drop(open_pool(&config, pool_options(&config)).unwrap());
    assert_eq!(rows(&config), 1);
    request(&command_instance(&setup), Marker::ClearPool, true, &mut Yes).unwrap();
    drop(open_pool(&config, pool_options(&config)).unwrap());
    assert_eq!(rows(&config), 0);
    assert!(!config.pool_marker().exists());
    // What the pool stores afterwards stays: the marker was used once.
    let mut storage = Storage::open(&pool_storage(&config), None).unwrap();
    storage.add(&stored_row(8)).unwrap();
    drop(storage);
    drop(open_pool(&config, pool_options(&config)).unwrap());
    assert_eq!(rows(&config), 1);
}

#[test]
fn a_directory_made_under_umask_002_is_honoured_and_one_any_user_may_write_is_not() {
    let setup = Setup::new("clear-umask", "");
    let config = load(&setup, &[]);
    locked(&config);
    let instance = command_instance(&setup);
    fs::remove_dir_all(&config.paths.temp).ok();
    mkdir_umask_002(&config.paths.temp);
    let made = fs::metadata(&config.paths.temp).unwrap();
    assert_eq!(made.permissions().mode() & 0o777, 0o775);
    if !heartwood_cli::recovery::own_group(made.uid(), made.gid()) {
        eprintln!("skipped: the user's files get a shared group, whose members may write there");
        return;
    }
    assert_eq!(restored(&config).unwrap().1.path, RestorePath::FirstStart);
    request(&instance, Marker::ClearSavedStates, true, &mut Yes).unwrap();
    // The file is 0600 and its directory 0775: the group's write bit does not matter.
    let (_, replayed) = restored(&config).unwrap();
    assert_eq!(replayed.path, RestorePath::Replay(ReplayReason::Requested));
    // Another user's write access to the directory: the relay ignores what is in it, and the
    // command refuses to write into it.
    fs::write(config.replay_marker(), "").unwrap();
    let open = fs::Permissions::from_mode(0o777);
    fs::set_permissions(&config.paths.temp, open).unwrap();
    assert_eq!(restored(&config).unwrap().1.path, RestorePath::Table);
    assert!(config.replay_marker().exists());
    let refused = request(&instance, Marker::ClearPool, true, &mut Yes).unwrap_err();
    assert!(refused.to_string().contains("any user may write it"));
    assert!(!config.pool_marker().exists());
}

#[test]
fn a_reset_variable_empties_the_pool_whatever_the_markers_trust() {
    let setup = Setup::new("clear-reset-untrusted", "");
    let config = load(&setup, &[]);
    locked(&config);
    let mut storage = Storage::open(&pool_storage(&config), None).unwrap();
    storage.flush().unwrap();
    storage.add(&stored_row(9)).unwrap();
    drop(storage);
    // A marker in a directory every user may write is not the relay's: ignored, and left.
    fs::create_dir_all(&config.paths.temp).unwrap();
    fs::write(config.pool_marker(), "").unwrap();
    let open = fs::Permissions::from_mode(0o777);
    fs::set_permissions(&config.paths.temp, open).unwrap();
    drop(open_pool(&config, pool_options(&config)).unwrap());
    assert_eq!(rows(&config), 1);
    assert!(config.pool_marker().exists());
    // A reset variable flushes the pool all the same, and leaves the marker where it was.
    let reset = load(&setup, &[("CORE_RESET_POOL", "1")]);
    drop(open_pool(&reset, pool_options(&reset)).unwrap());
    assert_eq!(rows(&config), 0);
    assert!(config.pool_marker().exists());
}

/// A node's directories in a scratch root, for the binary.
fn node(name: &str) -> PathBuf {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("core-clear")
        .join(name);
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    root
}

/// `heartwood <args>` in `root`, with `stdin` written to it.
fn heartwood(root: &Path, args: &[&str], stdin: &str) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_heartwood"));
    command.args(args).current_dir(root);
    command.env("HOME", root).env("TMPDIR", root.join("tmp"));
    for kind in ["CONFIG", "DATA", "CACHE", "LOG", "TEMP"] {
        command.env_remove(format!("CORE_PATH_{kind}"));
    }
    command.stdin(Stdio::piped());
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command.spawn().unwrap();
    let mut input = child.stdin.take().unwrap();
    let _ = input.write_all(stdin.as_bytes());
    drop(input);
    child.wait_with_output().unwrap()
}

/// The temp directory the binary uses for `solar` on `testnet` under `root`.
fn temp_of(root: &Path) -> PathBuf {
    let user = root.file_name().unwrap();
    root.join("tmp").join(user).join("solar-core/testnet")
}

const NAMES: [&str; 4] = ["--token", "solar", "--network", "testnet"];

#[test]
fn the_commands_ask_the_references_questions_and_write_the_markers() {
    for (words, file, question) in [
        (
            ["pool", "clear"],
            "clear-pool.lock",
            "Clear all unconfirmed transactions from your pool on next startup?",
        ),
        (
            ["state", "clear"],
            "clear-saved-states.lock",
            "Clear all saved states so a fresh state will be generated on startup?",
        ),
    ] {
        let root = node(&format!("asks-{}", words[0]));
        let args: Vec<&str> = words.iter().chain(NAMES.iter()).copied().collect();
        // No answer, or a no: the reference's refusal, exit 1, nothing written.
        for input in ["", "n\n", "\n"] {
            let out = heartwood(&root, &args, input);
            assert_eq!(out.status.code(), Some(1), "{input:?}");
            let text = String::from_utf8_lossy(&out.stderr);
            assert!(
                text.contains("You'll need to confirm the input to continue"),
                "{text}"
            );
            assert!(String::from_utf8_lossy(&out.stdout).contains(question));
            assert!(!temp_of(&root).join(file).exists());
        }
        // A yes writes it, and the command is silent apart from the question.
        let out = heartwood(&root, &args, "y\n");
        assert_eq!(out.status.code(), Some(0));
        assert!(out.stderr.is_empty());
        assert_eq!(fs::read(temp_of(&root).join(file)).unwrap(), b"");
        let mode = fs::metadata(temp_of(&root)).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
    }
}

#[test]
fn the_first_y_or_n_typed_decides_as_the_references_prompt_does() {
    let mut args = vec!["pool", "clear"];
    args.extend(NAMES);
    let marker = |root: &Path| temp_of(root).join("clear-pool.lock");
    // The reference's confirm prompt submits at the first y or n, whatever the case, ignores any
    // other key, and takes the return key before either as a no.
    for (name, input) in [("yeah", "yeah\n"), ("upper", "Y\n"), ("after", "ok y\n")] {
        let root = node(&format!("typed-{name}"));
        let out = heartwood(&root, &args, input);
        assert_eq!(out.status.code(), Some(0), "{input:?}");
        assert!(marker(&root).is_file(), "{input:?}");
    }
    for (name, input) in [
        ("nope", "nope\n"),
        ("upper-n", "N y\n"),
        ("other", "x\ny\n"),
    ] {
        let root = node(&format!("typed-{name}"));
        let out = heartwood(&root, &args, input);
        assert_eq!(out.status.code(), Some(1), "{input:?}");
        assert!(!marker(&root).exists(), "{input:?}");
    }
}

#[test]
fn force_writes_the_marker_in_silence_and_a_blocked_directory_is_exit_1() {
    let root = node("force");
    let mut args = vec!["pool", "clear", "--force"];
    args.extend(NAMES);
    let out = heartwood(&root, &args, "");
    assert_eq!(out.status.code(), Some(0));
    assert!(out.stdout.is_empty() && out.stderr.is_empty());
    assert!(temp_of(&root).join("clear-pool.lock").is_file());
    // The reference exits 0 and writes nothing where the directory cannot be made; this exits 1.
    let blocked = node("blocked");
    fs::write(blocked.join("tmp"), b"").unwrap();
    let out = heartwood(&blocked, &args, "");
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("tmp"));
    // A temp directory any user may write is refused, naming it and why.
    let open = node("open");
    let temp = temp_of(&open);
    fs::create_dir_all(&temp).unwrap();
    fs::set_permissions(&temp, fs::Permissions::from_mode(0o777)).unwrap();
    let out = heartwood(&open, &args, "");
    assert_eq!(out.status.code(), Some(1));
    let text = String::from_utf8_lossy(&out.stderr);
    assert!(text.contains(temp.to_str().unwrap()) && text.contains("any user may write it"));
    assert!(!temp.join("clear-pool.lock").exists());
}

#[test]
fn an_instance_that_cannot_be_named_is_exit_1() {
    let root = node("unnamed");
    let out = heartwood(&root, &["state", "clear", "--force"], "");
    assert_eq!(out.status.code(), Some(1));
    assert!(!out.stderr.is_empty());
}
