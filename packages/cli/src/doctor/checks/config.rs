//! Configuration and key files: the node's loaders, the log level, the run directory, the key file,
//! the configuration directory and stray key files. The loader gate lives here: no node loader runs
//! while others can write the configuration directory.

use crate::doctor::registry::{CheckInput, not_built};
use crate::doctor::report::Finding;
use crate::doctor::seams::FileFacts;
use crate::doctor::view::{Reason, Source, Unknown};

/// `config.load`.
pub fn load(input: &CheckInput<'_>) -> Vec<Finding> {
    not_built(input)
}

/// `config.log-level`.
pub fn log_level(input: &CheckInput<'_>) -> Vec<Finding> {
    not_built(input)
}

/// `config.run-dir`.
pub fn run_dir(input: &CheckInput<'_>) -> Vec<Finding> {
    not_built(input)
}

/// `keys.file`.
pub fn keys_file(input: &CheckInput<'_>) -> Vec<Finding> {
    not_built(input)
}

/// `keys.dir`.
pub fn keys_dir(input: &CheckInput<'_>) -> Vec<Finding> {
    not_built(input)
}

/// `keys.stray`.
pub fn keys_stray(input: &CheckInput<'_>) -> Vec<Finding> {
    not_built(input)
}

/// Whether the node's loaders may run, from the metadata of the configuration directory `dir`
/// and its parent: not while a user other than the one they belong to can write either, since such
/// a user could have planted a link to the key file there. The view calls it before every loader.
pub fn loader_gate(dir: Option<&FileFacts>, parent: Option<&FileFacts>) -> Result<(), Unknown> {
    let _ = (dir, parent);
    Err(Unknown::new(Source::Config, Reason::NotBuilt))
}
