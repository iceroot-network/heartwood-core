//! The ban scan: the reference nodes' "Banning" lines that name a node under test, classified.
//!
//! The reference's BanHammer bans an address for three kinds of reasons:
//!
//! - a request it refused (a Boom message: "Validation failed", "Validation failed (invalid
//!   version)", "Rate limit exceeded", "Not a websocket connection", ...), and
//! - a nes termination ("Invalid message received", "Invalid payload received", "Malicious ping
//!   frame received", "Exceeded ping limit", ...): both fail a run wherever they appear;
//! - a socket that closed without a request or before its handshake completed ("Did not make an
//!   endpoint request", "Failed to complete websocket handshake"), which the reference also gives a
//!   reference relay whose sockets die in a kill or a partition: allowed inside a fault window of
//!   the banned node or of a partition, a failure outside one.

use crate::live::run::{Window, parse_time};

/// The socket-close reasons.
pub const SOCKET_CLOSE: [&str; 2] = [
    "Did not make an endpoint request",
    "Failed to complete websocket handshake",
];

/// A ban line of `bans.jsonl`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ban {
    /// The reference node that banned.
    pub node: String,
    /// When, in milliseconds since the Unix epoch, if the line had a time.
    pub at: Option<i64>,
    /// The banned address.
    pub ip: String,
    /// The reason.
    pub reason: String,
}

impl Ban {
    /// The ban of a `bans.jsonl` line, if it is one.
    pub fn from_json(value: &serde_json::Value) -> Option<Ban> {
        let text = |k: &str| value.get(k).and_then(|v| v.as_str()).map(str::to_owned);
        Some(Ban {
            node: text("node")?,
            at: text("ts").and_then(|t| parse_time(&t)),
            ip: text("ip")?,
            reason: text("reason")?,
        })
    }
}

/// How the scan judges a ban of a node under test.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// A socket-close ban inside a fault window of the node or of a partition: recorded.
    InWindow(String),
    /// A socket-close ban outside every such window.
    OutsideWindow,
    /// A request refusal or a nes termination.
    Refusal,
}

/// Classify `ban` of the node `node` against the run's fault `windows`.
pub fn classify(ban: &Ban, node: &str, windows: &[Window]) -> Verdict {
    if !SOCKET_CLOSE.contains(&ban.reason.as_str()) {
        return Verdict::Refusal;
    }
    let Some(at) = ban.at else {
        return Verdict::OutsideWindow;
    };
    windows
        .iter()
        .find(|w| w.concerns(node) && w.contains(at))
        .map_or(Verdict::OutsideWindow, |w| {
            Verdict::InWindow(w.fault.clone())
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ban(reason: &str, at: Option<i64>) -> Ban {
        Ban {
            node: "node1".into(),
            at,
            ip: "198.51.100.14".into(),
            reason: reason.into(),
        }
    }

    #[test]
    fn reasons_and_windows() {
        let windows = [
            Window {
                fault: "sigkill".into(),
                nodes: vec!["node4".into()],
                start: 1_000,
                end: 40_000,
            },
            Window {
                fault: "partition".into(),
                nodes: vec![],
                start: 100_000,
                end: 200_000,
            },
            Window {
                fault: "forger-stop".into(),
                nodes: vec!["node2".into()],
                start: 300_000,
                end: 400_000,
            },
        ];
        let close = SOCKET_CLOSE[0];
        assert_eq!(
            classify(&ban(close, Some(20_000)), "node4", &windows),
            Verdict::InWindow("sigkill".into())
        );
        assert_eq!(
            classify(&ban(SOCKET_CLOSE[1], Some(150_000)), "node4", &windows),
            Verdict::InWindow("partition".into())
        );
        // Another node's fault is no window of this one.
        assert_eq!(
            classify(&ban(close, Some(350_000)), "node4", &windows),
            Verdict::OutsideWindow
        );
        assert_eq!(
            classify(&ban(close, None), "node4", &windows),
            Verdict::OutsideWindow
        );
        for reason in [
            "Validation failed (invalid version)",
            "Rate limit exceeded",
            "Invalid payload received",
            "Malicious ping frame received",
        ] {
            assert_eq!(
                classify(&ban(reason, Some(20_000)), "node4", &windows),
                Verdict::Refusal,
                "{reason}"
            );
        }
        let json = serde_json::json!({"node": "node2", "ts": "1970-01-01T00:00:20.000Z", "ip": "198.51.100.14", "reason": close});
        assert_eq!(Ban::from_json(&json).unwrap().at, Some(20_000));
    }
}
