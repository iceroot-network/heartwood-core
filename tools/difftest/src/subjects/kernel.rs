//! The kernel and the logger: rounds, forging slots, block chaining, the status canonical JSON,
//! `.env` text, epoch timestamps, IP lists, node paths and log lines.
//!
//! `round.calculate` and `round.isNew` also take a range, `{from, to}`, which the oracle answers
//! with the SHA-256 of one line per height (`h round roundHeight nextRound maxDelegates`) and with
//! the heights that start a round.

use std::borrow::Cow;
use std::path::Path;

use heartwood_crypto::BlockId;
use heartwood_crypto::crypto::hash::sha256;
use heartwood_crypto::utils::hex;
use heartwood_kernel::config::dot_env::{self, EnvValue};
use heartwood_kernel::errors::{NotChained, TimestampError};
use heartwood_kernel::ip as ips;
use heartwood_kernel::log::Level;
use heartwood_kernel::utils::stringify as canonical;
use heartwood_kernel::{
    ChainLink, calculate_forging_info, calculate_round, is_block_chained, is_new_round,
    paths as dirs,
};
use heartwood_logger::format;
use serde_json::{Map, Value, json};

use super::{Env, Outcome, failed, params, rejected, text};
use crate::subject::{Case, Documented, SubjectError, Verdict};
use crate::vectors::Expected;

/// A value of the input that Heartwood's types cannot hold: the case is outside the domain.
fn outside(what: &str) -> SubjectError {
    SubjectError::Unsupported(format!("{what} is outside Heartwood's domain"))
}

/// `input.key` as a height, 1 to `2^32 − 1`.
fn height_of(input: &Value, key: &str) -> Outcome<u32> {
    let value = input[key]
        .as_i64()
        .ok_or_else(|| failed(format!("input.{key} is not an integer")))?;
    u32::try_from(value)
        .ok()
        .filter(|height| *height >= 1)
        .ok_or_else(|| outside(&format!("input.{key} {value}")))
}

/// `input.key` as a JSON integer.
fn int(input: &Value, key: &str) -> Outcome<i64> {
    input[key]
        .as_i64()
        .ok_or_else(|| failed(format!("input.{key} is not an integer")))
}

/// The heights of a `{from, to}` range.
fn range(input: &Value) -> Outcome<std::ops::RangeInclusive<u32>> {
    Ok(height_of(input, "from")?..=height_of(input, "to")?)
}

/// `round.calculate`: the round of a height, or the digest of a range of heights.
pub(super) fn round_calculate(env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let milestones = env.milestones(case)?;
    let input = case.input;
    if input.get("height").is_some() {
        let info = calculate_round(height_of(input, "height")?, &milestones);
        return Ok(Verdict::Output(json!({
            "round": info.round,
            "roundHeight": info.round_height,
            "nextRound": info.next_round,
            "maxDelegates": info.max_delegates,
        })));
    }
    let heights = range(input)?;
    let mut lines = String::new();
    for height in heights.clone() {
        let info = calculate_round(height, &milestones);
        lines.push_str(&format!(
            "{height} {} {} {} {}\n",
            info.round, info.round_height, info.next_round, info.max_delegates
        ));
    }
    Ok(Verdict::Output(json!({
        "count": heights.count(),
        "sha256": hex::encode(&sha256(lines.as_bytes())),
    })))
}

/// `round.isNew`: whether a new round starts at a height, or the heights of a range that start
/// one.
pub(super) fn round_is_new(env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let milestones = env.milestones(case)?;
    let input = case.input;
    if input.get("height").is_some() {
        let is_new = is_new_round(height_of(input, "height")?, &milestones);
        return Ok(Verdict::Output(json!({ "isNewRound": is_new })));
    }
    let heights = range(input)?;
    let new_rounds: Vec<u32> = heights
        .clone()
        .filter(|height| is_new_round(*height, &milestones))
        .collect();
    Ok(Verdict::Output(json!({
        "count": heights.count(),
        "newRounds": new_rounds,
    })))
}

/// `forging.info`: the forging slot of a time.
pub(super) fn forging_info(env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let milestones = env.milestones(case)?;
    let info = calculate_forging_info(
        int(case.input, "timestamp")?,
        height_of(case.input, "height")?,
        &milestones,
    );
    Ok(Verdict::Output(json!({
        "currentForger": info.current_forger,
        "nextForger": info.next_forger,
        "blockTimestamp": info.block_timestamp,
        "canForge": info.can_forge,
    })))
}

/// The chaining fields of a block in the input.
fn link(block: &Value, previous_key: &str) -> Outcome<ChainLink> {
    let id = |key: &str| -> Outcome<BlockId> {
        match block.get(key) {
            None => Ok(BlockId::from_array([0; 32])),
            Some(value) => value
                .as_str()
                .and_then(|text| BlockId::from_hex(text).ok())
                .filter(|id| value.as_str() == Some(id.to_hex().as_str()))
                .ok_or_else(|| outside(&format!("the block id {value}"))),
        }
    };
    let number = |key: &str| -> Outcome<u32> {
        let value = int(block, key)?;
        u32::try_from(value).map_err(|_| outside(&format!("{key} {value}")))
    };
    Ok(ChainLink {
        id: id("id")?,
        previous_block: id(previous_key)?,
        height: number("height")?,
        timestamp: number("timestamp")?,
    })
}

/// `block.isChained`: whether a block chains onto the previous one, and the check that refuses
/// it.
pub(super) fn block_is_chained(env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let milestones = env.milestones(case)?;
    let previous = link(&case.input["previous"], "")?;
    let next = link(&case.input["next"], "previousBlock")?;
    let output = match is_block_chained(&previous, &next, &milestones) {
        Ok(()) => json!({ "isChained": true }),
        Err(NotChained::PreviousBlock) => json!({ "isChained": false, "reason": "previousBlock" }),
        Err(NotChained::HeightNotPlusOne) => json!({ "isChained": false, "reason": "height" }),
        Err(NotChained::SlotNotAfterPrevious { previous, next }) => json!({
            "isChained": false,
            "reason": "slot",
            "previousSlot": previous,
            "nextSlot": next,
        }),
    };
    Ok(Verdict::Output(output))
}

/// `kernel.stringify`: the status canonical JSON of a value.
pub(super) fn stringify(_env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    Ok(Verdict::Output(
        json!({ "text": canonical::stringify(&case.input["value"]) }),
    ))
}

/// `kernel.dotenv`: the pairs of `.env` text, and the text written back from them.
pub(super) fn dotenv(_env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let pairs = dot_env::parse(text(case.input, "text")?);
    let listed: Vec<Value> = pairs
        .iter()
        .map(|(key, value)| {
            let value = match value {
                EnvValue::Number(number) => Value::from(*number),
                EnvValue::Bool(flag) => Value::Bool(*flag),
                EnvValue::Text(text) => Value::String(text.clone()),
            };
            json!([key, value])
        })
        .collect();
    Ok(Verdict::Output(json!({
        "pairs": listed,
        "text": dot_env::stringify(&pairs),
    })))
}

/// `kernel.formatTimestamp`: the three forms of an epoch timestamp.
pub(super) fn format_timestamp(env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let milestones = env.milestones(case)?;
    let epoch = params(&milestones, case)?.epoch();
    let stamp = int(case.input, "timestamp")?;
    match heartwood_kernel::timestamp::format_timestamp(stamp, epoch) {
        Ok(time) => Ok(Verdict::Output(json!({
            "epoch": time.epoch,
            "unix": time.unix,
            "human": time.human,
        }))),
        Err(TimestampError::OutOfRange { .. }) => Ok(rejected("RangeError")),
        Err(error @ TimestampError::Epoch(_)) => Err(failed(error.to_string())),
    }
}

/// `kernel.ip`: whether an address is whitelisted and blacklisted by a list, and the address
/// helpers.
pub(super) fn ip(_env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let list: Vec<String> = case.input["list"]
        .as_array()
        .ok_or_else(|| failed("input.list is not an array"))?
        .iter()
        .map(|entry| entry.as_str().map(str::to_owned))
        .collect::<Option<_>>()
        .ok_or_else(|| failed("input.list holds a value that is not text"))?;
    let address = text(case.input, "ip")?;
    Ok(Verdict::Output(json!({
        "whitelisted": ips::is_whitelisted(&list, address),
        "blacklisted": ips::is_blacklisted(&list, address),
        "valid": ips::is_valid_address(address),
        "ipv6": ips::is_ipv6_address(address),
        "normalised": ips::normalise_address(address),
        "cleaned": ips::clean_address(address),
    })))
}

/// `kernel.paths`: the directories of a node.
pub(super) fn paths(_env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let input = case.input;
    let vars: Map<String, Value> = match input.get("env") {
        None => Map::new(),
        Some(Value::Object(vars)) => vars.clone(),
        Some(_) => return Err(failed("input.env is not an object")),
    };
    let lookup = |name: &str| vars.get(name).and_then(Value::as_str).map(str::to_owned);
    let cwd = input.get("cwd").and_then(Value::as_str).unwrap_or("/");
    let found = dirs::resolve_paths(
        text(input, "token")?,
        text(input, "network")?,
        &lookup,
        Path::new(text(input, "home")?),
        Path::new(cwd),
    );
    let shown = |path: &Path| path.to_string_lossy().into_owned();
    Ok(Verdict::Output(json!({
        "data": shown(&found.data),
        "config": shown(&found.config),
        "cache": shown(&found.cache),
        "log": shown(&found.log),
        "temp": shown(&found.temp),
    })))
}

/// `logger.format`: the line the logger writes for one message. A message with a character the
/// logger escapes is [`Documented::EscapedLogText`], judged against [`escaped_line`].
pub(super) fn logger_format(_env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let input = case.input;
    let name = text(input, "level")?;
    let level = Level::from_name(name).ok_or_else(|| failed(format!("unknown level {name}")))?;
    let flag = |key: &str| {
        input[key]
            .as_bool()
            .ok_or_else(|| failed(format!("input.{key} is not a boolean")))
    };
    let message = text(input, "message")?;
    let emojified = format::emojify(message, flag("emojify")?);
    let line = if message.is_empty() {
        None
    } else {
        format::line(level, int(input, "time")?, &emojified, flag("colorize")?)
    };
    let verdict = Verdict::Output(json!({ "line": line }));
    Ok(match format::escape(&emojified) {
        Cow::Owned(_) => Verdict::DiffersWhereShown {
            difference: Documented::EscapedLogText,
            verdict: Box::new(verdict),
        },
        Cow::Borrowed(_) => verdict,
    })
}

/// The reference's `logger.format` answer `expected` with its message escaped as Heartwood's
/// logger escapes it: the line up to the level as it is, then the message, read back from the
/// reference's line and escaped, cyan when the line is coloured. The reference's colour library
/// closes the cyan before each line break and reopens it after, and reopens it after each colour
/// reset inside the message; that is undone first. `None` when the answer has no line.
pub(crate) fn escaped_line(expected: &Expected) -> Option<Expected> {
    const OPEN: &str = "\u{1b}[36m";
    const CLOSE: &str = "\u{1b}[39m";
    let Expected::Output(output) = expected else {
        return None;
    };
    let line = output["line"].as_str()?.strip_suffix('\n')?;
    let (head, body) = line.split_at(line.find(": ")? + 2);
    // Only a coloured line has an escape code before its message.
    let coloured = head.contains('\u{1b}');
    let message = if coloured {
        body.strip_prefix(OPEN)?
            .strip_suffix(CLOSE)?
            .replace(&format!("{CLOSE}\r\n{OPEN}"), "\r\n")
            .replace(&format!("{CLOSE}\n{OPEN}"), "\n")
            .replace(&format!("{CLOSE}{OPEN}"), CLOSE)
    } else {
        body.to_owned()
    };
    let escaped = format::escape(&message);
    let line = if coloured {
        format!("{head}{OPEN}{escaped}{CLOSE}\n")
    } else {
        format!("{head}{escaped}\n")
    };
    Some(Expected::Output(json!({ "line": line })))
}
