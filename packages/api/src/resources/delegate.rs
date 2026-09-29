//! The delegate resource of the delegate routes: username, address, public key, votes received
//! with their percent of the supply, rank, resignation, the blocks produced and missed with the
//! productivity and the last block, the forged fees, burned fees, rewards, donations and total,
//! and the version.
//!
//! The resource is built as the listings search and sort it: a BigNumber is a
//! [`big_number`](crate::search::big_number) and the version a [`semver`](crate::search::semver);
//! [`plain`](crate::search::plain) writes it out. A key that the reference leaves `undefined` is
//! absent.

use heartwood_crypto::BlockId;
use heartwood_pool::PoolView;
use heartwood_state::{Resignation, Wallet, WalletRepository};
use serde_json::{Map, Value, json};

use super::hundredths;
use crate::context::ProductivityTable;
use crate::errors::ApiError;
use crate::search::convert::{Big, json_number};
use crate::search::{big_number, semver};

/// The supply the reference's calculator gives: every balance added.
pub fn supply(wallets: &WalletRepository) -> u128 {
    heartwood_state::supply::total_balance(wallets)
}

/// The resource of the delegate `wallet` at `supply`; an error (a 500) for a wallet without a
/// public key and for a vote percent the reference's arithmetic cannot make.
pub fn delegate(
    wallet: &Wallet,
    supply: u128,
    pool: &PoolView,
    table: &ProductivityTable,
) -> Result<Value, ApiError> {
    let (Some(delegate), Some(key)) = (wallet.delegate(), wallet.public_key()) else {
        return Err(ApiError::internal());
    };
    let key = key.to_compressed();
    let percent =
        vote_percent(u128::from(delegate.vote_balance), supply).ok_or_else(ApiError::internal)?;
    let resignation = delegate.resigned.map(|resigned| match resigned {
        Resignation::Permanent => "permanent",
        Resignation::Temporary => "temporary",
    });
    let production = table.by_username.get(delegate.username());
    let forged = |fees: u64| i128::from(fees);
    let total = forged(delegate.forged_fees) - forged(delegate.burned_fees)
        + forged(delegate.forged_rewards)
        - forged(delegate.donations);
    let mut resource = Map::new();
    let put = |map: &mut Map<String, Value>, name: &str, value: Option<Value>| {
        if let Some(value) = value {
            map.insert(name.to_owned(), value);
        }
    };
    put(&mut resource, "username", Some(json!(delegate.username())));
    put(
        &mut resource,
        "address",
        Some(json!(wallet.address().to_base58())),
    );
    put(&mut resource, "publicKey", Some(json!(key.to_hex())));
    let received = json!({
        "percent": json_number(percent), "votes": big_number(delegate.vote_balance),
        "voters": delegate.voters,
    });
    put(&mut resource, "votesReceived", Some(received));
    put(&mut resource, "rank", delegate.rank.map(|rank| json!(rank)));
    put(
        &mut resource,
        "isResigned",
        Some(json!(resignation.is_some())),
    );
    put(
        &mut resource,
        "resignationType",
        resignation.map(|kind| json!(kind)),
    );
    let mut blocks = Map::new();
    put(
        &mut blocks,
        "produced",
        Some(json!(delegate.produced_blocks)),
    );
    put(
        &mut blocks,
        "missed",
        production
            .and_then(|p| p.missed)
            .map(|missed| json!(missed)),
    );
    put(
        &mut blocks,
        "productivity",
        production.and_then(|p| p.productivity).map(hundredths),
    );
    put(
        &mut blocks,
        "last",
        delegate
            .last_block
            .as_ref()
            .map(|id: &BlockId| json!(id.to_hex())),
    );
    put(&mut resource, "blocks", Some(Value::Object(blocks)));
    let forged = json!({
        "fees": big_number(delegate.forged_fees), "burnedFees": big_number(delegate.burned_fees),
        "rewards": big_number(delegate.forged_rewards), "donations": big_number(delegate.donations),
        "total": big_number(total),
    });
    put(&mut resource, "forged", Some(forged));
    let version = pool.version(&key).filter(|version| !version.is_empty());
    put(&mut resource, "version", version.map(semver));
    Ok(Value::Object(resource))
}

/// `calculateVotePercent`: the share of the supply that `votes` are, in percent to two places, as
/// the reference computes it in `Number`s and `BigInt`s; `None` where it throws (no supply, or
/// votes with more than four digits beyond the supply's).
fn vote_percent(votes: u128, supply: u128) -> Option<f64> {
    let (votes, supply) = (votes.to_string(), supply.to_string());
    let exponent = i64::try_from(supply.len()).ok()? - i64::try_from(votes.len()).ok()? + 4;
    let scale = pow10(exponent);
    if scale.fract() != 0.0 || !scale.is_finite() {
        return None;
    }
    let scaled = Big::parse(&votes)?.times(&Big::parse(&format!("{scale:.0}"))?);
    let quotient = scaled.divided_by(&Big::parse(&supply)?)?;
    Some(to_fixed_2(quotient.to_f64() / pow10(exponent - 2)))
}

/// `Math.pow(10, k)` in Node 18: the double nearest 10^k, except at the exponents where V8's
/// repeated multiplication lands one step off.
fn pow10(k: i64) -> f64 {
    let text = match k {
        26 => "9.999999999999999e25".to_owned(),
        29 => "1.0000000000000001e29".to_owned(),
        34 => "1.0000000000000001e34".to_owned(),
        36 => "9.999999999999999e35".to_owned(),
        39 => "1.0000000000000001e39".to_owned(),
        42 => "9.999999999999999e41".to_owned(),
        49 => "1.0000000000000001e49".to_owned(),
        58 => "1.0000000000000001e58".to_owned(),
        _ => format!("1e{k}"),
    };
    text.parse().unwrap_or(f64::NAN)
}

/// `+x.toFixed(2)`: `x` rounded to two places by its exact value, a tie (as 1.125 and 0.625 are)
/// up; `x` itself from 1e21.
fn to_fixed_2(x: f64) -> f64 {
    if !x.is_finite() || x.abs() >= 1e21 {
        return x;
    }
    let exact = format!("{:.1100}", x.abs());
    let (whole, fraction) = exact.split_once('.').unwrap_or((&exact, ""));
    let mut digits: Vec<u8> = whole.bytes().chain(fraction.bytes().take(2)).collect();
    if fraction
        .as_bytes()
        .get(2)
        .is_some_and(|digit| *digit >= b'5')
    {
        for digit in digits.iter_mut().rev() {
            if *digit == b'9' {
                *digit = b'0';
            } else {
                *digit += 1;
                break;
            }
        }
        if digits.iter().all(|digit| *digit == b'0') {
            digits.insert(0, b'1');
        }
    }
    let split = digits.len().saturating_sub(2);
    let text = String::from_utf8_lossy(&digits);
    let rounded: f64 = format!(
        "{}.{}",
        text.get(..split).unwrap_or("0"),
        text.get(split..).unwrap_or("00")
    )
    .parse()
    .unwrap_or(f64::NAN);
    if x < 0.0 { -rounded } else { rounded }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_fixed_rounds_exact_ties_up_and_the_rest_by_value() {
        for (x, expected) in [
            (1.125, 1.13),
            (0.625, 0.63),
            (0.005, 0.01),
            (1.005, 1.0),
            (2.675, 2.67),
            (0.0, 0.0),
            (99.995, 100.0),
            (0.994999, 0.99),
            (9.995, 9.99),
            (0.5, 0.5),
            (99.999, 100.0),
        ] {
            assert_eq!(to_fixed_2(x), expected, "{x}");
        }
    }

    #[test]
    fn a_vote_percent_is_the_reference_s() {
        // A tenth of a supply of a thousand.
        assert_eq!(vote_percent(100, 1000), Some(10.0));
        assert_eq!(vote_percent(0, 1000), Some(0.0));
        assert_eq!(vote_percent(1, 3), Some(33.33));
        assert_eq!(vote_percent(1, 0), None);
        // Four digits more than the supply's exponent is negative.
        assert_eq!(vote_percent(1_000_000, 1), None);
        assert_eq!(vote_percent(9, 8), Some(112.5));
    }
}
