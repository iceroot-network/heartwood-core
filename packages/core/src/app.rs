//! The composition roots of the roles: [`relay`] (the relay's components, its peer-to-peer
//! server and its graceful shutdown), [`forger`] (the forger's) and [`mod@core`] (both in one
//! process), and the version floor they check.

pub mod core;
pub mod forger;
pub mod relay;

use heartwood_crypto::{Milestones, Params};
use heartwood_kernel::config::floor::{self, FloorError};
use heartwood_p2p::utils::is_valid_version::is_valid_version;

pub use self::relay::{
    RelayError, api_options, open_pool, peer_config, restored, run_relay, start, version_floor,
};
use crate::VERSION;

/// The version floor of `milestones` on the network named `network` at `height`, with the peers'
/// version gate and the minimum versions `configured`: refused when the floor in force refuses
/// this node's version; each later floor that refuses it is logged.
pub fn check_floor(
    milestones: &Milestones,
    network: &str,
    height: u32,
    configured: &[String],
) -> Result<(), FloorError> {
    let gate = |version: &str, params: &Params, network: &str| {
        is_valid_version(Some(version), params, configured, network)
    };
    for floor in floor::check(milestones, VERSION, network, height, &gate)? {
        let (height, ranges) = (floor.height, floor.ranges.join(", "));
        heartwood_kernel::warning!(
            "From height {height} the minimum versions are {ranges}, which this node's version {VERSION} does not satisfy"
        );
    }
    Ok(())
}
