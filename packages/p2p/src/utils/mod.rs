//! The peer rules: the address rules, the version gate and its npm semver subset, the /24 subnet
//! test, and [`rules::Rules`], which gives them and the wire's requests, reply checks and the
//! outgoing rate limiter to the rest of the client through [`crate::seams::PeerRules`].

pub mod is_valid_peer;
pub mod is_valid_version;
pub mod npm_semver;
pub mod rules;
pub mod subnet;

#[cfg(test)]
mod tests;
