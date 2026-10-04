//! `env get|list|set|paths`: an instance's `<config>/.env`, read with the kernel's parser (the one
//! the node reads it with) and written as the reference's `envfile` writes it
//! (`stringifySync(...).trim()`), through the writer that refuses a link in its place.
//!
//! - `get` and `list` show what the node reads. The kernel's parser drops the quotes of a quoted
//!   value and reads a value of digits as a number, where the reference's `envfile` keeps the text
//!   as it is, and reads a file that is JSON as JSON: a documented difference of the commands.
//! - `set` reads the file so, then writes every variable as `KEY=value`, in the order of a
//!   JavaScript object's keys (the array indexes first, in numeric order, then the others in the
//!   order they came), and nothing else: comments and blank lines are dropped, and the text has
//!   no final line break. The file is replaced through a temporary file and a rename in its
//!   directory, held without following a link, with the mode the file had. A link in the file's
//!   place is refused on a read and replaced, never written through, on a write.
//! - The reference's argument parser reads a flag's value that looks like a number as a number,
//!   and its text is then the number's ([`flag_text`]): `--value 0x10` sets `16`.

use std::fs::{self, Permissions};
use std::io::{ErrorKind, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use heartwood_crypto::utils::js;
use heartwood_kernel::config::dot_env::{self, EnvValue};
use nix::fcntl::OFlag;
use thiserror::Error;

use crate::errors::ProcessError;
use crate::seams::Instance;
use crate::supervisor::RunDir;

/// The environment file's name in the configuration directory.
const FILE: &str = ".env";

/// The largest integer JavaScript holds exactly, 2^53 - 1.
const MAX_SAFE: f64 = 9_007_199_254_740_991.0;

/// The environment file could not be read or written.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum EnvError {
    /// A required command argument was not supplied.
    #[error("\"{0}\" is required")]
    Required(&'static str),
    /// There is no environment file, as the reference says it.
    #[error("No environment file found at {}", path.display())]
    NoFile {
        /// Where it was looked for.
        path: PathBuf,
    },
    /// The file could not be read or written, or a link is in its place.
    #[error("{} cannot be read or written: {message}", path.display())]
    Io {
        /// The file.
        path: PathBuf,
        /// Why.
        message: String,
    },
    /// A variable that is not in the file, or has an empty value, as the reference says it.
    #[error("The \"{0}\" doesn't exist")]
    Missing(String),
    /// A key or a value that is empty, which the reference's flags refuse.
    #[error("\"{0}\" is not allowed to be empty")]
    Empty(&'static str),
    /// A flag that must be text and that the reference's argument parser reads as a number.
    #[error("\"{0}\" must be a string")]
    NotText(&'static str),
}

impl From<ProcessError> for EnvError {
    fn from(error: ProcessError) -> EnvError {
        match error {
            ProcessError::Io { path, message } => EnvError::Io { path, message },
            other => io(Path::new(FILE), other),
        }
    }
}

fn io(path: &Path, error: impl ToString) -> EnvError {
    let message = error.to_string();
    let path = path.to_path_buf();
    EnvError::Io { path, message }
}

/// The variables of the `.env` text `text`, as the node reads them: in the order of a JavaScript
/// object's keys, each value as the node's environment stores it.
pub fn parse(text: &str) -> Vec<(String, String)> {
    let stored = |value: EnvValue| match value {
        EnvValue::Number(number) => js::number_to_string(number),
        EnvValue::Bool(flag) => flag.to_string(),
        EnvValue::Text(text) => text,
    };
    let pairs = dot_env::parse(text).into_iter();
    pairs.map(|(key, value)| (key, stored(value))).collect()
}

/// The text `envfile`'s `stringifySync(vars).trim()` gives for the object that assigning `pairs`
/// in turn makes: a key again takes its place with the later value, and `__proto__` is no key.
pub fn stringify(pairs: &[(String, String)]) -> String {
    let mut vars = Vec::new();
    for (key, value) in pairs {
        assign(&mut vars, key, value);
    }
    let rank = |key: &str| match js::is_array_index(key) {
        true => key.parse::<u64>().unwrap_or(u64::MAX),
        false => u64::MAX,
    };
    // A stable sort: the keys that are no indexes keep the order they came in.
    vars.sort_by_key(|(key, _)| rank(key));
    let lines = vars.iter().filter(|(key, _)| !key.is_empty());
    let text: String = lines
        .map(|(key, value)| format!("{key}={value}\n"))
        .collect();
    // JavaScript's `trim`: white space, but not U+0085, and U+FEFF.
    let white = |c: char| (c.is_whitespace() && c != '\u{85}') || c == '\u{feff}';
    text.trim_matches(white).to_owned()
}

/// `vars[key] = value` for a JavaScript object: a key that is there keeps its place. Setting
/// `__proto__` to a string sets nothing.
fn assign(vars: &mut Vec<(String, String)>, key: &str, value: &str) {
    match vars.iter_mut().find(|(known, _)| known == key) {
        Some(var) => var.1 = value.to_owned(),
        None if key != "__proto__" => vars.push((key.to_owned(), value.to_owned())),
        None => {}
    }
}

/// The text of a flag's value as the reference's argument parser hands it on: a value that looks
/// like a number (`-5`, `.5`, `1e3`, `0x10`, but not `007`) whose integer part is at most 2^53 - 1
/// is that number, written as JavaScript writes it; any other value is as it is.
pub fn flag_text(text: &str) -> String {
    flag_number(text).map_or_else(|| text.to_owned(), js::number_to_string)
}

/// The number the reference's argument parser reads a flag's value `text` as, if it reads one.
fn flag_number(text: &str) -> Option<f64> {
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    let hex = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X"));
    if let Some(hex) = hex.filter(|h| !h.is_empty() && h.bytes().all(|b| b.is_ascii_hexdigit())) {
        let read = |c: char| f64::from(c.to_digit(16).unwrap_or(0));
        return Some(hex.chars().fold(0.0, |sum, c| sum * 16.0 + read(c)));
    }
    let body = text.strip_prefix('-').unwrap_or(text);
    let (mantissa, exponent) = body.split_once('e').unzip();
    let (mantissa, exponent) = (mantissa.unwrap_or(body), exponent);
    let exponent = exponent.is_none_or(|e| digits(e.strip_prefix(['-', '+']).unwrap_or(e)));
    let mantissa = match mantissa.split_once('.') {
        Some((whole, fraction)) => match whole.is_empty() {
            true => digits(fraction),
            false => digits(whole) && (fraction.is_empty() || digits(fraction)),
        },
        None => digits(mantissa),
    };
    // A leading zero and then no point reads as octal, which the parser leaves as text.
    let octal = text.starts_with('0') && text.chars().nth(1).is_some_and(|c| c != '.');
    let number = text.parse::<f64>().ok();
    number.filter(|n| mantissa && exponent && !octal && n.floor().abs() <= MAX_SAFE)
}

/// A file read: its directory held, its path, its variables and its mode.
struct Loaded {
    dir: RunDir,
    path: PathBuf,
    vars: Vec<(String, String)>,
    mode: u32,
}

/// Read `instance`'s environment file, without following a link in the file's place.
fn load(instance: &Instance) -> Result<Loaded, EnvError> {
    let wanted = instance.paths.config.join(FILE);
    let none = || EnvError::NoFile {
        path: wanted.clone(),
    };
    // The configuration directory may be a link; the file in it may not.
    let config = match fs::canonicalize(&instance.paths.config) {
        Err(e) if e.kind() == ErrorKind::NotFound => return Err(none()),
        found => found.map_err(|e| io(&wanted, e))?,
    };
    let path = config.join(FILE);
    let dir = RunDir::beside_if_there(&path)?.ok_or_else(none)?;
    let mut file = match dir.file(&path, OFlag::O_RDONLY, 0) {
        Err(e) if e.kind() == ErrorKind::NotFound => return Err(none()),
        opened => opened.map_err(|e| io(&wanted, e))?,
    };
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).map_err(|e| io(&wanted, e))?;
    let mode = file
        .metadata()
        .map_err(|e| io(&wanted, e))?
        .permissions()
        .mode();
    let vars = parse(&String::from_utf8_lossy(&bytes));
    Ok(Loaded {
        dir,
        path,
        vars,
        mode: mode & 0o777,
    })
}

/// The value of `key` in `instance`'s environment file; `None` when it has none.
pub fn get(instance: &Instance, key: &str) -> Result<Option<String>, EnvError> {
    let vars = load(instance)?.vars;
    Ok(vars
        .into_iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value))
}

/// Every variable of `instance`'s environment file, in the file's order.
pub fn list(instance: &Instance) -> Result<Vec<(String, String)>, EnvError> {
    Ok(load(instance)?.vars)
}

/// Set `key` to `value` in `instance`'s environment file, rewriting it as `envfile` does.
pub fn set(instance: &Instance, key: &str, value: &str) -> Result<(), EnvError> {
    for (name, text) in [("key", key), ("value", value)] {
        if text.is_empty() {
            return Err(EnvError::Empty(name));
        }
    }
    let mut file = load(instance)?;
    assign(&mut file.vars, key, value);
    let text = stringify(&file.vars);
    let temp = file.path.with_file_name(".env.tmp");
    let flags = OFlag::O_WRONLY | OFlag::O_CREAT | OFlag::O_TRUNC;
    let out = file
        .dir
        .file(&temp, flags, 0o600)
        .map_err(|e| io(&temp, e))?;
    let written = (&out).write_all(text.as_bytes());
    let written = written.and_then(|()| out.set_permissions(Permissions::from_mode(file.mode)));
    let written = written.and_then(|()| file.dir.rename(&temp, &file.path));
    if let Err(e) = written {
        let _ = file.dir.remove(&temp);
        return Err(io(&file.path, e));
    }
    Ok(())
}

/// A command's work on an instance: the text it prints.
pub type Action = Box<dyn FnOnce(&Instance) -> Result<String, EnvError>>;

/// What `env get --key key` prints: the variable's value and a line break, and a variable that
/// is empty is none. The reference validates `--key` as text before it reads the file: a key
/// that is empty, or that its argument parser reads as a number ([`flag_text`]), is refused.
pub fn get_text(instance: &Instance, key: &str) -> Result<String, EnvError> {
    match (key.is_empty(), flag_number(key)) {
        (true, _) => return Err(EnvError::Empty("key")),
        (_, Some(_)) => return Err(EnvError::NotText("key")),
        _ => {}
    }
    match get(instance, key)?.filter(|value| !value.is_empty()) {
        Some(value) => Ok(value + "\n"),
        None => Err(EnvError::Missing(key.to_owned())),
    }
}

/// What `env list` prints: every variable in a table.
pub fn list_text(instance: &Instance) -> Result<String, EnvError> {
    Ok(table(["Key", "Value"], &list(instance)?))
}

/// What `env set` prints (nothing) after setting `key` to `value`, both as the argument parser
/// reads them ([`flag_text`]).
pub fn set_text(instance: &Instance, key: &str, value: &str) -> Result<String, EnvError> {
    set(instance, &flag_text(key), &flag_text(value)).map(|()| String::new())
}

/// What `env paths` prints: the instance's directories in a table, in the reference's order
/// (env-paths' keys: data, config, cache, log and temp).
pub fn paths_text(instance: &Instance) -> Result<String, EnvError> {
    let [config, data, rest @ ..] = paths(instance);
    let rows: Vec<_> = ([data, config].into_iter().chain(rest))
        .map(|(kind, path)| (kind.to_owned(), path.display().to_string()))
        .collect();
    Ok(table(["Type", "Path"], &rows))
}

/// The rows of a two-column table under `header`, as `status`'s table is laid out: the columns as
/// wide as their widest cell, two spaces apart, no trailing spaces.
fn table(header: [&str; 2], rows: &[(String, String)]) -> String {
    let head = (header[0].to_owned(), header[1].to_owned());
    let all = std::iter::once(&head).chain(rows);
    let width = all
        .clone()
        .map(|(a, _)| a.chars().count())
        .max()
        .unwrap_or(0);
    all.map(|(a, b)| format!("{a:width$}  {b}").trim_end().to_owned() + "\n")
        .collect()
}

/// `instance`'s five directories by name: config, data, cache, log and temp.
pub fn paths(instance: &Instance) -> [(&'static str, PathBuf); 5] {
    let paths = &instance.paths;
    [
        ("config", paths.config.clone()),
        ("data", paths.data.clone()),
        ("cache", paths.cache.clone()),
        ("log", paths.log.clone()),
        ("temp", paths.temp.clone()),
    ]
}
