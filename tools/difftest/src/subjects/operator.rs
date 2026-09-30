//! The operator commands' files: `env.stringify` (the environment file as the reference's
//! `envfile` writes it), `env.parseCli` (the file as the reference's CLI reads it, which the
//! node's one parser reads differently on some lines: [`Documented::CliEnvParser`]) and
//! `config.forgerKey` (the forger's key written into `delegates.json`).
//!
//! [`Documented::CliEnvParser`]: crate::subject::Documented::CliEnvParser

use super::{Env, Outcome, Run, failed};
use crate::subject::{Case, Verdict};

/// The operations of this module.
pub(super) const OPS: &[(&str, Run)] = &[
    ("env.stringify", env_stringify),
    ("env.parseCli", env_parse_cli),
    ("config.forgerKey", config_forger_key),
];

fn env_stringify(_env: &Env<'_>, _case: &Case<'_>) -> Outcome<Verdict> {
    Err(failed("env.stringify has no Rust implementation yet"))
}

fn env_parse_cli(_env: &Env<'_>, _case: &Case<'_>) -> Outcome<Verdict> {
    Err(failed("env.parseCli has no Rust implementation yet"))
}

fn config_forger_key(_env: &Env<'_>, _case: &Case<'_>) -> Outcome<Verdict> {
    Err(failed("config.forgerKey has no Rust implementation yet"))
}
