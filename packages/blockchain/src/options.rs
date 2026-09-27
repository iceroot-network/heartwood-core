//! The blockchain options, with the reference's defaults: the database rollback bounds, the
//! missed-blocks lookback, and `networkStart`, which `--networkStart` sets.
//!
//! They are read as the reference's option schema reads them: `databaseRollback` must be an object
//! whose `maxBlockRewind` and `steps` are integers of at least 1 (numbers or their decimal text),
//! `networkStart` a boolean (or `"true"` or `"false"` in any case), and any other key is allowed
//! without a check. The first refusal names its key, in the schema's order. `missedBlocksLookback`
//! has no rule in the schema and no reader before the productivity tables; a value that is not a
//! non-negative integer leaves the default. A rewind bound above 2^32 − 1 is refused (the
//! reference takes one up to 2^53 − 1, and refuses a larger one as not a safe number).

use heartwood_kernel::config::{Env, Options, OptionsError};
use serde_json::Value;

/// The database rollback's bounds (`databaseRollback`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DatabaseRollback {
    /// The most blocks a rollback rewinds (`maxBlockRewind`).
    pub max_block_rewind: u32,
    /// The blocks removed per step (`steps`).
    pub steps: u32,
}

/// The blockchain component's options.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockchainOptions {
    /// The database rollback's bounds.
    pub database_rollback: DatabaseRollback,
    /// How far back the missed blocks are counted, in seconds (`missedBlocksLookback`).
    pub missed_blocks_lookback: u64,
    /// Whether the node starts a new network (`networkStart`).
    pub network_start: bool,
}

impl Default for BlockchainOptions {
    /// The reference's defaults.
    fn default() -> BlockchainOptions {
        BlockchainOptions {
            database_rollback: DatabaseRollback {
                max_block_rewind: 10_000,
                steps: 1_000,
            },
            missed_blocks_lookback: 2_592_000,
            network_start: false,
        }
    }
}

impl Options for BlockchainOptions {
    const PACKAGE: &'static str = "@solar-network/blockchain";

    /// The options of `value`; a key it does not give keeps its default.
    fn from_options(value: &Value, _env: &Env) -> Result<BlockchainOptions, OptionsError> {
        let refuse = |key: String, reason: &str| OptionsError {
            package: Self::PACKAGE,
            key,
            reason: reason.to_owned(),
        };
        let object = |value: &Value, key: &str| match value {
            Value::Object(map) => Ok(map.clone()),
            _ => Err(refuse(key.to_owned(), "must be of type object")),
        };
        let mut options = BlockchainOptions::default();
        let root = object(value, "value")?;
        if let Some(rollback) = root.get("databaseRollback") {
            let rollback = object(rollback, "databaseRollback")?;
            let bounds = &mut options.database_rollback;
            for (key, field) in [
                ("maxBlockRewind", &mut bounds.max_block_rewind),
                ("steps", &mut bounds.steps),
            ] {
                if let Some(given) = rollback.get(key) {
                    let refused = |reason| refuse(format!("databaseRollback.{key}"), reason);
                    *field = at_least_one(given).map_err(refused)?;
                }
            }
        }
        if let Some(given) = root.get("networkStart") {
            let refused = || refuse("networkStart".to_owned(), "must be a boolean");
            options.network_start = boolean(given).ok_or_else(refused)?;
        }
        let lookback = root.get("missedBlocksLookback").and_then(Value::as_u64);
        options.missed_blocks_lookback = lookback.unwrap_or(options.missed_blocks_lookback);
        Ok(options)
    }
}

/// An integer of at least 1, from a number or its decimal text (white space, a sign, digits with
/// a fraction or not, an exponent), as the schema's `number().integer().min(1)` reads it; the
/// refusal's reason otherwise.
fn at_least_one(value: &Value) -> Result<u32, &'static str> {
    let decimal = |text: &str| text.bytes().all(|byte| b"0123456789+-.eE".contains(&byte));
    let number = match value {
        Value::Number(number) => number.as_f64(),
        Value::String(text) if decimal(text.trim()) => text.trim().parse().ok(),
        _ => None,
    };
    let number: f64 = number.ok_or("must be a number")?;
    if number.fract() != 0.0 {
        Err("must be an integer")
    } else if number < 1.0 {
        Err("must be greater than or equal to 1")
    } else {
        let integer = format!("{number:.0}").parse::<u32>();
        integer.map_err(|_| "must be less than or equal to 4294967295")
    }
}

/// A boolean, or `"true"` or `"false"` in any case.
fn boolean(value: &Value) -> Option<bool> {
    match value {
        Value::Bool(flag) => Some(*flag),
        Value::String(text) => text.to_lowercase().parse().ok(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn read(value: Value) -> Result<BlockchainOptions, OptionsError> {
        BlockchainOptions::from_options(&value, &Env::default())
    }

    fn refused(value: Value) -> (String, String) {
        let error = read(value).unwrap_err();
        assert_eq!(error.package, "@solar-network/blockchain");
        (error.key, error.reason)
    }

    #[test]
    fn the_defaults_and_the_given_values_as_the_schema_reads_them() {
        assert_eq!(read(json!({})).unwrap(), BlockchainOptions::default());
        let options = read(json!({
            "databaseRollback": {"maxBlockRewind": "20", "steps": 5.0},
            "networkStart": "TRUE",
            "missedBlocksLookback": 60,
            "unknown": [1],
        }))
        .unwrap();
        assert_eq!(
            options.database_rollback,
            DatabaseRollback {
                max_block_rewind: 20,
                steps: 5
            }
        );
        assert!(options.network_start);
        assert_eq!(options.missed_blocks_lookback, 60);
        let options = read(json!({"networkStart": false, "missedBlocksLookback": "x"})).unwrap();
        assert!(!options.network_start);
        assert_eq!(options.missed_blocks_lookback, 2_592_000);
        for text in [" 1e2 ", "+100", "100.", "1.00e+2", "+.1e3"] {
            let options = read(json!({"databaseRollback": {"steps": text}})).unwrap();
            assert_eq!(options.database_rollback.steps, 100, "{text}");
        }
    }

    #[test]
    fn a_refusal_names_the_first_key_in_the_schema_order() {
        let key = |key: &str, reason: &str| (key.to_owned(), reason.to_owned());
        let rewind = "databaseRollback.maxBlockRewind";
        assert_eq!(refused(json!([])), key("value", "must be of type object"));
        assert_eq!(
            refused(json!({"databaseRollback": null, "networkStart": 1})),
            key("databaseRollback", "must be of type object")
        );
        for (given, reason) in [
            (json!(0), "must be greater than or equal to 1"),
            (json!(1.5), "must be an integer"),
            (json!("1x"), "must be a number"),
            (json!(".e1"), "must be a number"),
            (json!("inf"), "must be a number"),
            (json!("0x10"), "must be a number"),
            (json!(null), "must be a number"),
            (json!(true), "must be a number"),
            (
                json!(4_294_967_296_u64),
                "must be less than or equal to 4294967295",
            ),
        ] {
            let options = json!({"databaseRollback": {"maxBlockRewind": given, "steps": 0}});
            assert_eq!(refused(options), key(rewind, reason));
        }
        assert_eq!(
            refused(json!({"databaseRollback": {"steps": "0"}})),
            key(
                "databaseRollback.steps",
                "must be greater than or equal to 1"
            )
        );
        assert_eq!(
            refused(json!({"networkStart": "yes"})),
            key("networkStart", "must be a boolean")
        );
    }
}
