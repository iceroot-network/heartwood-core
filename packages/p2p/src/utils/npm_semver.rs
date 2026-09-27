//! The subset of npm's `semver` that the version gate and the discovery log need: `valid`,
//! `satisfies` with and without prereleases, and the comparison of `gt` and `lt`.

use std::cmp::Ordering;

/// `semver.valid(text)`: the version in its normal form, or `None` when `text` is not valid
/// semver.
pub fn valid(text: &str) -> Option<String> {
    let _ = text;
    todo!("semver.valid")
}

/// `semver.satisfies(version, range, { includePrerelease })`.
pub fn satisfies(version: &str, range: &str, include_prerelease: bool) -> bool {
    let _ = (version, range, include_prerelease);
    todo!("semver.satisfies")
}

/// The order of two valid versions, as `semver.gt` and `semver.lt` compare them; `None` when
/// either is not valid.
pub fn compare(a: &str, b: &str) -> Option<Ordering> {
    let _ = (a, b);
    todo!("semver's comparison")
}
