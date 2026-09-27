//! Seeds, peer sources and the plugin port probes.
//!
//! The seeds are `peers.json`'s `list` plus the first of its `sources` that gives a list: a local
//! file (a path starting with `/`) read as the reference reads it, or a plain HTTP URL fetched
//! with a hand-written `GET`. No seed at all stops the node. The port probes are a hand-written
//! HTTP/1.1 `HEAD /`: 200 records the port, anything else -1.

use std::net::IpAddr;
use std::time::Duration;

use heartwood_p2p_wire::replies::PeerAddress;

use crate::errors::SeedError;
use crate::seams::PortProbe;

/// The peers of the first source in `sources` that gives a list.
pub async fn load_sources(sources: &[String]) -> Result<Vec<PeerAddress>, SeedError> {
    let _ = sources;
    todo!("read the peer sources")
}

/// The seeds: `list`, then the peers of the sources whose IP `list` does not have, in order.
pub fn merge_seeds(list: &[PeerAddress], from_sources: &[PeerAddress]) -> Vec<PeerAddress> {
    let _ = (list, from_sources);
    todo!("merge the seeds")
}

/// The HTTP probe of plugin ports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HttpProbe {
    /// How long a probe may take.
    pub timeout: Duration,
}

impl PortProbe for HttpProbe {
    async fn head(&self, ip: IpAddr, port: u16) -> Option<u16> {
        let _ = (ip, port);
        todo!("an HTTP HEAD")
    }
}
