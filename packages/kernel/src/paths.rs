//! The directories of a node: config, data, cache, log and temp.
//!
//! As the reference's kernel derives them: the platform's application directories for the name
//! `<token>-core`, each followed by `/` and the network name, or replaced by its `CORE_PATH_DATA`,
//! `CORE_PATH_CONFIG`, `CORE_PATH_CACHE`, `CORE_PATH_LOG` or `CORE_PATH_TEMP` variable when that is
//! set and not empty. A leading `~` then stands for the home directory and a leading `~+` for the
//! working directory, and the path is resolved against the working directory and normalised, as
//! Node's `path.resolve` does.
//!
//! On Linux the application directories follow the XDG base directories: `$XDG_DATA_HOME` or
//! `~/.local/share`, `$XDG_CONFIG_HOME` or `~/.config`, `$XDG_CACHE_HOME` or `~/.cache`,
//! `$XDG_STATE_HOME` or `~/.local/state` for logs, and `<tmp>/<user>` for temp, where `<tmp>` is
//! `$TMPDIR`, `$TMP`, `$TEMP` or `/tmp` and `<user>` the last component of the home directory. On
//! macOS they are under `~/Library` (`Application Support`, `Preferences`, `Caches`, `Logs`) and
//! temp is `<tmp>` itself.

use std::path::{Path, PathBuf};

/// The directories of a node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    /// Configuration files.
    pub config: PathBuf,
    /// Databases and other data.
    pub data: PathBuf,
    /// Caches.
    pub cache: PathBuf,
    /// Log files.
    pub log: PathBuf,
    /// Temporary files.
    pub temp: PathBuf,
}

/// The directories for `token` and `network`, reading environment variables through `env`, with
/// `home` as the user's home directory and `cwd` as the working directory (absolute).
pub fn resolve_paths(
    token: &str,
    network: &str,
    env: &dyn Fn(&str) -> Option<String>,
    home: &Path,
    cwd: &Path,
) -> Paths {
    let var = |name: &str| env(name).filter(|value| !value.is_empty());
    let home = home.to_string_lossy();
    let cwd = cwd.to_string_lossy();
    let name = format!("{token}-core");
    let mut tmp = var("TMPDIR")
        .or_else(|| var("TMP"))
        .or_else(|| var("TEMP"))
        .unwrap_or_else(|| "/tmp".to_owned());
    if tmp.len() > 1 && tmp.ends_with('/') {
        tmp.pop();
    }
    let under = |variable: &str, default: &[&str]| {
        let base = var(variable).unwrap_or_else(|| join(&[&home, &default.join("/")]));
        join(&[&base, &name])
    };
    let base = if cfg!(target_os = "macos") {
        [
            join(&[&home, "Library/Application Support", &name]),
            join(&[&home, "Library/Preferences", &name]),
            join(&[&home, "Library/Caches", &name]),
            join(&[&home, "Library/Logs", &name]),
            join(&[&tmp, &name]),
        ]
    } else {
        [
            under("XDG_DATA_HOME", &[".local", "share"]),
            under("XDG_CONFIG_HOME", &[".config"]),
            under("XDG_CACHE_HOME", &[".cache"]),
            under("XDG_STATE_HOME", &[".local", "state"]),
            join(&[&tmp, basename(&home), &name]),
        ]
    };
    let [data, config, cache, log, temp] = base;
    let finish = |kind: &str, default: String| {
        let path =
            var(&format!("CORE_PATH_{kind}")).unwrap_or_else(|| format!("{default}/{network}"));
        let path = if let Some(rest) = path.strip_prefix("~+") {
            join(&[&cwd, rest])
        } else if let Some(rest) = path.strip_prefix('~') {
            join(&[&home, rest])
        } else {
            path
        };
        PathBuf::from(resolve(&cwd, &path))
    };
    Paths {
        data: finish("DATA", data),
        config: finish("CONFIG", config),
        cache: finish("CACHE", cache),
        log: finish("LOG", log),
        temp: finish("TEMP", temp),
    }
}

/// The non-empty `parts` joined with `/`, as `path.join` joins them; the result is normalised
/// later by [`resolve`], which gives the same path.
fn join(parts: &[&str]) -> String {
    parts
        .iter()
        .filter(|part| !part.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join("/")
}

/// The last component of `path`, as `path.basename` gives it (trailing slashes ignored).
fn basename(path: &str) -> &str {
    let trimmed = path.trim_end_matches('/');
    trimmed.rsplit('/').next().unwrap_or(trimmed)
}

/// `path.resolve(cwd, path)`: absolute, normalised, without a trailing slash.
fn resolve(cwd: &str, path: &str) -> String {
    let full = if path.starts_with('/') {
        path.to_owned()
    } else {
        format!("{cwd}/{path}")
    };
    let mut components: Vec<&str> = Vec::new();
    for component in full.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                components.pop();
            }
            other => components.push(other),
        }
    }
    format!("/{}", components.join("/"))
}
