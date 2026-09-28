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

use heartwood_crypto::utils::js;
use serde_json::{Map, Value, json};
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
        detail: error.to_string(),
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
            .ok_or(AppConfigError::NoProfile { profile });
        let plugins = profile_plugins(entries?);
        let plugins = plugins.ok_or_else(|| shape(format!("its {profile} profile")))?;
        if profile == role.profile() {
            chosen = plugins;
        }
    }
    let flags = json.get(role.profile()).map(|profile| profile.get("flags"));
    let ignored = flags.flatten().map(|_| format!("{}.flags", role.profile()));
    Ok(AppConfig {
        plugins: chosen,
        ignored: Vec::from_iter(ignored),
    })
}

/// The components of a profile, when the schema takes it.
fn profile_plugins(profile: &Value) -> Option<Vec<PluginEntry>> {
    let profile = only(profile, &["flags", "services", "plugins"])?;
    let text = |value: &Value| Some(value.as_str().filter(|t| !t.is_empty())?.to_owned());
    let texts = |list: &Vec<Value>| list.iter().all(|flag| text(flag).is_some());
    let flags = profile
        .get("flags")
        .is_none_or(|f| f.as_array().is_some_and(texts));
    let services = profile.get("services").is_none_or(Value::is_object);
    let entry = |plugin: &Value| {
        let plugin = only(plugin, &["package", "options"])?;
        let package = plugin.get("package").map_or(Some(String::new()), text)?;
        let options = plugin.get("options").cloned().unwrap_or_else(|| json!({}));
        Some(PluginEntry { package, options }).filter(|entry| entry.options.is_object())
    };
    let plugins = profile.get("plugins")?.as_array()?;
    let plugins: Option<Vec<_>> = plugins.iter().map(entry).collect();
    plugins.filter(|_| flags && services)
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
/// or text that [`joi_number`] reads; a whole number of at most 2^53 − 1.
pub fn integer(value: &Value, min: i64, max: i64) -> Option<i64> {
    let number = match value {
        Value::Number(number) => number.as_f64()?,
        Value::String(text) => joi_number(text)?,
        _ => return None,
    };
    // A whole number of at most 2^53 - 1 converts exactly.
    let whole = number.fract() == 0.0 && number.abs() <= MAX_SAFE;
    Some(number as i64).filter(|n| whole && (min..=max).contains(n))
}

/// The number Joi's `number()` reads from `text`: an optional sign, digits with an optional
/// fraction (or a fraction alone) and an optional exponent, with white space around (the forms
/// Rust's reader takes, infinity and NaN aside), `-0` read as 0, at most 2^53 − 1 in size; refused
/// as unsafe unless it prints back as the text (`normalised`), or, in an exponent form, unless
/// the number over ten to the exponent prints back as the mantissa.
pub fn joi_number(text: &str) -> Option<f64> {
    let text = text.trim();
    let form = text.bytes().all(|b| b"+-.eE0123456789".contains(&b));
    let number = text.parse::<f64>().ok().filter(|_| form)? + 0.0;
    let same = match text.split_once(['e', 'E']) {
        Some((mantissa, exponent)) => {
            let quotient = number / 10f64.powf(exponent.parse().ok()?);
            normalised(&js::number_to_string(quotient), false) == normalised(mantissa, false)
        }
        None => {
            let printed = js::number_to_string(number);
            printed.contains('e') || printed == normalised(text, true)
        }
    };
    (same && number.abs() <= MAX_SAFE).then_some(number)
}

/// A number's text as Joi normalises it: no `+`; a fraction without trailing zeros, and no point
/// without one; leading zeros removed before a digit from 1 to 9, and in a `decimal` text all
/// but one before a point or at the end (a fraction alone gains a `0`, and `-0` is `0`).
fn normalised(text: &str, decimal: bool) -> String {
    let text = text.strip_prefix('+').unwrap_or(text);
    let (sign, rest) = text.split_at(usize::from(text.starts_with('-')));
    let (whole, fraction) = rest.split_once('.').unwrap_or((rest, ""));
    let fraction = fraction.trim_end_matches('0');
    let bare = whole.trim_start_matches('0');
    let whole = match bare.chars().next() {
        Some('1'..='9') => bare,
        _ if decimal && !(whole.is_empty() && fraction.is_empty()) => "0",
        _ => whole,
    };
    let point = if fraction.is_empty() { "" } else { "." };
    let sign = if decimal && whole == "0" && point.is_empty() {
        ""
    } else {
        sign
    };
    format!("{sign}{whole}{point}{fraction}")
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
    fn numbers_are_read_as_joi_reads_them() {
        // Joi 17's answers, recorded from the reference's own copy.
        for (text, expected) in [
            ("0", Some(0.0)),
            ("-0", Some(0.0)),
            ("+0", Some(0.0)),
            ("00", Some(0.0)),
            ("-00", Some(0.0)),
            ("000", Some(0.0)),
            ("007", Some(7.0)),
            ("-007", Some(-7.0)),
            ("0.0", Some(0.0)),
            ("-0.0", Some(0.0)),
            ("00.5", Some(0.5)),
            ("0.50", Some(0.5)),
            ("1.50", Some(1.5)),
            ("20.", Some(20.0)),
            ("20.0", Some(20.0)),
            ("20.00", Some(20.0)),
            ("020.0", Some(20.0)),
            (".5", Some(0.5)),
            (".50", Some(0.5)),
            ("-.5", Some(-0.5)),
            ("-.0", None),
            ("+.5", Some(0.5)),
            ("1e3", Some(1000.0)),
            ("1E3", Some(1000.0)),
            ("1e+3", Some(1000.0)),
            ("1e03", Some(1000.0)),
            ("1.0e3", Some(1000.0)),
            ("1.500e3", Some(1500.0)),
            ("10e1", Some(100.0)),
            ("10.e1", Some(100.0)),
            (".5e1", None),
            ("0.5e1", Some(5.0)),
            ("2e-1", Some(0.2)),
            ("3e-1", None),
            ("-0e0", None),
            ("0e0", Some(0.0)),
            ("1e400", None),
            ("9007199254740991", Some(9007199254740991.0)),
            ("9007199254740992", None),
            ("9007199254740993", None),
            ("1e21", None),
            ("100000000000000000000000", None),
            (" 12 ", Some(12.0)),
            ("\t7\n", Some(7.0)),
            ("1e-7", Some(1e-07)),
            ("123456789012345678901234", None),
            ("0.1", Some(0.1)),
            ("0.30000000000000004", Some(0.30000000000000004)),
            ("5e-324", None),
            ("-5", Some(-5.0)),
            ("+5", Some(5.0)),
            ("12.30e1", Some(123.0)),
            ("007e1", Some(70.0)),
            ("00.5e1", None),
            ("1_0", None),
            ("0x10", None),
            ("Infinity", None),
            ("NaN", None),
            ("", None),
            (".", None),
            (" ", None),
            ("e5", None),
            ("1e", None),
            ("--1", None),
            ("+-1", None),
        ] {
            assert_eq!(joi_number(text), expected, "{text:?}");
        }
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
            ("-0", 0),
            ("-0.0", 0),
            ("0", 0),
            ("9007199254740991", 9_007_199_254_740_991),
        ] {
            assert_eq!(read(json!(text)), Some(expected), "{text}");
        }
        for text in [
            ".2e2",
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
