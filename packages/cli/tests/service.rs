//! `service install|uninstall`: the systemd units against golden files, the sandboxing a host's
//! systemd supports, the unit checked by `systemd-analyze` where the host has it, no service
//! manager called, and what a unit's start leaves for `status` and `stop` (the manager's record).

use std::collections::HashMap;
use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use heartwood_cli::ProcessError;
use heartwood_cli::argv::rewrite;
use heartwood_cli::doctor::seams::RoleState;
use heartwood_cli::process::{self};
use heartwood_cli::process_table::render;
use heartwood_cli::seams::{Instance, Role, SupervisedStatus};
use heartwood_cli::service::{
    Manager, ServiceError, install, install_text, render as render_unit,
    render_for as render_for_account, uninstall, uninstall_text,
};
use heartwood_cli::supervisor::manager::{MANAGER_VARIABLE, ManagerRecord};
use heartwood_cli::supervisor::signals::{is_locked, record_manager, take_lock};
use heartwood_cli::supervisor::{RunDir, run_dir};
use heartwood_kernel::paths::Paths;

/// The program every golden unit runs.
const BINARY: &str = "/usr/local/bin/heartwood";

/// The account every system unit here runs as.
const ACCOUNT: &str = "node";

/// The unit for a host whose systemd has `version`, a system unit running as [`ACCOUNT`].
fn render_for(
    instance: &Instance,
    role: Role,
    manager: Manager,
    binary: &Path,
    version: Option<u32>,
) -> Result<String, ServiceError> {
    render_for_account(instance, role, manager, binary, version, ACCOUNT)
}

/// The instance of the golden units: the default directories of an account whose home is
/// `/srv/node`.
fn golden_instance(role: Role) -> Instance {
    let home = Path::new("/srv/node");
    Instance {
        token: "node1".into(),
        network: "devnet".into(),
        role,
        paths: Paths {
            config: home.join(".config/node1-core/devnet"),
            data: home.join(".local/share/node1-core/devnet"),
            cache: home.join(".cache/node1-core/devnet"),
            log: home.join(".local/state/node1-core/devnet"),
            temp: PathBuf::from("/tmp/node/node1-core/devnet"),
        },
    }
}

fn scratch(test: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("cli-service")
        .join(test);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Wait until the lock at `path` is free. A lock this test drops stays held while a child that
/// another test is spawning still has a copy of its descriptor, until that child's exec closes it.
fn wait_released(path: &Path) {
    for _ in 0..250 {
        if !is_locked(path).unwrap() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    panic!("the lock at {} was not released", path.display());
}

/// Set in the child process that [`isolated`] runs: the test's scratch directory.
const CHILD: &str = "HEARTWOOD_SERVICE_TEST_CHILD";

/// The scratch directory of `test`, and whether this process runs the test's body. The first
/// call makes the directory and runs `test` again in a child process whose HOME,
/// `XDG_CONFIG_HOME` and `XDG_RUNTIME_DIR` lie inside it, with the variables `env` gives for
/// that directory, and asserts that the child passed; the child is then told to run the body.
/// So no test reads or writes the host's own unit directories.
fn isolated(
    test: &str,
    env: impl FnOnce(&Path) -> Vec<(&'static str, OsString)>,
) -> (PathBuf, bool) {
    if let Some(root) = std::env::var_os(CHILD) {
        return (PathBuf::from(root), true);
    }
    let root = scratch(test);
    let home = root.join("home");
    fs::create_dir_all(&home).unwrap();
    let ran = Command::new(std::env::current_exe().unwrap())
        .args([test, "--exact", "--nocapture"])
        .env(CHILD, &root)
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_RUNTIME_DIR", root.join("run"))
        .envs(env(&root))
        .output()
        .unwrap();
    let said =
        String::from_utf8_lossy(&ran.stdout).into_owned() + &String::from_utf8_lossy(&ran.stderr);
    assert!(ran.status.success() && said.contains("1 passed"), "{said}");
    (root, false)
}

/// The user manager's own unit directory and its runtime one, as [`isolated`] places them.
fn user_dirs(root: &Path) -> (PathBuf, PathBuf) {
    (
        root.join("home/.config/systemd/user"),
        root.join("run/systemd/user"),
    )
}

fn instance(root: &Path, role: Role) -> Instance {
    Instance {
        token: "node1".into(),
        network: "devnet".into(),
        role,
        paths: Paths {
            config: root.join("config"),
            data: root.join("data"),
            cache: root.join("cache"),
            log: root.join("log"),
            temp: root.join("temp"),
        },
    }
}

fn golden(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/units");
    fs::read_to_string(path.join(name)).unwrap()
}

#[test]
fn units_equal_their_golden_files() {
    let binary = Path::new(BINARY);
    let user = |role| {
        render_for(
            &golden_instance(role),
            role,
            Manager::SystemdUser,
            binary,
            Some(255),
        )
    };
    let system = |role| {
        render_for(
            &golden_instance(role),
            role,
            Manager::SystemdSystem,
            binary,
            Some(255),
        )
    };
    assert_eq!(user(Role::Relay).unwrap(), golden("user-relay-255.service"));
    assert_eq!(
        system(Role::Forger).unwrap(),
        golden("system-forger-255.service")
    );
    // A host whose systemd is not known gets no sandboxing at all.
    let core = render_for(
        &golden_instance(Role::Core),
        Role::Core,
        Manager::SystemdUser,
        binary,
        None,
    );
    assert_eq!(core.unwrap(), golden("user-core-unknown.service"));
}

#[test]
fn a_unit_has_the_settings_the_plan_names_and_only_the_sandboxing_a_host_supports() {
    let root = scratch("settings");
    let unit = |version, manager| {
        render_for(
            &instance(&root, Role::Relay),
            Role::Relay,
            manager,
            Path::new(BINARY),
            version,
        )
        .unwrap()
    };
    for manager in [Manager::SystemdUser, Manager::SystemdSystem] {
        let text = unit(Some(255), manager);
        for line in [
            "Type=exec",
            "ExecStart=/usr/local/bin/heartwood relay run --token=node1 --network=devnet",
            "Restart=always",
            "KillSignal=SIGINT",
            "TimeoutStopSec=30",
            "UMask=0077",
            "NoNewPrivileges=yes",
            "StartLimitIntervalSec=10",
            "StartLimitBurst=5",
            "Environment=CORE_SERVICE_MANAGER=systemd",
        ] {
            assert!(
                text.lines().any(|l| l == line),
                "{manager:?}: {line}\n{text}"
            );
        }
    }
    let has = |text: &str, directive: &str| text.lines().any(|line| line == directive);
    // Each directive only from the systemd version that has it.
    let old = unit(Some(232), Manager::SystemdSystem);
    assert!(has(&old, "ProtectKernelTunables=yes") && has(&old, "RestrictRealtime=yes"));
    assert!(!has(&old, "RestrictSUIDSGID=yes") && !has(&old, "LockPersonality=yes"));
    let ancient = unit(Some(200), Manager::SystemdSystem);
    assert!(
        !ancient.contains("Protect") && !ancient.contains("Restrict"),
        "{ancient}"
    );
    // A user manager gets none that needs a mount namespace.
    let user = unit(Some(255), Manager::SystemdUser);
    for directive in [
        "PrivateTmp=yes",
        "ProtectSystem=full",
        "ProtectKernelTunables=yes",
    ] {
        assert!(!has(&user, directive), "{directive}");
    }
    assert!(has(&user, "RestrictSUIDSGID=yes"));
    // The unit is for the manager's targets.
    assert!(user.contains("WantedBy=default.target") && !user.contains("network-online"));
    let system = unit(Some(255), Manager::SystemdSystem);
    assert!(
        system.contains("WantedBy=multi-user.target")
            && system.contains("After=network-online.target")
    );
}

#[test]
fn a_binary_path_is_quoted_for_systemd_and_a_relative_one_refused() {
    let root = scratch("quote");
    let relay = instance(&root, Role::Relay);
    let unit = |path: &str| {
        render_for(
            &relay,
            Role::Relay,
            Manager::SystemdUser,
            Path::new(path),
            None,
        )
    };
    let text = unit("/opt/my heartwood/100%/bin/$x\"y").unwrap();
    let exec = text.lines().find(|l| l.starts_with("ExecStart=")).unwrap();
    assert!(
        exec.starts_with("ExecStart=\"/opt/my heartwood/100%%/bin/$$x\\\"y\" relay run"),
        "{exec}"
    );
    assert!(unit("heartwood").is_err());
    assert!(unit("/opt/line\nbreak").is_err());
    // A path that is not UTF-8 is refused, not written with a replacement character.
    use std::os::unix::ffi::OsStrExt;
    let path = Path::new(std::ffi::OsStr::from_bytes(b"/opt/\xff/heartwood"));
    let refused = render_for(&relay, Role::Relay, Manager::SystemdUser, path, None);
    assert!(refused.is_err(), "{refused:?}");
    // A name with a line break cannot start another setting.
    let mut odd = relay.clone();
    odd.token = "a\nExecStartPre=/bin/evil".into();
    let text = render_for(
        &odd,
        Role::Relay,
        Manager::SystemdUser,
        Path::new(BINARY),
        None,
    )
    .unwrap();
    assert!(
        !text.lines().any(|l| l.starts_with("ExecStartPre")),
        "{text}"
    );
}

#[test]
fn install_writes_the_unit_into_the_directory_and_uninstall_removes_it() {
    let root = scratch("install");
    let out = root.join("units");
    let relay = instance(&root, Role::Relay);
    let binary = Path::new(BINARY);
    let done = install(
        &relay,
        Role::Relay,
        Manager::SystemdUser,
        binary,
        Some(&out),
    )
    .unwrap();
    assert_eq!(done.unit, "node1-devnet-relay.service");
    assert_eq!(done.path, out.join("node1-devnet-relay.service"));
    let text = fs::read_to_string(&done.path).unwrap();
    assert_eq!(
        text,
        render_unit(&relay, Role::Relay, Manager::SystemdUser, binary).unwrap()
    );
    assert_eq!(
        fs::metadata(&done.path).unwrap().permissions().mode() & 0o777,
        0o644
    );
    // Written through a rename: nothing else is left in the directory.
    assert_eq!(fs::read_dir(&out).unwrap().count(), 1);
    // Another role of the instance has its own unit, which an uninstall of the first leaves.
    let forger = install(
        &relay,
        Role::Forger,
        Manager::SystemdUser,
        binary,
        Some(&out),
    )
    .unwrap();
    assert_eq!(forger.unit, "node1-devnet-forger.service");
    uninstall(&relay, Role::Relay, Manager::SystemdUser, Some(&out)).unwrap();
    assert!(!done.path.exists() && forger.path.exists());
    assert!(uninstall(&relay, Role::Relay, Manager::SystemdUser, Some(&out)).is_err());
    // A link in the unit's place is refused, and neither it nor its target is written.
    let target = root.join("elsewhere");
    fs::write(&target, "kept").unwrap();
    std::os::unix::fs::symlink(&target, &done.path).unwrap();
    let refused = install(
        &relay,
        Role::Relay,
        Manager::SystemdUser,
        binary,
        Some(&out),
    )
    .unwrap_err();
    assert!(refused.to_string().contains("symbolic link"), "{refused}");
    assert_eq!(fs::read_to_string(&target).unwrap(), "kept");
    assert!(fs::symlink_metadata(&done.path).unwrap().is_symlink());
}

#[test]
fn the_text_the_commands_print_names_the_commands_that_enable_and_disable_the_unit() {
    let name = "the_text_the_commands_print_names_the_commands_that_enable_and_disable_the_unit";
    let (root, child) = isolated(name, |_| Vec::new());
    if !child {
        return;
    }
    let out = root.join("units");
    let relay = instance(&root, Role::Relay);
    let binary = Path::new(BINARY);
    let said = install_text(
        &relay,
        Role::Relay,
        Manager::SystemdUser,
        binary,
        Some(&out),
    )
    .unwrap();
    assert!(
        said.contains(&out.join("node1-devnet-relay.service").display().to_string()),
        "{said}"
    );
    assert!(said.contains("systemctl --user daemon-reload\nsystemctl --user enable --now node1-devnet-relay.service"), "{said}");
    // A unit that is neither enabled nor running is removed, and the reload is what is left.
    let said = uninstall_text(&relay, Role::Relay, Manager::SystemdUser, Some(&out)).unwrap();
    assert_eq!(
        said,
        "Removed node1-devnet-relay.service\nRun:\nsystemctl --user daemon-reload\n"
    );
    // A system unit with no directory is printed, and /etc is not written: the commands are
    // comments, so the text saved as the unit is still one.
    let said = install_text(&relay, Role::Relay, Manager::SystemdSystem, binary, None).unwrap();
    assert!(said.starts_with("[Unit]\n"), "{said}");
    assert!(
        said.contains("# Save this as /etc/systemd/system/node1-devnet-relay.service"),
        "{said}"
    );
    assert!(
        said.contains("# systemctl enable --now node1-devnet-relay.service\n"),
        "{said}"
    );
    // Without a directory nothing is removed: the steps are printed in their order.
    let said = uninstall_text(&relay, Role::Relay, Manager::SystemdSystem, None).unwrap();
    assert_eq!(
        said,
        "Run:\nsystemctl disable --now node1-devnet-relay.service\nthen remove \
         /etc/systemd/system/node1-devnet-relay.service and run:\nsystemctl daemon-reload\n"
    );
}

/// The unit `systemd-analyze` verifies: it is skipped, with a note, where the host lacks it.
#[test]
fn systemd_analyze_verifies_the_units() {
    let found = Command::new("systemd-analyze")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    if !found.is_ok_and(|status| status.success()) {
        eprintln!("systemd-analyze is not on this host: the units are not verified");
        return;
    }
    let root = scratch("analyze");
    // The unit runs a program that exists, which `verify` checks.
    let binary = std::env::current_exe().unwrap();
    for (manager, flags) in [
        (Manager::SystemdUser, &["--user"][..]),
        (Manager::SystemdSystem, &[]),
    ] {
        for role in [Role::Relay, Role::Forger, Role::Core] {
            let out = root.join(format!("{manager:?}-{role}"));
            let relay = instance(&root, role);
            let done = install(&relay, role, manager, &binary, Some(&out)).unwrap();
            let verified = Command::new("systemd-analyze")
                .args(flags)
                .arg("verify")
                .arg(&done.path)
                .output()
                .unwrap();
            let said = String::from_utf8_lossy(&verified.stderr);
            assert!(
                verified.status.success() && said.trim().is_empty(),
                "{manager:?} {role}: {said}"
            );
        }
    }
}

/// The commands run with every systemd tool a fake that leaves a mark on the `PATH`.
#[test]
fn no_service_manager_is_called() {
    let binary = Path::new(BINARY);
    let fakes = |root: &Path| {
        let bin = root.join("bin");
        fs::create_dir_all(&bin).unwrap();
        let tools = [
            "systemctl",
            "systemd-analyze",
            "systemd-run",
            "loginctl",
            "busctl",
            "machinectl",
        ];
        for tool in tools {
            let path = bin.join(tool);
            let mark = root.join("called");
            fs::write(
                &path,
                format!("#!/bin/sh\necho {tool} >> {}\n", mark.display()),
            )
            .unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let path = format!(
            "{}:{}",
            bin.display(),
            std::env::var("PATH").unwrap_or_default()
        );
        vec![("PATH", OsString::from(path))]
    };
    let (root, child) = isolated("no_service_manager_is_called", fakes);
    let out = root.join("units");
    if child {
        // The child: the commands over every role and manager, writing only into `out`.
        let relay = instance(&out, Role::Relay);
        for manager in [Manager::SystemdUser, Manager::SystemdSystem] {
            for role in [Role::Relay, Role::Forger, Role::Core] {
                install_text(&relay, role, manager, binary, Some(&out)).unwrap();
                if manager == Manager::SystemdSystem {
                    install_text(&relay, role, manager, binary, None).unwrap();
                    uninstall_text(&relay, role, manager, None).unwrap();
                }
                uninstall_text(&relay, role, manager, Some(&out)).unwrap();
            }
        }
        return;
    }
    let mark = root.join("called");
    assert!(
        !mark.exists(),
        "a service manager was called: {}",
        fs::read_to_string(&mark).unwrap()
    );
    // And the code that writes units has no way to run one.
    let source = include_str!("../src/service.rs");
    assert!(
        !source.contains("Command") && !source.contains("process::"),
        "service.rs runs a process"
    );
}

/// The environment a unit's `Environment=` lines give its process, as systemd reads them: one
/// assignment per line here, unquoted, its escapes undone and its specifiers expanded (`%n` is
/// `name`, the unit's name, and `%%` a `%`).
fn unit_environment(unit: &str, name: &str) -> HashMap<String, String> {
    let lines = unit
        .lines()
        .filter_map(|line| line.strip_prefix("Environment="));
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
    lines
        .map(read)
        .filter_map(|pair| {
            let (k, v) = pair.split_once('=')?;
            Some((k.to_owned(), v.to_owned()))
        })
        .collect()
}

#[test]
fn under_a_unit_status_names_the_manager_and_stop_and_restart_refuse() {
    let root = scratch("managed");
    let relay = instance(&root, Role::Relay);
    run_dir(&relay).unwrap();
    // The node holds its lock, and its pid file names a process that a wrong `stop` would signal.
    let lock = take_lock(&relay.lock_path()).unwrap().unwrap();
    let mut bystander = Command::new("sleep").arg("30").spawn().unwrap();
    fs::write(relay.pid_path(), bystander.id().to_string()).unwrap();
    let row = process::status(&relay).unwrap();
    assert_eq!(row.status, SupervisedStatus::OnlineUnsupervised);

    // The unit's own environment is what the node's start records.
    let unit = render_unit(&relay, Role::Relay, Manager::SystemdUser, Path::new(BINARY)).unwrap();
    let vars = unit_environment(&unit, "node1-devnet-relay.service");
    assert_eq!(
        vars.get(MANAGER_VARIABLE).map(String::as_str),
        Some("systemd")
    );
    let env = |name: &str| vars.get(name).cloned();
    let dir = RunDir::beside(&relay.lock_path()).unwrap();
    record_manager(&dir, &relay, &env).unwrap();
    let record = ManagerRecord::read(&relay.manager_path()).unwrap().unwrap();
    assert_eq!(record.unit, "node1-devnet-relay.service");
    // The unit names itself, so a unit saved under another name is recorded as that one.
    let renamed = unit_environment(&unit, "relay-a.service");
    let renamed_env = |name: &str| renamed.get(name).cloned();
    record_manager(&dir, &relay, &renamed_env).unwrap();
    let record = ManagerRecord::read(&relay.manager_path()).unwrap().unwrap();
    assert_eq!(record.unit, "relay-a.service");
    record_manager(&dir, &relay, &env).unwrap();

    let row = process::status(&relay).unwrap();
    assert_eq!(row.status, SupervisedStatus::OnlineSystemd);
    assert_eq!(row.status.name(), "online (systemd)");
    assert!(row.status.is_running() && row.pid == Some(bystander.id()));
    // The doctor counts the manager as the node's supervisor: no "unsupervised" warning.
    let state = heartwood_cli::doctor::seams::role_state(row.status);
    assert_eq!(state, RoleState::Running { supervised: true });
    assert!(render(&[row]).contains("online (systemd)"));
    for verb in ["stop", "restart"] {
        let refused = match verb {
            "stop" => process::stop(&relay).unwrap_err(),
            _ => process::restart(&relay, &Vec::new()).unwrap_err(),
        };
        assert!(
            matches!(
                refused,
                ProcessError::Managed {
                    manager: "systemd",
                    ..
                }
            ),
            "{refused:?}"
        );
        let said = refused.to_string();
        let user = format!("`systemctl --user {verb} node1-devnet-relay.service`");
        assert!(said.contains(&user), "{said}");
        assert!(
            said.contains(&format!("`systemctl {verb} node1-devnet-relay.service`")),
            "{said}"
        );
    }
    // Nothing was signalled.
    assert!(bystander.try_wait().unwrap().is_none());

    // A start that names no manager removes the record: the node is unsupervised again.
    record_manager(&dir, &relay, &|_| None).unwrap();
    assert!(!relay.manager_path().exists());
    assert_eq!(
        process::status(&relay).unwrap().status,
        SupervisedStatus::OnlineUnsupervised
    );

    // A record an earlier node left names nothing once the lock is free.
    record_manager(&dir, &relay, &env).unwrap();
    assert!(is_locked(&relay.lock_path()).unwrap());
    drop(lock);
    wait_released(&relay.lock_path());
    assert_eq!(
        process::status(&relay).unwrap().status,
        SupervisedStatus::Stopped
    );
    assert!(matches!(
        process::stop(&relay),
        Err(ProcessError::NotRunning { .. })
    ));
    bystander.kill().unwrap();
    bystander.wait().unwrap();
}

#[test]
fn every_colon_spelling_of_service_is_its_spaced_form() {
    let args = |words: &[&str]| -> Vec<OsString> {
        ["heartwood"].iter().chain(words).map(Into::into).collect()
    };
    let more = ["--role", "relay", "--network", "devnet", "--system"];
    for (colon, spaced) in [
        ("service:install", &["service", "install"][..]),
        ("service:uninstall", &["service", "uninstall"]),
    ] {
        let colon = rewrite(args(&[&[colon][..], &more].concat()));
        assert_eq!(colon, args(&[spaced, &more].concat()), "{colon:?}");
    }
}

#[test]
fn a_unit_names_every_directory_of_the_instance_it_was_installed_for() {
    let root = scratch("paths");
    let mut relay = instance(&root, Role::Relay);
    // A data directory moved to another disk, with characters systemd would otherwise read.
    relay.paths.data = root.join("ssd data/100% \"fast\"\\db");
    for manager in [Manager::SystemdUser, Manager::SystemdSystem] {
        let text = render_for(&relay, Role::Relay, manager, Path::new(BINARY), Some(255)).unwrap();
        let vars = unit_environment(&text, "node1-devnet-relay.service");
        let paths = &relay.paths;
        for (kind, dir) in [
            ("CONFIG", &paths.config),
            ("DATA", &paths.data),
            ("CACHE", &paths.cache),
            ("LOG", &paths.log),
            ("TEMP", &paths.temp),
        ] {
            let name = format!("CORE_PATH_{kind}");
            assert_eq!(
                vars.get(&name),
                dir.to_str().map(str::to_owned).as_ref(),
                "{name}\n{text}"
            );
        }
    }
    // A directory with a line break cannot start another setting: it is refused.
    let mut odd = relay.clone();
    odd.paths.log = root.join("log\nExecStartPre=/bin/evil");
    let refused = render_for(
        &odd,
        Role::Relay,
        Manager::SystemdUser,
        Path::new(BINARY),
        None,
    );
    assert!(refused.is_err(), "{refused:?}");
}

#[test]
fn no_unit_hides_the_shared_temporary_directory_from_its_node() {
    let root = scratch("private-tmp");
    let relay = instance(&root, Role::Relay);
    for manager in [Manager::SystemdUser, Manager::SystemdSystem] {
        let text = render_for(&relay, Role::Relay, manager, Path::new(BINARY), Some(255)).unwrap();
        // The markers of `state clear` and `pool clear` are in the instance's temp directory,
        // which a private /tmp would hide from the relay.
        for directive in [
            "PrivateTmp",
            "ProtectHome",
            "TemporaryFileSystem",
            "InaccessiblePaths",
        ] {
            assert!(
                !text.contains(directive),
                "{manager:?}: {directive}\n{text}"
            );
        }
    }
}

#[test]
fn uninstall_keeps_a_unit_that_is_enabled_or_runs_and_says_to_disable_it_first() {
    let name = "uninstall_keeps_a_unit_that_is_enabled_or_runs_and_says_to_disable_it_first";
    let (root, child) = isolated(name, |_| Vec::new());
    if !child {
        return;
    }
    let out = root.join("units");
    let relay = instance(&root, Role::Relay);
    let unit = "node1-devnet-relay.service";
    let done = install(
        &relay,
        Role::Relay,
        Manager::SystemdUser,
        Path::new(BINARY),
        Some(&out),
    )
    .unwrap();
    // `systemctl --user enable` links the unit into its target's directory.
    let wants = out.join("default.target.wants");
    fs::create_dir_all(&wants).unwrap();
    std::os::unix::fs::symlink(&done.path, wants.join(unit)).unwrap();
    let refused = uninstall_text(&relay, Role::Relay, Manager::SystemdUser, Some(&out));
    let said = refused.expect_err("an enabled unit is kept").to_string();
    assert!(
        done.path.exists(),
        "the unit file was removed while enabled"
    );
    let disable = format!("systemctl --user disable --now {unit}");
    assert!(said.contains(&disable), "{said}");
    fs::remove_file(wants.join(unit)).unwrap();

    // A node the unit runs (its lock held, its start's record naming the unit) keeps it too.
    run_dir(&relay).unwrap();
    let lock = take_lock(&relay.lock_path()).unwrap().unwrap();
    let dir = RunDir::beside(&relay.lock_path()).unwrap();
    let env = |name: &str| (name == MANAGER_VARIABLE).then(|| "systemd".to_owned());
    record_manager(&dir, &relay, &env).unwrap();
    let refused = uninstall_text(&relay, Role::Relay, Manager::SystemdUser, Some(&out));
    let said = refused.expect_err("a running unit is kept").to_string();
    assert!(done.path.exists() && said.contains(&disable), "{said}");
    drop(lock);
    wait_released(&relay.lock_path());

    // Disabled and stopped: the file goes, and only the reload is left to do.
    let said = uninstall_text(&relay, Role::Relay, Manager::SystemdUser, Some(&out)).unwrap();
    assert!(!done.path.exists());
    assert!(said.contains("systemctl --user daemon-reload"), "{said}");
    assert!(!said.contains("disable"), "{said}");
}

#[test]
fn uninstall_looks_for_the_enable_link_in_the_managers_own_and_runtime_directories() {
    let name = "uninstall_looks_for_the_enable_link_in_the_managers_own_and_runtime_directories";
    let (root, child) = isolated(name, |_| Vec::new());
    if !child {
        return;
    }
    let (own, runtime) = user_dirs(&root);
    let relay = instance(&root, Role::Relay);
    let unit = "node1-devnet-relay.service";
    let binary = Path::new(BINARY);
    let user = Manager::SystemdUser;
    let in_use = |dir: Option<&Path>| match uninstall_text(&relay, Role::Relay, user, dir) {
        Err(ServiceError::InUse {
            unit: named, steps, ..
        }) => {
            assert_eq!(named, unit);
            assert!(steps.starts_with(&format!("systemctl --user disable --now {unit}\n")));
        }
        other => panic!("an enabled unit was not kept: {other:?}"),
    };
    // With no directory the unit goes into the manager's own, and an enable link in its
    // target's directory there, or in the runtime directory, keeps it.
    let placed = install(&relay, Role::Relay, user, binary, None).unwrap();
    assert_eq!(placed.path, own.join(unit));
    for base in [&own, &runtime] {
        let wants = base.join("default.target.wants");
        fs::create_dir_all(&wants).unwrap();
        std::os::unix::fs::symlink(&placed.path, wants.join(unit)).unwrap();
        in_use(None);
        assert!(placed.path.exists(), "{}", base.display());
        // A unit written elsewhere is kept too while the manager has it enabled.
        let out = root.join("units");
        install(&relay, Role::Relay, user, binary, Some(&out)).unwrap();
        in_use(Some(&out));
        assert!(out.join(unit).exists());
        fs::remove_file(wants.join(unit)).unwrap();
    }
    // Neither enabled nor running: a unit written elsewhere goes, and the copy placed in the
    // manager's own directory is named, as it stays.
    let out = root.join("units");
    let said = uninstall_text(&relay, Role::Relay, user, Some(&out)).unwrap();
    assert_eq!(
        said,
        format!(
            "Removed {unit}\nA copy at {} stays: remove it too, then run:\n\
             systemctl --user daemon-reload\n",
            placed.path.display()
        )
    );
    assert!(!out.join(unit).exists() && placed.path.exists());
    let said = uninstall_text(&relay, Role::Relay, user, None).unwrap();
    assert_eq!(
        said,
        format!("Removed {unit}\nRun:\nsystemctl --user daemon-reload\n")
    );
    assert!(!placed.path.exists());
    // A system unit written elsewhere is kept by its link in that directory's target.
    let system = Manager::SystemdSystem;
    let out = root.join("system");
    let written = install(&relay, Role::Relay, system, binary, Some(&out)).unwrap();
    let wants = out.join("multi-user.target.wants");
    fs::create_dir_all(&wants).unwrap();
    std::os::unix::fs::symlink(&written.path, wants.join(unit)).unwrap();
    match uninstall_text(&relay, Role::Relay, system, Some(&out)) {
        Err(ServiceError::InUse { steps, .. }) => {
            assert!(steps.starts_with(&format!("systemctl disable --now {unit}\n")));
            assert!(steps.contains(" --system "), "{steps}");
        }
        other => panic!("an enabled system unit was not kept: {other:?}"),
    }
    assert!(written.path.exists());
}

#[test]
fn under_a_unit_kill_refuses_log_names_the_journal_and_top_lists_the_node() {
    let root = scratch("managed-more");
    let relay = instance(&root, Role::Relay);
    run_dir(&relay).unwrap();
    let lock = take_lock(&relay.lock_path()).unwrap().unwrap();
    let mut bystander = Command::new("sleep").arg("30").spawn().unwrap();
    fs::write(relay.pid_path(), bystander.id().to_string()).unwrap();
    let dir = RunDir::beside(&relay.lock_path()).unwrap();
    let env = |name: &str| (name == MANAGER_VARIABLE).then(|| "systemd".to_owned());
    record_manager(&dir, &relay, &env).unwrap();
    let unit = "node1-devnet-relay.service";

    // `kill` would leave the manager to start the node again: it names the manager's stop.
    let refused = process::kill(&relay).unwrap_err();
    assert!(
        matches!(refused, ProcessError::Managed { .. }),
        "{refused:?}"
    );
    assert!(
        refused
            .to_string()
            .contains(&format!("systemctl --user stop {unit}")),
        "{refused}"
    );
    assert!(
        bystander.try_wait().unwrap().is_none(),
        "kill signalled the node"
    );

    // The node's output is in the journal, not in a capture.
    let said = process::log(&relay, 5, false, false)
        .unwrap_err()
        .to_string();
    assert!(
        said.contains(&format!("journalctl --user -u {unit}")),
        "{said}"
    );

    // `top` lists a node a unit runs from its index entry, with no supervisor.
    let index = root.join("index");
    heartwood_cli::instance_index::write(&index, &relay).unwrap();
    let rows = heartwood_cli::top::top("node1", &index).unwrap();
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].status, SupervisedStatus::OnlineSystemd);
    drop(lock);
    wait_released(&relay.lock_path());
    assert!(heartwood_cli::top::top("node1", &index).unwrap().is_empty());
    bystander.kill().unwrap();
    bystander.wait().unwrap();
}

#[test]
fn a_system_unit_runs_as_the_account_that_installed_it_and_a_user_unit_names_none() {
    let root = scratch("account");
    let relay = instance(&root, Role::Relay);
    let unit = |manager, account| {
        render_for_account(
            &relay,
            Role::Relay,
            manager,
            Path::new(BINARY),
            None,
            account,
        )
    };
    let system = unit(Manager::SystemdSystem, "node-op").unwrap();
    assert_eq!(
        system.lines().filter(|l| l.starts_with("User=")).count(),
        1,
        "{system}"
    );
    assert!(system.lines().any(|l| l == "User=node-op"), "{system}");
    let user = unit(Manager::SystemdUser, "node-op").unwrap();
    assert!(!user.contains("User="), "{user}");
    // An account given by its uid, as for one with no name, is written as it is.
    assert!(
        unit(Manager::SystemdSystem, "1000")
            .unwrap()
            .contains("\nUser=1000\n")
    );
    // A name systemd would not read, or one that would start another setting, is refused.
    for odd in ["", "a b", "a\nExecStartPre=/bin/evil", "%n"] {
        assert!(unit(Manager::SystemdSystem, odd).is_err(), "{odd:?}");
    }
    // `render` writes the account that runs it.
    let rendered = render_unit(
        &relay,
        Role::Relay,
        Manager::SystemdSystem,
        Path::new(BINARY),
    );
    let rendered = rendered.unwrap();
    let uid = nix::unistd::geteuid();
    let name = nix::unistd::User::from_uid(uid).ok().flatten();
    let expected = name.map_or_else(|| uid.to_string(), |user| user.name);
    assert!(
        rendered.lines().any(|l| l == format!("User={expected}")),
        "{rendered}"
    );
}
