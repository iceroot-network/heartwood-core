//! Network exposure: the P2P listener, the API's exposure, the forger's listeners, `remoteAccess`,
//! the host firewall and reachability.

use crate::doctor::registry::{CheckInput, not_built};
use crate::doctor::report::Finding;

/// `net.p2p-listen`.
pub fn p2p_listen(input: &CheckInput<'_>) -> Vec<Finding> {
    not_built(input)
}

/// `net.api-exposure`.
pub fn api_exposure(input: &CheckInput<'_>) -> Vec<Finding> {
    not_built(input)
}

/// `net.forger-no-listener`.
pub fn forger_no_listener(input: &CheckInput<'_>) -> Vec<Finding> {
    not_built(input)
}

/// `net.remote-access`.
pub fn remote_access(input: &CheckInput<'_>) -> Vec<Finding> {
    not_built(input)
}

/// `net.firewall`.
pub fn firewall(input: &CheckInput<'_>) -> Vec<Finding> {
    not_built(input)
}

/// `net.reachable`.
pub fn reachable(input: &CheckInput<'_>) -> Vec<Finding> {
    not_built(input)
}
