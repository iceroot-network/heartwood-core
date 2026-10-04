//! The operator commands' files: `env.stringify` (the environment file as the reference's
//! `envfile` writes it), `env.parseCli` (the file as the reference's CLI reads it, which the
//! node's one parser reads differently on some lines: [`Documented::CliEnvParser`]) and
//! `config.forgerKey` (the forger's key written into `delegates.json`).
//!
//! [`Documented::CliEnvParser`]: crate::subject::Documented::CliEnvParser

use heartwood_cli::{config, env};
use heartwood_crypto::utils::js;
use serde_json::{Value, json};

use super::{Env, Outcome, Run, failed, text, unclassified};
use crate::subject::{Case, Documented, Verdict};

/// The operations of this module.
pub(super) const OPS: &[(&str, Run)] = &[
    ("env.stringify", env_stringify),
    ("env.parseCli", env_parse_cli),
    ("config.forgerKey", config_forger_key),
];

/// The variables of `case`'s `vars` (`[key, value]` pairs of text or numbers), each value as
/// JavaScript writes it.
fn vars(case: &Case<'_>) -> Outcome<Vec<(String, String)>> {
    let list = case.input["vars"]
        .as_array()
        .ok_or_else(|| failed("input.vars is not a list"))?;
    let mut pairs = Vec::new();
    for pair in list {
        let (key, value) = match pair.as_array().map(Vec::as_slice) {
            Some([Value::String(key), value]) => (key, value),
            _ => return Err(failed("a variable is not [key, value]")),
        };
        let value = match (value.as_str(), value.as_f64()) {
            (Some(text), _) => text.to_owned(),
            (None, Some(number)) => js::number_to_string(number),
            _ => return Err(failed("a value is not text or a number")),
        };
        pairs.push((key.clone(), value));
    }
    Ok(pairs)
}

/// The variables of a file's text and `vars` assigned to them, written as `env set` writes the
/// file. The node's parser reads the file, which the reference's does not do alike on some lines.
fn env_stringify(_env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let mut pairs = match case.input["file"].as_str() {
        Some(file) => env::parse(file),
        None => Vec::new(),
    };
    pairs.extend(vars(case)?);
    let verdict = Verdict::Output(json!({ "text": env::stringify(&pairs) }));
    Ok(Verdict::DiffersWhereShown {
        difference: Documented::CliEnvParser,
        verdict: Box::new(verdict),
    })
}

/// The file's variables as the node reads them, in order. The reference's CLI reads it with
/// envfile, and the two differ on some lines: [`Documented::CliEnvParser`].
fn env_parse_cli(_env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let pairs: Vec<Value> = env::parse(text(case.input, "text")?)
        .into_iter()
        .map(|(key, value)| json!([key, value]))
        .collect();
    Ok(Verdict::DiffersWhereShown {
        difference: Documented::CliEnvParser,
        verdict: Box::new(Verdict::Output(json!({ "pairs": pairs }))),
    })
}

/// The key file after `config forger private-key`: the flag validated, then `keys` set in the
/// existing file. Every refusal is a verdict only: the reference's errors (the validator's
/// message, `require`'s failure, a `TypeError` on a value that is no object) are not compared.
fn config_forger_key(_env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let Some(key) = case.input["flags"]["privateKey"].as_str() else {
        return Ok(unclassified("the privateKey flag is not a string"));
    };
    let Some(existing) = case.input["delegatesJson"].as_str() else {
        return Ok(unclassified("there is no delegates.json"));
    };
    Ok(match config::with_key(existing, key) {
        Ok(text) => Verdict::Output(json!({ "delegatesJson": text.as_str() })),
        Err(error) => unclassified(error),
    })
}
