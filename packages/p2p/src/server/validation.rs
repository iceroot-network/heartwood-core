//! The server's request checks, as the reference's server makes them (and as the oracle's
//! `p2p.validateRequest` answers them): the route's payload limit (413), the codec's decode (400
//! "Payload deserialising failed"), the version gate on `headers.version` (400 "Validation
//! failed (invalid version)"), then the route's request schema, the reference's schemas of the
//! seven public routes and `emitEvent` (400 "Validation failed").

use heartwood_crypto::Params;
use heartwood_p2p_wire::RouteId;

use super::seams::ServerRefusal;

/// The version gate's inputs: the milestone at the tip plus one, the `minimumVersions` option and
/// the network's name.
pub struct VersionGate<'a> {
    /// The milestone.
    pub milestone: &'a Params,
    /// The configured version ranges.
    pub configured: &'a [String],
    /// The network's name.
    pub network_name: &'a str,
}

/// What the plugins after the checks read of an accepted request's headers.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Checked {
    /// The version the headers carry.
    pub version: Option<String>,
    /// The port the headers carry.
    pub port: Option<u16>,
}

/// Check `payload`, a request on `route`, as the reference's server does.
pub fn check_request(
    route: RouteId,
    payload: &[u8],
    gate: &VersionGate<'_>,
) -> Result<Checked, ServerRefusal> {
    let _ = (route, payload, gate);
    todo!("the request checks")
}
