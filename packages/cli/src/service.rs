//! `service install|uninstall`: a systemd unit for one role of an instance (a user unit by
//! default, a system unit with `--system`), written only: no service manager is called, and the
//! commands to enable the unit are printed. The unit sets the service-manager variables the start
//! records ([`crate::supervisor::manager`]): the manager, and the unit's own name (`%n`, so a unit
//! saved under another name is recorded as that one), so `status` names the manager and `stop`
//! refuses.
//!
//! A unit runs `<binary> <role> run`, the node itself with no supervisor of Heartwood's: the
//! manager restarts it, stops it with SIGINT and gives it 30 s. It names the five directories of
//! the instance as `service install` resolved them (`CORE_PATH_*`), so the node under the unit
//! and the commands run where `service install` ran use one data directory, one lock and one
//! signing record, whatever the manager's own environment holds. A system unit runs as the
//! account that ran `service install` (`User=`), whose directories those are.
//!
//! Only the sandboxing directives the host's systemd has are written, and a user unit gets none
//! that need a mount namespace, which a user manager may not be allowed to make. No unit gets a
//! private `/tmp`: the marker files of `state clear` and `pool clear` are in the instance's temp
//! directory, under `/tmp` by default, and the relay under the unit must see them.
//!
//! `service uninstall` keeps a unit that is enabled (its link in the target's `.wants` directory
//! of the manager's unit directory, or of `--out`'s) or whose node runs (its lock held and its
//! start's record naming the unit), and says to disable it first: `systemctl disable` fails on a
//! unit whose file is gone, which would leave the node running and the unit enabled.

use std::fs;
use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::config::{ConfigError, io, write_atomic};
use crate::errors::ProcessError;
use crate::seams::{Instance, Role};
use crate::supervisor::manager::{ManagerRecord, UNIT_VARIABLE};
use crate::supervisor::signals::is_locked;

/// Where the units of a system manager live.
const SYSTEM_DIR: &str = "/etc/systemd/system";

/// Where the units of a system manager's runtime enablement live.
const SYSTEM_RUNTIME_DIR: &str = "/run/systemd/system";

/// The sandboxing directives, the systemd version that has each, and whether it needs a mount
/// namespace. No `PrivateTmp=`: it would hide the recovery commands' markers from the relay.
const SANDBOX: [(&str, u32, bool); 9] = [
    ("SystemCallArchitectures=native", 209, false),
    ("RestrictRealtime=yes", 231, false),
    ("RestrictNamespaces=yes", 233, false),
    ("LockPersonality=yes", 235, false),
    ("RestrictSUIDSGID=yes", 242, false),
    ("ProtectSystem=full", 214, true),
    ("ProtectKernelTunables=yes", 232, true),
    ("ProtectKernelModules=yes", 232, true),
    ("ProtectControlGroups=yes", 232, true),
];

/// Where a unit goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Manager {
    /// A systemd user unit.
    SystemdUser,
    /// A systemd system unit.
    SystemdSystem,
}

/// What `service install` wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    /// The unit file.
    pub path: PathBuf,
    /// The unit's name.
    pub unit: String,
}

/// A service command failed.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ServiceError {
    /// A file could not be read or written, or a link is in its place.
    #[error(transparent)]
    Config(#[from] ConfigError),
    /// A lock or the manager's record of the instance could not be read.
    #[error(transparent)]
    Process(#[from] ProcessError),
    /// `service uninstall` of a unit that is enabled or whose node runs: the file is kept.
    #[error(
        "{unit} is {why}, so its file is kept: disable it first, while the file is in place, \
         then uninstall it again:\n{steps}"
    )]
    InUse {
        /// The unit's name.
        unit: String,
        /// What keeps it: enabled (and where) or running.
        why: String,
        /// The commands to run, one per line.
        steps: String,
    },
}

/// The unit's name for `role` of `instance`: `<token>-<network>-<role>.service`.
fn unit_name(instance: &Instance, role: Role) -> String {
    format!("{}-{}-{role}.service", instance.token, instance.network)
}

/// The systemd version of this host, read from the name of its shared library, which runs
/// nothing; `None` when it is not found.
fn host_version() -> Option<u32> {
    let dirs = [
        "/usr/lib",
        "/usr/lib64",
        "/lib",
        "/usr/lib/x86_64-linux-gnu",
        "/usr/lib/aarch64-linux-gnu",
    ];
    let names = dirs
        .iter()
        .filter_map(|dir| fs::read_dir(format!("{dir}/systemd")).ok());
    let names = names
        .flatten()
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok());
    let version = |name: String| {
        let digits = name.strip_prefix("libsystemd-shared-")?;
        digits
            .split(|c: char| !c.is_ascii_digit())
            .next()?
            .parse()
            .ok()
    };
    names.filter_map(version).max()
}

/// A word of an `ExecStart=` line: quoted where systemd would otherwise split it, and with the
/// characters it expands (`%`, `$`) and the quotes escaped.
fn word(text: &str) -> String {
    let plain = |c: char| c.is_ascii_alphanumeric() || "-_./=:@+,".contains(c);
    if !text.is_empty() && text.chars().all(plain) {
        return text.to_owned();
    }
    let escaped = text.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{}\"", escaped.replace('%', "%%").replace('$', "$$"))
}

/// An `Environment=` line that sets `name` to `value`: the assignment quoted where systemd would
/// otherwise split it or read an escape, with the backslashes and quotes escaped and the
/// specifier sign `%` doubled (systemd expands no `$` in it). A value that is not UTF-8 or holds
/// a control character is refused: a line break would start another setting.
fn environment(name: &str, value: &Path) -> Result<String, ServiceError> {
    let text = value.to_str().unwrap_or("\0");
    if text.chars().any(char::is_control) {
        return Err(io(value, "not a UTF-8 path without control codes").into());
    }
    let pair = format!("{name}={text}");
    let plain = |c: char| c.is_ascii_alphanumeric() || "-_./=:@+,".contains(c);
    if pair.chars().all(plain) {
        return Ok(format!("Environment={pair}\n"));
    }
    let escaped = pair.replace('\\', "\\\\").replace('"', "\\\"");
    Ok(format!("Environment=\"{}\"\n", escaped.replace('%', "%%")))
}

/// The account that runs this command, which a system unit runs as: its name, or its uid when
/// it has none.
fn account() -> String {
    let uid = nix::unistd::geteuid();
    let user = nix::unistd::User::from_uid(uid).ok().flatten();
    user.map_or_else(|| uid.to_string(), |user| user.name)
}

/// The unit's text for `role` of `instance` under `manager`, running `binary`, with the
/// sandboxing of this host's systemd; a system unit runs as the account that runs this.
pub fn render(
    instance: &Instance,
    role: Role,
    manager: Manager,
    binary: &Path,
) -> Result<String, ServiceError> {
    render_for(instance, role, manager, binary, host_version(), &account())
}

/// [`render`] for a host whose systemd has `version` (no sandboxing for a host that is not
/// known), a system unit running as `account`.
pub fn render_for(
    instance: &Instance,
    role: Role,
    manager: Manager,
    binary: &Path,
    version: Option<u32>,
    account: &str,
) -> Result<String, ServiceError> {
    // A path that is not UTF-8 is refused with the others: its stand-in is a control character.
    let text = binary.to_str().unwrap_or("\0");
    if !binary.is_absolute() || text.chars().any(char::is_control) {
        return Err(io(binary, "not an absolute UTF-8 path without control codes").into());
    }
    let clean = |text: &str| text.chars().filter(|c| !c.is_control()).collect::<String>();
    let (token, network) = (clean(&instance.token), clean(&instance.network));
    let user = manager == Manager::SystemdUser;
    let (t, n) = (format!("--token={token}"), format!("--network={network}"));
    let exec = [text, role.name(), "run", &t, &n].map(word);
    let mut unit = format!(
        "[Unit]\nDescription=Heartwood {role} of {token} on {network}\n\
         StartLimitIntervalSec=10\nStartLimitBurst=5\n{}\n[Service]\nType=exec\n\
         ExecStart={}\nRestart=always\nKillSignal=SIGINT\nTimeoutStopSec=30\nUMask=0077\n\
         NoNewPrivileges=yes\nEnvironment=CORE_SERVICE_MANAGER=systemd\n\
         Environment={UNIT_VARIABLE}=%n\n",
        if user {
            ""
        } else {
            "After=network-online.target\nWants=network-online.target\n"
        },
        exec.join(" ")
    );
    let paths = &instance.paths;
    for (kind, dir) in [
        ("CONFIG", &paths.config),
        ("DATA", &paths.data),
        ("CACHE", &paths.cache),
        ("LOG", &paths.log),
        ("TEMP", &paths.temp),
    ] {
        unit += &environment(&format!("CORE_PATH_{kind}"), dir)?;
    }
    if !user {
        // A name of letters, digits and `._-` only, or the account's uid: nothing to quote.
        let valid = |c: char| c.is_ascii_alphanumeric() || "._-".contains(c);
        if account.is_empty() || !account.chars().all(valid) {
            return Err(io(Path::new(account), "not an account name systemd reads").into());
        }
        unit += &format!("User={account}\n");
    }
    for (directive, since, namespace) in SANDBOX {
        if version.is_some_and(|v| v >= since) && !(user && namespace) {
            unit += &format!("{directive}\n");
        }
    }
    Ok(unit + &format!("\n[Install]\nWantedBy={}\n", target(manager)))
}

/// The target a unit of `manager` is enabled for.
fn target(manager: Manager) -> &'static str {
    match manager {
        Manager::SystemdUser => "default.target",
        Manager::SystemdSystem => "multi-user.target",
    }
}

/// The directory of the units: `dir`, else the manager's own (`$XDG_CONFIG_HOME/systemd/user` or
/// `~/.config/systemd/user` for a user manager).
fn unit_dir(manager: Manager, dir: Option<&Path>) -> Result<PathBuf, ServiceError> {
    if let Some(dir) = dir {
        return Ok(dir.to_path_buf());
    }
    if manager == Manager::SystemdSystem {
        return Ok(PathBuf::from(SYSTEM_DIR));
    }
    let base = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from);
    let base = base.filter(|dir| dir.is_absolute());
    let base = base.or_else(|| Some(std::env::home_dir()?.join(".config")));
    let missing = io(Path::new("~/.config/systemd/user"), "no home directory");
    Ok(base.ok_or(missing)?.join("systemd/user"))
}

/// Write the unit of `role` of `instance` into `dir`, or the manager's own unit directory.
pub fn install(
    instance: &Instance,
    role: Role,
    manager: Manager,
    binary: &Path,
    dir: Option<&Path>,
) -> Result<Installed, ServiceError> {
    let text = render(instance, role, manager, binary)?;
    let dir = unit_dir(manager, dir)?;
    let unit = unit_name(instance, role);
    fs::create_dir_all(&dir).map_err(|e| io(&dir, e))?;
    let path = dir.join(&unit);
    write_atomic(&path, text.as_bytes(), 0o644)?;
    Ok(Installed { path, unit })
}

/// The runtime unit directory of `manager`, where `systemctl enable --runtime` links a unit:
/// `$XDG_RUNTIME_DIR/systemd/user` for a user manager, when that variable is set.
fn runtime_dir(manager: Manager) -> Option<PathBuf> {
    match manager {
        Manager::SystemdSystem => Some(PathBuf::from(SYSTEM_RUNTIME_DIR)),
        Manager::SystemdUser => {
            let base = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from);
            Some(base.filter(|dir| dir.is_absolute())?.join("systemd/user"))
        }
    }
}

/// What keeps the unit of `role` of `instance` from being removed, if anything: its enable link
/// in the target's `.wants` directory of the manager's unit directories or of `dir`, or its node
/// running under it (the node's lock held without a supervisor, and its start's record naming the
/// unit).
fn in_use(
    instance: &Instance,
    role: Role,
    manager: Manager,
    dir: Option<&Path>,
) -> Result<Option<String>, ServiceError> {
    let unit = unit_name(instance, role);
    let wants = format!("{}.wants", target(manager));
    let dirs = [Some(unit_dir(manager, None)?), runtime_dir(manager)];
    let dirs = dirs.into_iter().flatten().chain(dir.map(Path::to_path_buf));
    for base in dirs {
        let link = base.join(&wants).join(&unit);
        if fs::symlink_metadata(&link).is_ok() {
            return Ok(Some(format!("enabled ({})", link.display())));
        }
    }
    let mut node = instance.clone();
    node.role = role;
    let runs = !is_locked(&node.supervisor_lock_path())? && is_locked(&node.lock_path())?;
    let record = match runs {
        true => ManagerRecord::read(&node.manager_path())?,
        false => None,
    };
    Ok(record
        .filter(|record| record.unit == unit)
        .map(|_| "running under the service manager".to_owned()))
}

/// Remove the unit of `role` of `instance` from `dir`, or the manager's own unit directory.
pub fn uninstall(
    instance: &Instance,
    role: Role,
    manager: Manager,
    dir: Option<&Path>,
) -> Result<(), ServiceError> {
    let path = unit_dir(manager, dir)?.join(unit_name(instance, role));
    Ok(fs::remove_file(&path).map_err(|e| io(&path, e))?)
}

/// What `service install` prints: the unit written and the commands that enable it; for a system
/// unit with no `dir`, the unit itself (the commands as comments), to save under `/etc`.
pub fn install_text(
    instance: &Instance,
    role: Role,
    manager: Manager,
    binary: &Path,
    dir: Option<&Path>,
) -> Result<String, ServiceError> {
    let (unit, user) = (unit_name(instance, role), manager == Manager::SystemdUser);
    let ctl = if user {
        "systemctl --user"
    } else {
        "systemctl"
    };
    let steps = format!("{ctl} daemon-reload\n{ctl} enable --now {unit}");
    if !user && dir.is_none() {
        let text = render(instance, role, manager, binary)?;
        let steps = steps.replace('\n', "\n# ");
        return Ok(format!(
            "{text}\n# Save this as {SYSTEM_DIR}/{unit}, then run:\n# {steps}\n"
        ));
    }
    let path = install(instance, role, manager, binary, dir)?.path;
    Ok(format!(
        "Wrote {}\nTo enable it from the manager's unit directory, run:\n{steps}\n",
        path.display()
    ))
}

/// What `service uninstall` prints: the unit removed and the reload that follows; for a system
/// unit with no `dir`, the steps alone, in their order, as nothing was written to `/etc`. A unit
/// that is enabled or whose node runs is kept ([`ServiceError::InUse`]), with the steps that
/// disable it first. With `dir`, a copy placed in the manager's unit directory is named, as it
/// stays.
pub fn uninstall_text(
    instance: &Instance,
    role: Role,
    manager: Manager,
    dir: Option<&Path>,
) -> Result<String, ServiceError> {
    let (unit, user) = (unit_name(instance, role), manager == Manager::SystemdUser);
    let ctl = if user {
        "systemctl --user"
    } else {
        "systemctl"
    };
    let (disable, reload) = (
        format!("{ctl} disable --now {unit}"),
        format!("{ctl} daemon-reload"),
    );
    if !user && dir.is_none() {
        return Ok(format!(
            "Run:\n{disable}\nthen remove {SYSTEM_DIR}/{unit} and run:\n{reload}\n"
        ));
    }
    if let Some(why) = in_use(instance, role, manager, dir)? {
        let options = [("--system", !user), ("--out", dir.is_some())];
        let flags = options.iter().filter(|(_, on)| *on).map(|(flag, _)| *flag);
        let flags: String = flags.map(|flag| format!(" {flag}")).collect();
        let again = format!(
            "heartwood service uninstall --role {role} --token {} --network {}{flags}{}",
            instance.token,
            instance.network,
            dir.map_or_else(String::new, |dir| format!(" {}", shell_word(dir))),
        );
        let steps = format!("{disable}\n{again}\n{reload}");
        return Err(ServiceError::InUse { unit, why, steps });
    }
    uninstall(instance, role, manager, dir)?;
    let own = unit_dir(manager, None)?;
    let placed = own.join(&unit);
    let copy = dir.is_some_and(|dir| dir != own) && placed.exists();
    let then = match copy {
        true => format!(
            "A copy at {} stays: remove it too, then run:\n",
            placed.display()
        ),
        false => "Run:\n".to_owned(),
    };
    Ok(format!("Removed {unit}\n{then}{reload}\n"))
}

/// `path` as one word of a POSIX shell: as it is when it holds nothing the shell reads, else in
/// single quotes.
fn shell_word(path: &Path) -> String {
    let text = path.to_string_lossy();
    let plain = |c: char| c.is_ascii_alphanumeric() || "-_./=:@+,".contains(c);
    match !text.is_empty() && text.chars().all(plain) {
        true => text.into_owned(),
        false => format!("'{}'", text.replace('\'', "'\\''")),
    }
}
