//! `app.json`: the components of a process's role, in file order, with their options.
//!
//! The file must hold the three profiles `core`, `relay` and `forger`, each with only the keys
//! `flags` (text items), `services` (an object) and `plugins` (required: objects with only
//! `package`, non-empty text, and `options`, an object), as the reference's schema requires even
//! of the profiles a process does not run. Each component of the role's profile names its package
//! and gives options, which are merged over the component's defaults (arrays replaced, see
//! [`crate::config::merge`]) and read as the reference's option schemas read them (numbers as
//! [`integer`] reads them), unknown keys allowed. A profile's `flags` and an `app.js`
//! beside the file are not read; [`AppConfig::ignored`] names them for the start's warnings. Each
//! component's options type implements [`Options`].

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};
use thiserror::Error;

use crate::config::env::Env;

/// The role of a process, and so its profile in `app.json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// A relay: the chain, the pool, the peers and the API, without forging.
    Relay,
    /// A forger.
    Forger,
    /// A relay and a forger in one process.
    Core,
}

impl Role {
    /// The profile's key in `app.json`.
    pub const fn profile(self) -> &'static str {
        match self {
            Role::Relay => "relay",
            Role::Forger => "forger",
            Role::Core => "core",
        }
    }
}

/// One component of a profile.
#[derive(Debug, Clone, PartialEq)]
pub struct PluginEntry {
    /// The package name, such as `@solar-network/api`.
    pub package: String,
    /// The options the file gives, before the defaults.
    pub options: Value,
}

/// A role's profile of `app.json`.
#[derive(Debug, Clone, PartialEq)]
pub struct AppConfig {
    /// The components, in file order.
    pub plugins: Vec<PluginEntry>,
    /// What the file holds that is not read (a profile's `flags`, an `app.js`), for the start's
    /// warnings.
    pub ignored: Vec<String>,
}

/// `app.json` could not be read as a profile.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum AppConfigError {
    /// The file could not be read.
    #[error("cannot read {}: {detail}", path.display())]
    Read {
        /// The file.
        path: PathBuf,
        /// Why.
        detail: String,
    },
    /// The file is not JSON, or not of the reference's shape.
    #[error("{} is not a valid app.json: {detail}", path.display())]
    Shape {
        /// The file.
        path: PathBuf,
        /// What is wrong.
        detail: String,
    },
    /// The file has no profile for the role.
    #[error("app.json has no {profile} profile")]
    NoProfile {
        /// The role's profile key.
        profile: &'static str,
    },
}

/// A component's options could not be read as the reference's schema reads them.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("the {package} option {key} is not valid: {reason}")]
pub struct OptionsError {
    /// The component's package.
    pub package: &'static str,
    /// The option's key, as a dotted path.
    pub key: String,
    /// Why it is refused.
    pub reason: String,
}

/// A component's options, read from its `app.json` entry.
pub trait Options: Sized {
    /// The component's package name.
    const PACKAGE: &'static str;

    /// The options of `value`: the entry's options as the node gives them (`{}` when the entry
    /// has none), merged over the component's defaults, which read the variables of `env` as the
    /// reference's defaults do, then read as the reference's schema reads them. A `value` that
    /// already holds the defaults gives the same options, since the merge keeps them.
    fn from_options(value: &Value, env: &Env) -> Result<Self, OptionsError>;
}

/// The profile of `role` in `<config_dir>/app.json` (see the module documentation).
pub fn load(config_dir: &Path, role: Role) -> Result<AppConfig, AppConfigError> {
    let path = config_dir.join("app.json");
    let script = config_dir.join("app.js").exists();
    let text = std::fs::read_to_string(&path).map_err(|error| AppConfigError::Read {
        path: path.clone(),
        detail: format!(
            "{error}{}",
            if script { " (app.js is not read)" } else { "" }
        ),
    })?;
    let mut app = parse(&text, role).map_err(|error| match error {
        AppConfigError::Shape { detail, .. } => AppConfigError::Shape { path, detail },
        other => other,
    })?;
    app.ignored.extend(script.then(|| "app.js".to_owned()));
    Ok(app)
}

/// The profile of `role` in the `app.json` text `text`.
pub fn parse(text: &str, role: Role) -> Result<AppConfig, AppConfigError> {
    let shape = |detail: String| AppConfigError::Shape {
        path: PathBuf::from("app.json"),
        detail,
    };
    let json: Value = serde_json::from_str(text).map_err(|error| shape(error.to_string()))?;
    let mut chosen = Vec::new();
    for profile in [Role::Core, Role::Relay, Role::Forger].map(Role::profile) {
        let entries = json
            .get(profile)
            .ok_or(AppConfigError::NoProfile { profile })?;
        let plugins = profile_plugins(entries).ok_or_else(|| {
            shape(format!(
                "the {profile} profile is not of the reference's shape"
            ))
        })?;
        if profile == role.profile() {
            chosen = plugins;
        }
    }
    let flags = json
        .get(role.profile())
        .is_some_and(|profile| profile.get("flags").is_some());
    Ok(AppConfig {
        plugins: chosen,
        ignored: flags
            .then(|| format!("{}.flags", role.profile()))
            .into_iter()
            .collect(),
    })
}

/// The components of a profile, when the schema takes it.
fn profile_plugins(profile: &Value) -> Option<Vec<PluginEntry>> {
    let profile = only(profile, &["flags", "services", "plugins"])?;
    let text = |value: &Value| {
        value
            .as_str()
            .filter(|text| !text.is_empty())
            .map(str::to_owned)
    };
    let flags = profile.get("flags").map(Value::as_array);
    let flags =
        flags.is_none_or(|list| list.is_some_and(|list| list.iter().all(|f| text(f).is_some())));
    let services = profile.get("services").is_none_or(Value::is_object);
    let entry = |plugin: &Value| {
        let plugin = only(plugin, &["package", "options"])?;
        let package = plugin.get("package").map_or(Some(String::new()), text)?;
        let options = plugin
            .get("options")
            .cloned()
            .unwrap_or(Value::Object(Map::new()));
        options
            .is_object()
            .then_some(PluginEntry { package, options })
    };
    let plugins = profile.get("plugins")?.as_array()?.iter().map(entry);
    plugins
        .collect::<Option<Vec<_>>>()
        .filter(|_| flags && services)
}

/// The object `value`, when it has no key outside `keys`.
pub(crate) fn only<'a>(value: &'a Value, keys: &[&str]) -> Option<&'a Map<String, Value>> {
    value
        .as_object()
        .filter(|map| map.keys().all(|key| keys.contains(&key.as_str())))
}

/// The largest integer a JavaScript number holds exactly.
const MAX_SAFE: f64 = 9_007_199_254_740_991.0;

/// `value` as `Joi.number().integer()` converts it, when it is from `min` to `max`: a JSON number,
/// or text of an optional sign, digits with an optional fraction (or a fraction alone) and an
/// optional exponent, with white space around, which is the form Rust's reader takes too (apart
/// from infinity and NaN, which are no whole numbers); a whole safe number, and not `-0`, which
/// Joi refuses because it does not print back as its text.
pub fn integer(value: &Value, min: i64, max: i64) -> Option<i64> {
    let number = match value {
        Value::Number(number) => number.as_f64()?,
        Value::String(text) => (text.trim().parse::<f64>().ok())
            .filter(|number| *number != 0.0 || !text.trim().starts_with('-'))?,
        _ => return None,
    };
    // A whole number of at most 2^53 - 1 converts exactly.
    let whole = (number.fract() == 0.0 && number.abs() <= MAX_SAFE).then_some(number as i64)?;
    (min..=max).contains(&whole).then_some(whole)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// An `app.json` text whose relay profile is `relay`, the other two profiles empty.
    fn with_relay(relay: Value) -> String {
        json!({"core": {"plugins": []}, "relay": relay, "forger": {"plugins": []}}).to_string()
    }

    #[test]
    fn the_role_profile_is_read_in_file_order() {
        let text = with_relay(json!({"plugins": [
            {"package": "@solar-network/p2p", "options": {"dns": []}},
            {"package": "@solar-network/state"},
            {}
        ], "services": {}}));
        let app = parse(&text, Role::Relay).unwrap();
        let packages: Vec<&str> = app.plugins.iter().map(|p| p.package.as_str()).collect();
        assert_eq!(packages, ["@solar-network/p2p", "@solar-network/state", ""]);
        assert_eq!(app.plugins[0].options, json!({"dns": []}));
        assert_eq!(app.plugins[1].options, json!({}));
        assert!(app.ignored.is_empty());
        assert!(parse(&text, Role::Forger).unwrap().plugins.is_empty());
        // A profile's flags are not read, and named for a warning.
        let flagged = with_relay(json!({"plugins": [], "flags": ["x"]}));
        assert_eq!(
            parse(&flagged, Role::Relay).unwrap().ignored,
            ["relay.flags"]
        );
        assert!(parse(&flagged, Role::Core).unwrap().ignored.is_empty());
    }

    #[test]
    fn every_profile_must_follow_the_schema() {
        let missing = json!({"core": {"plugins": []}, "relay": {"plugins": []}}).to_string();
        assert_eq!(
            parse(&missing, Role::Relay),
            Err(AppConfigError::NoProfile { profile: "forger" })
        );
        // A bad core profile refuses a relay too, as the reference checks all three.
        let bad_core =
            json!({"core": {"plugins": {}}, "relay": {"plugins": []}, "forger": {"plugins": []}});
        assert!(parse(&bad_core.to_string(), Role::Relay).is_err());
        for relay in [
            json!(null),
            json!({}),
            json!({"plugins": [], "other": 1}),
            json!({"plugins": [], "flags": "x"}),
            json!({"plugins": [], "flags": [""]}),
            json!({"plugins": [], "services": []}),
            json!({"plugins": [null]}),
            json!({"plugins": [{"package": "a", "x": 1}]}),
            json!({"plugins": [{"package": ""}]}),
            json!({"plugins": [{"package": 1}]}),
            json!({"plugins": [{"package": "a", "options": []}]}),
            json!({"plugins": [{"package": "a", "options": null}]}),
        ] {
            let error = parse(&with_relay(relay.clone()), Role::Relay).unwrap_err();
            assert!(
                matches!(error, AppConfigError::Shape { .. }),
                "{relay}: {error}"
            );
        }
        assert!(parse("[]", Role::Relay).is_err());
        assert!(parse("{", Role::Relay).is_err());
    }

    #[test]
    fn load_reads_app_json_and_names_an_app_js_beside_it() {
        let dir = std::env::temp_dir().join(format!("heartwood-app-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("app.js"), "module.exports = {}").unwrap();
        assert!(matches!(
            load(&dir, Role::Relay),
            Err(AppConfigError::Read { .. })
        ));
        std::fs::write(
            dir.join("app.json"),
            with_relay(json!({"plugins": [], "flags": []})),
        )
        .unwrap();
        let app = load(&dir, Role::Relay).unwrap();
        assert_eq!(app.ignored, ["relay.flags", "app.js"]);
        std::fs::write(dir.join("app.json"), with_relay(json!({}))).unwrap();
        match load(&dir, Role::Relay) {
            Err(AppConfigError::Shape { path, .. }) => assert_eq!(path, dir.join("app.json")),
            other => panic!("{other:?}"),
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn integers_are_read_as_joi_converts_them() {
        let read = |value: Value| integer(&value, 0, i64::MAX);
        for (text, expected) in [
            ("20", 20),
            (" +20 ", 20),
            ("20.0", 20),
            ("20.", 20),
            ("020", 20),
            ("2e1", 20),
            ("2E+1", 20),
            (".2e2", 20),
            ("0", 0),
            ("9007199254740991", 9_007_199_254_740_991),
        ] {
            assert_eq!(read(json!(text)), Some(expected), "{text}");
        }
        for text in [
            "-0",
            "-0.0",
            "1.5",
            "0x10",
            "Infinity",
            "NaN",
            "",
            ".",
            "1e",
            "1_0",
            "9007199254740992",
        ] {
            assert_eq!(read(json!(text)), None, "{text}");
        }
        assert_eq!(read(json!(20.0)), Some(20));
        assert_eq!(read(json!(1e300)), None);
        assert_eq!(read(json!(true)), None);
        assert_eq!(integer(&json!(0), 1, 10), None);
        assert_eq!(integer(&json!("11"), 1, 10), None);
        assert_eq!(integer(&json!(-3), -5, 10), Some(-3));
    }
}
