//! The messages of `shared.proto`.

use crate::proto::message;

message! {
    /// The headers of a request: the client's version and its peer-to-peer port (0 from a
    /// forger).
    Headers {
        /// The client's version.
        1 version "version": string,
        /// The client's peer-to-peer port.
        2 port "port": u32,
    }
}
