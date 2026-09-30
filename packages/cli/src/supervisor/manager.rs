//! The service-manager record, `<role>.manager.json` beside the role's lock: which service
//! manager started the instance, and the unit it runs as. A start writes it when the node's
//! environment names a manager ([`MANAGER_VARIABLE`]), and removes a stale one otherwise; `top`
//! and `status` read it, and `stop` refuses an instance a manager runs, naming the command to use.
//!
//! A JSON object: `manager` (`systemd`) and `unit` (the unit's name, such as
//! `node1-devnet-relay.service`). It is written through a temporary file and a rename, in the run
//! directory held without following a link ([`RunDir`]), and read the same way, so a link in its
//! place is refused on a read and replaced, never written through, on a write, as for the
//! supervisor's state file.

use std::io::{ErrorKind, Write};
use std::path::Path;

use nix::fcntl::OFlag;
use serde_json::{Value, json};

use super::{RunDir, at};
use crate::errors::ProcessError;
use crate::instance::pick;
use crate::seams::Instance;

/// The variable that names the service manager an instance runs under: its unit sets it.
pub const MANAGER_VARIABLE: &str = "CORE_SERVICE_MANAGER";

/// The variable that names the unit an instance runs as; the instance's own unit name
/// ([`unit_name`]) when a manager is named without it.
pub const UNIT_VARIABLE: &str = "CORE_SERVICE_UNIT";

/// A service manager an instance can run under.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ServiceManager {
    /// systemd, as a user or a system unit.
    Systemd,
}

impl ServiceManager {
    /// Every manager.
    pub const ALL: [ServiceManager; 1] = [ServiceManager::Systemd];

    /// The manager's name, in the record and in [`MANAGER_VARIABLE`].
    pub const fn name(self) -> &'static str {
        match self {
            ServiceManager::Systemd => "systemd",
        }
    }

    fn named(text: &str) -> Option<ServiceManager> {
        ServiceManager::ALL.into_iter().find(|m| m.name() == text)
    }
}

/// The name of the unit that runs `instance`: `<token>-<network>-<role>.service`.
pub fn unit_name(instance: &Instance) -> String {
    format!("{}.service", instance.name())
}

/// What a start under a service manager records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagerRecord {
    /// The manager.
    pub manager: ServiceManager,
    /// The unit the instance runs as.
    pub unit: String,
}

impl ManagerRecord {
    /// The record of `instance` in the environment `env` gives: `None` unless
    /// [`MANAGER_VARIABLE`] names a known manager; the unit from [`UNIT_VARIABLE`], else
    /// [`unit_name`].
    pub fn from_env(
        instance: &Instance,
        env: &dyn Fn(&str) -> Option<String>,
    ) -> Option<ManagerRecord> {
        let manager = ServiceManager::named(env(MANAGER_VARIABLE)?.trim())?;
        let unit = env(UNIT_VARIABLE).filter(|unit| !unit.is_empty());
        let unit = unit.unwrap_or_else(|| unit_name(instance));
        Some(ManagerRecord { manager, unit })
    }

    /// Read the record at `path`; `None` when there is none.
    pub fn read(path: &Path) -> Result<Option<ManagerRecord>, ProcessError> {
        let Some(dir) = RunDir::beside_if_there(path)? else {
            return Ok(None);
        };
        let text = match dir.read(path) {
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
            read => at(path, read)?,
        };
        let invalid = |reason: String| ProcessError::State {
            path: path.to_path_buf(),
            reason,
        };
        let value: Value = serde_json::from_str(&text).map_err(|e| invalid(e.to_string()))?;
        let manager: String = pick!(value, "manager").map_err(invalid)?;
        let manager = ServiceManager::named(&manager);
        Ok(Some(ManagerRecord {
            manager: manager.ok_or_else(|| invalid("an unknown service manager".into()))?,
            unit: pick!(value, "unit").map_err(invalid)?,
        }))
    }

    /// Write the record at `path`, atomically, in its directory as [`RunDir::beside`] holds it.
    pub fn write(&self, path: &Path) -> Result<(), ProcessError> {
        self.write_in(&RunDir::beside(path)?, path)
    }

    /// [`ManagerRecord::write`] in the run directory `dir`, which was checked.
    pub fn write_in(&self, dir: &RunDir, path: &Path) -> Result<(), ProcessError> {
        let value = json!({ "manager": self.manager.name(), "unit": self.unit });
        let temp = path.with_extension("json.tmp");
        let flags = OFlag::O_WRONLY | OFlag::O_CREAT | OFlag::O_TRUNC;
        let file = at(&temp, dir.file(&temp, flags, 0o600))?;
        at(&temp, (&file).write_all(format!("{value:#}\n").as_bytes()))?;
        at(path, dir.rename(&temp, path))
    }

    /// Remove the record at `path`, when there is one: a start with no manager named.
    pub fn remove(path: &Path) -> Result<(), ProcessError> {
        let Some(dir) = RunDir::beside_if_there(path)? else {
            return Ok(());
        };
        match dir.remove(path) {
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
            removed => at(path, removed),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::symlink;

    use heartwood_kernel::paths::Paths;

    use super::*;
    use crate::seams::Role;

    fn instance(dir: &Path) -> Instance {
        let paths = Paths {
            config: dir.join("config"),
            data: dir.join("data"),
            cache: dir.join("cache"),
            log: dir.join("log"),
            temp: dir.join("temp"),
        };
        Instance {
            token: "node1".into(),
            network: "devnet".into(),
            role: Role::Core,
            paths,
        }
    }

    #[test]
    fn a_record_reads_back_as_written_and_a_link_in_its_place_is_refused() {
        let dir = std::env::temp_dir().join(format!("hw-cli-manager-{}", std::process::id()));
        let instance = instance(&dir);
        let path = instance.manager_path();
        fs::create_dir_all(instance.run_dir()).unwrap();
        assert_eq!(ManagerRecord::read(&path).unwrap(), None);
        let env = |name: &str| (name == MANAGER_VARIABLE).then(|| "systemd".to_owned());
        let record = ManagerRecord::from_env(&instance, &env).unwrap();
        assert_eq!(record.unit, "node1-devnet-core.service");
        record.write(&path).unwrap();
        assert_eq!(ManagerRecord::read(&path).unwrap(), Some(record.clone()));
        let unit = |name: &str| match name {
            MANAGER_VARIABLE => Some("systemd".to_owned()),
            UNIT_VARIABLE => Some("heartwood@node1.service".to_owned()),
            _ => None,
        };
        let named = ManagerRecord::from_env(&instance, &unit).unwrap();
        assert_eq!(named.unit, "heartwood@node1.service");
        let other = |name: &str| (name == MANAGER_VARIABLE).then(|| "launchd".to_owned());
        assert_eq!(ManagerRecord::from_env(&instance, &other), None);
        assert_eq!(ManagerRecord::from_env(&instance, &|_| None), None);
        ManagerRecord::remove(&path).unwrap();
        ManagerRecord::remove(&path).unwrap();
        assert_eq!(ManagerRecord::read(&path).unwrap(), None);
        let target = dir.join("elsewhere.json");
        fs::write(&target, "{\"manager\": \"systemd\", \"unit\": \"x\"}").unwrap();
        symlink(&target, &path).unwrap();
        assert!(ManagerRecord::read(&path).is_err());
        // A write replaces the link by a rename, never writing through it.
        record.write(&path).unwrap();
        assert_eq!(ManagerRecord::read(&path).unwrap(), Some(record));
        assert!(fs::read_to_string(&target).unwrap().contains("\"x\""));
        fs::remove_dir_all(dir).unwrap();
    }
}
