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

use std::collections::BTreeMap;

use crate::live::run::{Window, parse_time};

/// The socket-close reasons.
pub const SOCKET_CLOSE: [&str; 2] = [
    "Did not make an endpoint request",
    "Failed to complete websocket handshake",
];

/// The reason of a "Banning" line without what the line ends with after it: the reference's
/// police-car emoji, Heartwood's `:oncoming_police_car:` short code, and the space before either.
pub fn reason_text(reason: &str) -> &str {
    let mut text = reason.trim_end();
    // A short code: `:name:` of letters, digits and underscores after a space.
    if let Some(inner) = text.strip_suffix(':')
        && let Some((head, name)) = inner.rsplit_once(':')
        && !name.is_empty()
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && head.ends_with(' ')
    {
        text = head.trim_end();
    }
    text.trim_end_matches(|c: char| !c.is_ascii() || c.is_whitespace())
}

/// Whether `reason` is a socket-close reason (with or without its trailing emoji).
pub fn is_socket_close(reason: &str) -> bool {
    SOCKET_CLOSE.contains(&reason_text(reason))
}

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
    if !is_socket_close(&ban.reason) {
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

/// The class a line outside every fault window of its node is counted in.
pub const OUTSIDE: &str = "outside";

/// The class of the stall, whose lines are left out of every comparison.
pub const STALL: &str = "stall";

/// Counts of log events per node, window class and kind: the unit a mixed run's log lines are
/// compared with the baseline run's.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Tally {
    counts: BTreeMap<(String, String, String), usize>,
}

/// How one count stands against the baseline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ruling {
    /// No more than the baseline's count, which is the number.
    Within(usize),
    /// More than the baseline's count, which is the number.
    Over(usize),
    /// Left out of the comparison, with the reason.
    Left(&'static str),
}

/// One count and its ruling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// The node the count is keyed by.
    pub node: String,
    /// The window class.
    pub class: String,
    /// The kind of line.
    pub kind: String,
    /// The count.
    pub count: usize,
    /// How it stands.
    pub ruling: Ruling,
}

impl Finding {
    /// The finding as a sentence: what was counted and against what.
    pub fn describe(&self) -> String {
        let what = format!(
            "{} {} lines of {} in {}",
            self.count, self.kind, self.node, self.class
        );
        match &self.ruling {
            Ruling::Within(n) => format!("{what} (the baseline had {n})"),
            Ruling::Over(n) => format!("{what}, more than the baseline's {n}"),
            Ruling::Left(why) => format!("{what}, left out: {why}"),
        }
    }
}

impl Tally {
    /// Count one more `kind` line of `node` in `class`.
    pub fn add(&mut self, node: &str, class: &str, kind: &str) {
        *self
            .counts
            .entry((node.to_owned(), class.to_owned(), kind.to_owned()))
            .or_default() += 1;
    }

    /// The count of `kind` lines of `node` in `class`.
    pub fn get(&self, node: &str, class: &str, kind: &str) -> usize {
        self.counts
            .get(&(node.to_owned(), class.to_owned(), kind.to_owned()))
            .copied()
            .unwrap_or(0)
    }

    /// Whether nothing was counted.
    pub fn is_empty(&self) -> bool {
        self.counts.is_empty()
    }

    /// Every count, in order of node, class and kind.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str, &str, usize)> {
        self.counts
            .iter()
            .map(|((n, c, k), v)| (n.as_str(), c.as_str(), k.as_str(), *v))
    }

    /// Add every count of `other` to this one.
    pub fn merge(&mut self, other: &Tally) {
        for (key, v) in &other.counts {
            *self.counts.entry(key.clone()).or_default() += v;
        }
    }

    /// The largest count of each node, class and kind over `tallies` (one tally per source of
    /// lines: a reader of a log, a peer banned), times `sources`: what `sources` sources may give
    /// if each gives no more than the one that gave most.
    pub fn most(tallies: &[Tally], sources: usize) -> Tally {
        let mut out = Tally::default();
        for tally in tallies {
            for (key, v) in &tally.counts {
                let entry = out.counts.entry(key.clone()).or_default();
                *entry = (*entry).max(*v);
            }
        }
        for v in out.counts.values_mut() {
            *v *= sources;
        }
        out
    }

    /// Rule on every count against `baseline`, whose own fault windows are `windows`:
    ///
    /// - the stall's lines are left out (they are the await check's evidence);
    /// - a count outside every window fails whatever the baseline gave: these lines belong inside
    ///   a fault window;
    /// - a count in a class the baseline had a window of for the same node is compared with the
    ///   baseline's count in that class;
    /// - a restart count of a node the baseline never restarted is compared with the baseline's
    ///   `fallback` node's restart count, when that node had a restart window;
    /// - any other count is left out: the baseline has no fault of that class to compare with.
    pub fn compare(&self, baseline: &Tally, windows: &[Window], fallback: &str) -> Vec<Finding> {
        let had = |node: &str, class: &str| {
            windows
                .iter()
                .any(|w| w.class() == class && w.concerns(node) && !w.is_partition())
        };
        self.iter()
            .map(|(node, class, kind, count)| {
                let ruling = if class == STALL {
                    Ruling::Left("the stall's lines are evidence for the await check")
                } else if class == OUTSIDE {
                    rule(count, 0)
                } else if had(node, class) {
                    rule(count, baseline.get(node, class, kind))
                } else if class == "restart" && had(fallback, class) {
                    rule(count, baseline.get(fallback, class, kind))
                } else {
                    Ruling::Left("the baseline has no fault of that class for the node")
                };
                Finding {
                    node: node.to_owned(),
                    class: class.to_owned(),
                    kind: kind.to_owned(),
                    count,
                    ruling,
                }
            })
            .collect()
    }
}

fn rule(count: usize, allowed: usize) -> Ruling {
    if count > allowed {
        Ruling::Over(allowed)
    } else {
        Ruling::Within(allowed)
    }
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
        // The reason as the reference logs it, with its emoji, and as Heartwood does, with a code.
        for suffixed in [
            format!("{close} 🚔"),
            format!("{close} :oncoming_police_car:"),
        ] {
            assert_eq!(
                classify(&ban(&suffixed, Some(20_000)), "node4", &windows),
                Verdict::InWindow("sigkill".into()),
                "{suffixed}"
            );
        }
        assert_eq!(
            reason_text("Validation failed (invalid version) 🚔"),
            "Validation failed (invalid version)"
        );
        assert_eq!(reason_text("Rate limit exceeded"), "Rate limit exceeded");
        let json = serde_json::json!({"node": "node2", "ts": "1970-01-01T00:00:20.000Z", "ip": "198.51.100.14", "reason": close});
        assert_eq!(Ban::from_json(&json).unwrap().at, Some(20_000));
    }

    #[test]
    fn counts_compare_per_node_and_class() {
        let win = |fault: &str, node: &str| Window {
            fault: fault.into(),
            nodes: vec![node.into()],
            start: 0,
            end: 1,
        };
        let baseline_windows = [win("sigkill", "node4"), win("stall", "node2")];
        let mut baseline = Tally::default();
        baseline.add("node4", "restart", "close");
        baseline.add("node4", "restart", "close");
        baseline.add("node3", OUTSIDE, "close");
        let mut run = Tally::default();
        for _ in 0..3 {
            run.add("node4", "restart", "close");
        }
        run.add("node2", "stall", "close");
        run.add("node6", "restart", "close");
        run.add("node6", "restart", "close");
        run.add("node4", "partition", "close");
        run.add("node3", OUTSIDE, "close");
        run.add("node3", OUTSIDE, "close");
        let found = run.compare(&baseline, &baseline_windows, "node4");
        let ruling = |node: &str, class: &str| {
            found
                .iter()
                .find(|f| f.node == node && f.class == class)
                .map(|f| f.ruling.clone())
                .unwrap()
        };
        assert_eq!(ruling("node4", "restart"), Ruling::Over(2));
        assert!(matches!(ruling("node2", "stall"), Ruling::Left(_)));
        // Node 6 was never restarted in the baseline: node 4's restart count is the allowance.
        assert_eq!(ruling("node6", "restart"), Ruling::Within(2));
        assert!(matches!(ruling("node4", "partition"), Ruling::Left(_)));
        // Outside every window nothing is allowed, whatever the baseline logged there.
        assert_eq!(ruling("node3", OUTSIDE), Ruling::Over(0));
        assert!(found[0].describe().contains("lines of node"));
    }

    #[test]
    fn the_most_of_one_source_times_the_sources() {
        let of = |n: usize| {
            let mut t = Tally::default();
            for _ in 0..n {
                t.add("node4", "restart", "transport");
            }
            t
        };
        let most = Tally::most(&[of(1), of(3), of(2)], 2);
        assert_eq!(most.get("node4", "restart", "transport"), 6);
        assert!(Tally::most(&[], 3).is_empty());
        let mut sum = of(1);
        sum.merge(&of(2));
        assert_eq!(sum.get("node4", "restart", "transport"), 3);
    }
}
