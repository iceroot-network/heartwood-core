//! The reference's version gate (`isValidVersion`).

use heartwood_crypto::Params;
use serde_json::Value;

use super::npm_semver;

/// `isValidVersion`: `version` is present and not empty, valid semver, and satisfies one of the
/// ranges of the milestone's `p2p.minimumVersions` when that is a list with entries, or else one
/// of `configured` (the p2p option), with prereleases included unless `network_name` is `mainnet`.
/// An entry of the milestone's list that is not text satisfies nothing.
pub fn is_valid_version(
    version: Option<&str>,
    milestone: &Params,
    configured: &[String],
    network_name: &str,
) -> bool {
    let Some(version) = version.filter(|version| npm_semver::valid(version).is_some()) else {
        return false;
    };
    let include_prerelease = network_name != "mainnet";
    let satisfied = |range: &str| npm_semver::satisfies(version, range, include_prerelease);
    let listed = (milestone.as_json().get("p2p"))
        .and_then(|p2p| p2p.get("minimumVersions"))
        .and_then(Value::as_array)
        .filter(|ranges| !ranges.is_empty());
    match listed {
        Some(ranges) => ranges
            .iter()
            .any(|range| range.as_str().is_some_and(satisfied)),
        None => configured.iter().any(|range| satisfied(range)),
    }
}
