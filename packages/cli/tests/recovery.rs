//! The recovery commands' marker writing and the trust rule the relay shares with them.

use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::Command;

use heartwood_cli::instance::{InstanceFlags, resolve};
use heartwood_cli::recovery::{
    Confirm, Kind, Marker, Meta, RecoveryError, Trust, Untrusted, judge, marker_trust, own_group,
    request, temp_trust,
};
use heartwood_cli::seams::{Instance, Role};

/// A fresh directory for one test, under the target's temporary directory.
fn scratch(test: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("cli-recovery")
        .join(test);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// The relay's instance in `home`, with its temp directory (`CORE_PATH_TEMP`) and its data
/// directory named.
fn instance(home: &Path, vars: &[(&str, &str)]) -> Instance {
    let vars: HashMap<String, String> = vars
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect();
    let env = move |name: &str| vars.get(name).cloned();
    let flags = InstanceFlags {
        token: Some("node1".to_owned()),
        network: Some("devnet".to_owned()),
    };
    resolve(Role::Relay, &flags, &env, home, home).unwrap()
}

/// An instance whose temp and data directories are `home/tmp` and `home/data`.
fn simple(home: &Path) -> Instance {
    let (tmp, data) = (home.join("tmp"), home.join("data"));
    instance(
        home,
        &[
            ("CORE_PATH_TEMP", tmp.to_str().unwrap()),
            ("CORE_PATH_DATA", data.to_str().unwrap()),
        ],
    )
}

/// A confirmation that answers as told and records the questions.
struct Answer {
    yes: Option<bool>,
    asked: Vec<String>,
}

impl Answer {
    fn new(yes: Option<bool>) -> Answer {
        Answer {
            yes,
            asked: Vec::new(),
        }
    }
}

impl Confirm for Answer {
    fn confirm(&mut self, question: &str) -> Option<bool> {
        self.asked.push(question.to_owned());
        self.yes
    }
}

fn mode(path: &Path) -> u32 {
    fs::symlink_metadata(path).unwrap().mode() & 0o7777
}

fn me() -> u32 {
    fs::metadata(env!("CARGO_TARGET_TMPDIR")).unwrap().uid()
}

/// The group of the files this process makes, and whether it is the user's own.
fn my_group() -> (u32, bool) {
    let gid = nix::unistd::Gid::effective().as_raw();
    (gid, own_group(me(), gid))
}

/// `path` made with a umask of 002 (a child process, so that no other test sees the umask).
fn mkdir_umask_002(path: &Path) {
    let done = Command::new("sh")
        .args(["-c", "umask 002 && mkdir -p \"$1\"", "sh"])
        .arg(path)
        .status()
        .unwrap();
    assert!(done.success());
}

#[test]
fn the_markers_are_the_references_files_and_questions() {
    assert_eq!(Marker::ClearPool.file_name(), "clear-pool.lock");
    assert_eq!(
        Marker::ClearSavedStates.file_name(),
        "clear-saved-states.lock"
    );
    assert_eq!(
        Marker::ClearPool.question(),
        "Clear all unconfirmed transactions from your pool on next startup?"
    );
    assert_eq!(
        Marker::ClearSavedStates.question(),
        "Clear all saved states so a fresh state will be generated on startup?"
    );
}

#[test]
fn a_confirmed_command_writes_an_empty_private_marker_and_creates_the_directory_0700() {
    let home = scratch("writes");
    let instance = simple(&home);
    for (marker, yes) in [(Marker::ClearPool, true), (Marker::ClearSavedStates, true)] {
        let mut answer = Answer::new(Some(yes));
        let path = request(&instance, marker, false, &mut answer).unwrap();
        assert_eq!(path, home.join("tmp").join(marker.file_name()));
        assert_eq!(answer.asked, [marker.question()]);
        assert_eq!(fs::read(&path).unwrap(), b"");
        assert_eq!(mode(&path), 0o600);
    }
    assert_eq!(mode(&home.join("tmp")), 0o700);
}

#[test]
fn the_default_temp_directory_is_the_relays_own() {
    let home = scratch("default-temp");
    let tmp = home.join("tmpdir");
    let instance = instance(&home, &[("TMPDIR", tmp.to_str().unwrap())]);
    assert!(instance.paths.temp.starts_with(&tmp));
    let path = request(&instance, Marker::ClearPool, true, &mut Answer::new(None)).unwrap();
    assert_eq!(path, instance.paths.temp.join("clear-pool.lock"));
    assert!(path.is_file());
}

#[test]
fn force_asks_nothing() {
    let home = scratch("force");
    let mut answer = Answer::new(Some(false));
    let path = request(&simple(&home), Marker::ClearPool, true, &mut answer).unwrap();
    assert!(answer.asked.is_empty());
    assert!(path.is_file());
}

#[test]
fn without_a_yes_nothing_is_written() {
    for (name, yes) in [("no", Some(false)), ("none", None)] {
        let home = scratch(&format!("refused-{name}"));
        let error = request(
            &simple(&home),
            Marker::ClearPool,
            false,
            &mut Answer::new(yes),
        );
        let error = error.unwrap_err();
        assert!(matches!(error, RecoveryError::NotConfirmed));
        assert_eq!(
            error.to_string(),
            "You'll need to confirm the input to continue"
        );
        assert!(!home.join("tmp").exists());
    }
}

#[test]
fn a_marker_that_exists_is_kept() {
    let home = scratch("exists");
    let instance = simple(&home);
    let path = request(&instance, Marker::ClearPool, true, &mut Answer::new(None)).unwrap();
    fs::write(&path, b"kept").unwrap();
    request(&instance, Marker::ClearPool, true, &mut Answer::new(None)).unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"kept");
}

#[test]
fn a_marker_that_is_a_symbolic_link_is_not_followed() {
    let home = scratch("symlink");
    let instance = simple(&home);
    let target = home.join("target");
    fs::create_dir_all(home.join("tmp")).unwrap();
    fs::set_permissions(home.join("tmp"), fs::Permissions::from_mode(0o700)).unwrap();
    symlink(&target, home.join("tmp/clear-pool.lock")).unwrap();
    let error = request(&instance, Marker::ClearPool, true, &mut Answer::new(None)).unwrap_err();
    assert!(matches!(error, RecoveryError::Io { .. }));
    assert!(error.to_string().contains("clear-pool.lock"));
    assert!(!target.exists());
}

#[test]
fn a_marker_the_relay_would_ignore_is_refused() {
    let home = scratch("untrusted-marker");
    let instance = simple(&home);
    fs::create_dir_all(instance.paths.data.join("run")).unwrap();
    let path = request(&instance, Marker::ClearPool, true, &mut Answer::new(None)).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o666)).unwrap();
    let error = request(&instance, Marker::ClearPool, true, &mut Answer::new(None)).unwrap_err();
    assert!(matches!(
        error,
        RecoveryError::Untrusted { why: Untrusted::WorldWritable { mode: 0o666 }, path: ref p }
            if *p == path
    ));
    assert!(error.to_string().contains("any user may write it"));
}

#[test]
fn a_temp_directory_that_cannot_be_written_is_an_error_naming_the_marker() {
    let home = scratch("unwritable");
    let instance = simple(&home);
    let tmp = home.join("tmp");
    fs::create_dir_all(&tmp).unwrap();
    fs::set_permissions(&tmp, fs::Permissions::from_mode(0o500)).unwrap();
    let error = request(&instance, Marker::ClearPool, true, &mut Answer::new(None)).unwrap_err();
    fs::set_permissions(&tmp, fs::Permissions::from_mode(0o700)).unwrap();
    let marker = tmp.join("clear-pool.lock");
    assert!(matches!(error, RecoveryError::Io { ref path, .. } if *path == marker));
    assert!(!marker.exists());
}

#[test]
fn a_temp_directory_any_user_may_write_is_refused_and_nothing_is_written() {
    let home = scratch("world-writable");
    let instance = simple(&home);
    let tmp = home.join("tmp");
    fs::create_dir_all(&tmp).unwrap();
    fs::set_permissions(&tmp, fs::Permissions::from_mode(0o777)).unwrap();
    let error = request(&instance, Marker::ClearPool, true, &mut Answer::new(None)).unwrap_err();
    assert!(matches!(
        error,
        RecoveryError::Untrusted {
            why: Untrusted::WorldWritable { mode: 0o777 },
            ..
        }
    ));
    let text = error.to_string();
    assert!(text.contains(tmp.to_str().unwrap()) && text.contains("any user may write it"));
    assert!(!tmp.join("clear-pool.lock").exists());
}

#[test]
fn a_temp_directory_that_is_a_file_or_a_link_is_refused() {
    let home = scratch("wrong-kind");
    let instance = simple(&home);
    let tmp = home.join("tmp");
    fs::write(&tmp, b"").unwrap();
    let error = request(&instance, Marker::ClearPool, true, &mut Answer::new(None)).unwrap_err();
    assert!(matches!(
        error,
        RecoveryError::Io { .. } | RecoveryError::Untrusted { .. }
    ));
    fs::remove_file(&tmp).unwrap();
    let real = home.join("real");
    fs::create_dir(&real).unwrap();
    symlink(&real, &tmp).unwrap();
    let error = request(&instance, Marker::ClearPool, true, &mut Answer::new(None)).unwrap_err();
    assert!(matches!(
        error,
        RecoveryError::Untrusted {
            why: Untrusted::WrongKind {
                expected: Kind::Directory
            },
            ..
        }
    ));
    assert!(!real.join("clear-pool.lock").exists());
}

#[test]
fn a_temp_directory_that_cannot_be_made_is_an_error_naming_it() {
    let home = scratch("unmakeable");
    let blocker = home.join("blocker");
    fs::write(&blocker, b"").unwrap();
    let tmp = blocker.join("tmp");
    let instance = instance(&home, &[("CORE_PATH_TEMP", tmp.to_str().unwrap())]);
    let error = request(&instance, Marker::ClearPool, true, &mut Answer::new(None)).unwrap_err();
    assert!(matches!(error, RecoveryError::Io { ref path, .. } if *path == tmp));
    assert!(error.to_string().starts_with(tmp.to_str().unwrap()));
}

#[test]
fn a_group_writable_directory_of_the_users_own_umask_is_trusted() {
    let home = scratch("umask-002");
    let tmp = home.join("tmp");
    mkdir_umask_002(&tmp);
    assert_eq!(mode(&tmp), 0o775);
    let instance = simple(&home);
    let (gid, own) = my_group();
    if !own {
        // A user whose files get a shared group: its members could plant a marker there.
        let why = Untrusted::GroupWritable { mode: 0o775, gid };
        let path = tmp.clone();
        let verdict = temp_trust(&tmp, &instance.paths.data).unwrap();
        assert_eq!(verdict, Trust::Untrusted { path, why });
        return;
    }
    assert_eq!(
        temp_trust(&tmp, &instance.paths.data).unwrap(),
        Trust::Trusted
    );
    let path = request(
        &instance,
        Marker::ClearSavedStates,
        true,
        &mut Answer::new(None),
    )
    .unwrap();
    assert!(path.is_file());
}

fn meta(kind: Kind, uid: u32, mode: u32) -> Meta {
    Meta {
        kind,
        uid,
        mode,
        gid: 7,
        own_group: true,
        links: 1,
    }
}

#[test]
fn a_directory_a_shared_group_may_write_and_a_linked_marker_are_not_trusted() {
    let path = Path::new("/x");
    let bad = |why| Trust::Untrusted {
        path: path.to_owned(),
        why,
    };
    let (file, dir) = (Kind::File, Kind::Directory);
    let shared = |kind, mode| Meta {
        own_group: false,
        gid: 100,
        ..meta(kind, 7, mode)
    };
    assert_eq!(
        judge(path, &shared(dir, 0o775), dir, 7),
        bad(Untrusted::GroupWritable {
            mode: 0o775,
            gid: 100
        })
    );
    assert_eq!(judge(path, &shared(dir, 0o755), dir, 7), Trust::Trusted);
    assert_eq!(judge(path, &meta(dir, 7, 0o775), dir, 7), Trust::Trusted);
    // The file's own group bit lets no one place it: its directory decides that.
    assert_eq!(judge(path, &shared(file, 0o664), file, 7), Trust::Trusted);
    let linked = Meta {
        links: 2,
        ..meta(file, 7, 0o600)
    };
    assert_eq!(
        judge(path, &linked, file, 7),
        bad(Untrusted::Linked { links: 2 })
    );
    let directory = Meta {
        links: 5,
        ..meta(dir, 7, 0o700)
    };
    assert_eq!(judge(path, &directory, dir, 7), Trust::Trusted);
}

#[test]
fn the_relay_ignores_a_marker_in_a_directory_of_another_group_or_one_hard_linked_there() {
    let home = scratch("group-and-link");
    let (tmp, data) = (home.join("tmp"), home.join("data"));
    fs::create_dir_all(&tmp).unwrap();
    fs::create_dir_all(data.join("run")).unwrap();
    fs::set_permissions(&tmp, fs::Permissions::from_mode(0o700)).unwrap();
    let marker = tmp.join("clear-pool.lock");
    // A file of the user linked in as the marker, as a user who may read and write it can.
    fs::write(home.join("elsewhere"), b"").unwrap();
    fs::hard_link(home.join("elsewhere"), &marker).unwrap();
    assert!(matches!(
        marker_trust(&marker, &data),
        Some(Trust::Untrusted { why: Untrusted::Linked { links: 2 }, ref path }) if *path == marker
    ));
    fs::remove_file(&marker).unwrap();
    fs::write(&marker, b"").unwrap();
    assert_eq!(marker_trust(&marker, &data), Some(Trust::Trusted));
    // The directory given to another group of the user, which its members may write.
    let (primary, _) = my_group();
    let groups = nix::unistd::getgroups().unwrap();
    let Some(other) = groups.iter().map(|g| g.as_raw()).find(|&g| g != primary) else {
        eprintln!("skipped: the user is in no group but its primary one");
        return;
    };
    nix::unistd::chown(&tmp, None, Some(nix::unistd::Gid::from_raw(other))).unwrap();
    fs::set_permissions(&tmp, fs::Permissions::from_mode(0o775)).unwrap();
    let verdict = marker_trust(&marker, &data);
    if own_group(me(), other) {
        assert_eq!(verdict, Some(Trust::Trusted));
    } else {
        let why = Untrusted::GroupWritable {
            mode: 0o775,
            gid: other,
        };
        assert_eq!(
            verdict,
            Some(Trust::Untrusted {
                path: tmp.clone(),
                why
            })
        );
    }
    fs::set_permissions(&tmp, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(marker_trust(&marker, &data), Some(Trust::Trusted));
}

#[test]
fn the_command_run_as_another_user_than_the_relays_makes_nothing() {
    let home = scratch("other-user");
    let instance = simple(&home);
    // The relay's `run` directory is the root directory's, whose owner is user 0.
    fs::create_dir_all(&instance.paths.data).unwrap();
    symlink("/", instance.paths.data.join("run")).unwrap();
    if me() == 0 {
        eprintln!("skipped: the test runs as user 0, the owner of the relay's files here");
        return;
    }
    let error = request(&instance, Marker::ClearPool, true, &mut Answer::new(None)).unwrap_err();
    assert!(matches!(
        error,
        RecoveryError::OtherUser { owner: 0, uid } if uid == me()
    ));
    assert!(error.to_string().contains("run the command as that user"));
    assert!(!instance.paths.temp.exists());
}

#[test]
fn the_rule_judges_kind_then_owner_then_the_world_write_bit() {
    let path = Path::new("/x");
    let bad = |why| Trust::Untrusted {
        path: path.to_owned(),
        why,
    };
    let file = Kind::File;
    assert_eq!(judge(path, &meta(file, 7, 0o600), file, 7), Trust::Trusted);
    assert_eq!(judge(path, &meta(file, 7, 0o664), file, 7), Trust::Trusted);
    assert_eq!(judge(path, &meta(file, 7, 0o775), file, 7), Trust::Trusted);
    assert_eq!(
        judge(path, &meta(file, 7, 0o666), file, 7),
        bad(Untrusted::WorldWritable { mode: 0o666 })
    );
    assert_eq!(
        judge(path, &meta(file, 7, 0o602), file, 7),
        bad(Untrusted::WorldWritable { mode: 0o602 })
    );
    assert_eq!(
        judge(path, &meta(file, 8, 0o600), file, 7),
        bad(Untrusted::OtherOwner { uid: 8, owner: 7 })
    );
    assert_eq!(
        judge(path, &meta(Kind::Other, 7, 0o600), file, 7),
        bad(Untrusted::WrongKind { expected: file })
    );
    assert_eq!(
        judge(path, &meta(file, 7, 0o600), Kind::Directory, 7),
        bad(Untrusted::WrongKind {
            expected: Kind::Directory
        })
    );
    // The kind is judged first, then the owner: a link owned by another user is a wrong kind.
    assert_eq!(
        judge(path, &meta(Kind::Other, 8, 0o777), file, 7),
        bad(Untrusted::WrongKind { expected: file })
    );
}

#[test]
fn the_relays_check_reads_the_marker_and_its_directory_against_the_owner_of_run() {
    let home = scratch("marker-trust");
    let (tmp, data) = (home.join("tmp"), home.join("data"));
    fs::create_dir_all(&tmp).unwrap();
    fs::create_dir_all(data.join("run")).unwrap();
    fs::set_permissions(&tmp, fs::Permissions::from_mode(0o700)).unwrap();
    let marker = tmp.join("clear-pool.lock");
    assert_eq!(marker_trust(&marker, &data), None);
    fs::write(&marker, b"").unwrap();
    fs::set_permissions(&marker, fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(marker_trust(&marker, &data), Some(Trust::Trusted));
    // The group's write bit is allowed on the file, and on its directory of the user's own group.
    fs::set_permissions(&marker, fs::Permissions::from_mode(0o660)).unwrap();
    fs::set_permissions(&tmp, fs::Permissions::from_mode(0o770)).unwrap();
    if my_group().1 {
        assert_eq!(marker_trust(&marker, &data), Some(Trust::Trusted));
    }
    // Any user's write bit is not, on either.
    fs::set_permissions(&marker, fs::Permissions::from_mode(0o666)).unwrap();
    let verdict = marker_trust(&marker, &data);
    assert!(matches!(
        verdict,
        Some(Trust::Untrusted { why: Untrusted::WorldWritable { .. }, ref path }) if *path == marker
    ));
    fs::set_permissions(&marker, fs::Permissions::from_mode(0o600)).unwrap();
    fs::set_permissions(&tmp, fs::Permissions::from_mode(0o777)).unwrap();
    let verdict = marker_trust(&marker, &data);
    assert!(matches!(
        verdict,
        Some(Trust::Untrusted { why: Untrusted::WorldWritable { .. }, ref path }) if *path == tmp
    ));
    fs::set_permissions(&tmp, fs::Permissions::from_mode(0o700)).unwrap();
    // A symbolic link is no file, and a relay that never ran has no user to compare with.
    fs::remove_file(&marker).unwrap();
    fs::write(home.join("real"), b"").unwrap();
    symlink(home.join("real"), &marker).unwrap();
    assert!(matches!(
        marker_trust(&marker, &data),
        Some(Trust::Untrusted {
            why: Untrusted::WrongKind {
                expected: Kind::File
            },
            ..
        })
    ));
    fs::remove_dir(data.join("run")).unwrap();
    assert_eq!(marker_trust(&marker, &data), None);
}

#[test]
fn meta_reads_a_link_as_neither_a_file_nor_a_directory() {
    let home = scratch("meta");
    fs::write(home.join("f"), b"").unwrap();
    symlink(home.join("f"), home.join("l")).unwrap();
    assert_eq!(Meta::of(&home).unwrap().kind, Kind::Directory);
    assert_eq!(Meta::of(&home.join("f")).unwrap().kind, Kind::File);
    assert_eq!(Meta::of(&home.join("l")).unwrap().kind, Kind::Other);
    assert_eq!(Meta::of(&home.join("f")).unwrap().uid, me());
    assert!(Meta::of(&home.join("missing")).is_err());
}
