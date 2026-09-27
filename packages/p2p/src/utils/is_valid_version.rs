//! The reference's version gate (`isValidVersion`).

use heartwood_crypto::Params;

/// `isValidVersion`: `version` is present, valid semver, and satisfies one of the ranges of the
/// milestone's `p2p.minimumVersions`, or of `configured` (the p2p option) when the milestone has
/// none, with prereleases included unless `network_name` is `mainnet`.
pub fn is_valid_version(
    version: Option<&str>,
    milestone: &Params,
    configured: &[String],
    network_name: &str,
) -> bool {
    let _ = (version, milestone, configured, network_name);
    todo!("the version gate")
}
