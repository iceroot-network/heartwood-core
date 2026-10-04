//! `service install|uninstall`: a systemd unit for one role of an instance (a user unit by
//! default, a system unit with `--system`), written only: no service manager is called, and the
//! commands to enable the unit are printed. The unit sets the service-manager variable the start
//! records ([`crate::supervisor::manager`]), so `status` names the manager and `stop` refuses.
//!
//! A unit runs `<binary> <role> run`, the node itself with no supervisor of Heartwood's: the
//! manager restarts it, stops it with SIGINT and gives it 30 s. Only the sandboxing directives
//! the host's systemd has are written, and a user unit gets none that need a mount namespace,
//! which a user manager may not be allowed to make.

use std::fs;
use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::config::{ConfigError, io, write_atomic};
use crate::seams::{Instance, Role};

/// Where the units of a system manager live.
const SYSTEM_DIR: &str = "/etc/systemd/system";

/// The sandboxing directives, the systemd version that has each, and whether it needs a mount
/// namespace.
const SANDBOX: [(&str, u32, bool); 10] = [
    ("SystemCallArchitectures=native", 209, false),
    ("RestrictRealtime=yes", 231, false),
    ("RestrictNamespaces=yes", 233, false),
    ("LockPersonality=yes", 235, false),
    ("RestrictSUIDSGID=yes", 242, false),
    ("PrivateTmp=yes", 209, true),
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

/// A service command failed: a file could not be read or written, or a link is in its place.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error(transparent)]
pub struct ServiceError(#[from] ConfigError);

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

/// The unit's text for `role` of `instance` under `manager`, running `binary`, with the
/// sandboxing of this host's systemd.
pub fn render(
    instance: &Instance,
    role: Role,
    manager: Manager,
    binary: &Path,
) -> Result<String, ServiceError> {
    render_for(instance, role, manager, binary, host_version())
}

/// [`render`] for a host whose systemd has `version`; no sandboxing for a host that is not known.
pub fn render_for(
    instance: &Instance,
    role: Role,
    manager: Manager,
    binary: &Path,
    version: Option<u32>,
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
         NoNewPrivileges=yes\nEnvironment=CORE_SERVICE_MANAGER=systemd\n",
        if user {
            ""
        } else {
            "After=network-online.target\nWants=network-online.target\n"
        },
        exec.join(" ")
    );
    for (directive, since, namespace) in SANDBOX {
        if version.is_some_and(|v| v >= since) && !(user && namespace) {
            unit += &format!("{directive}\n");
        }
    }
    let target = if user {
        "default.target"
    } else {
        "multi-user.target"
    };
    Ok(unit + &format!("\n[Install]\nWantedBy={target}\n"))
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

/// What `service uninstall` prints: the unit removed and the commands that disable it; for a
/// system unit with no `dir`, the commands alone, as nothing was written to `/etc`.
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
    let steps = format!("{ctl} disable --now {unit}\n{ctl} daemon-reload");
    if !user && dir.is_none() {
        return Ok(format!("Run:\n{steps}\nthen remove {SYSTEM_DIR}/{unit}\n"));
    }
    uninstall(instance, role, manager, dir)?;
    Ok(format!(
        "Removed {unit}\nIf it was enabled, run:\n{steps}\n"
    ))
}
