//! The logging facade: the reference's eight levels as macros over `tracing`.
//!
//! Every crate logs through these macros, so consensus crates never link the subscriber, which
//! `heartwood-logger` installs in a node process. `tracing` has five levels; each macro emits an
//! event at the nearest `tracing` level and records the reference's level in the `severity`
//! field, which the logger reads back.
//!
//! ```
//! heartwood_kernel::notice!("Starting Round {}", 2);
//! heartwood_kernel::warning!(height = 5, "a block was disregarded");
//! ```

/// A log level of the reference, from the most to the least severe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Level {
    /// `emergency`.
    Emergency,
    /// `alert`.
    Alert,
    /// `critical`.
    Critical,
    /// `error`.
    Error,
    /// `warning`.
    Warning,
    /// `notice`.
    Notice,
    /// `info`.
    Info,
    /// `debug`.
    Debug,
}

impl Level {
    /// Every level, from the most to the least severe.
    pub const ALL: [Level; 8] = [
        Level::Emergency,
        Level::Alert,
        Level::Critical,
        Level::Error,
        Level::Warning,
        Level::Notice,
        Level::Info,
        Level::Debug,
    ];

    /// The reference's name of the level.
    pub const fn name(self) -> &'static str {
        match self {
            Level::Emergency => "emergency",
            Level::Alert => "alert",
            Level::Critical => "critical",
            Level::Error => "error",
            Level::Warning => "warning",
            Level::Notice => "notice",
            Level::Info => "info",
            Level::Debug => "debug",
        }
    }

    /// The level named `name`, as the reference's `CORE_LOG_LEVEL` values name them.
    pub fn from_name(name: &str) -> Option<Level> {
        Level::ALL.into_iter().find(|level| level.name() == name)
    }

    /// The `tracing` level the macros emit this level at.
    pub const fn tracing_level(self) -> tracing::Level {
        match self {
            Level::Emergency | Level::Alert | Level::Critical | Level::Error => {
                tracing::Level::ERROR
            }
            Level::Warning => tracing::Level::WARN,
            Level::Notice | Level::Info => tracing::Level::INFO,
            Level::Debug => tracing::Level::DEBUG,
        }
    }
}

/// What the macros expand to; not part of the API.
#[doc(hidden)]
pub mod __private {
    pub use tracing;
}

/// Emit an event at a reference level; the level macros below call it.
#[doc(hidden)]
#[macro_export]
macro_rules! __log {
    ($tracing:ident, $severity:literal, $($arg:tt)+) => {
        $crate::log::__private::tracing::event!(
            $crate::log::__private::tracing::Level::$tracing,
            severity = $severity,
            $($arg)+
        )
    };
}

/// Log at level `emergency`.
#[macro_export]
macro_rules! emergency {
    ($($arg:tt)+) => { $crate::__log!(ERROR, "emergency", $($arg)+) };
}

/// Log at level `alert`.
#[macro_export]
macro_rules! alert {
    ($($arg:tt)+) => { $crate::__log!(ERROR, "alert", $($arg)+) };
}

/// Log at level `critical`.
#[macro_export]
macro_rules! critical {
    ($($arg:tt)+) => { $crate::__log!(ERROR, "critical", $($arg)+) };
}

/// Log at level `error`.
#[macro_export]
macro_rules! error {
    ($($arg:tt)+) => { $crate::__log!(ERROR, "error", $($arg)+) };
}

/// Log at level `warning`.
#[macro_export]
macro_rules! warning {
    ($($arg:tt)+) => { $crate::__log!(WARN, "warning", $($arg)+) };
}

/// Log at level `notice`.
#[macro_export]
macro_rules! notice {
    ($($arg:tt)+) => { $crate::__log!(INFO, "notice", $($arg)+) };
}

/// Log at level `info`.
#[macro_export]
macro_rules! info {
    ($($arg:tt)+) => { $crate::__log!(INFO, "info", $($arg)+) };
}

/// Log at level `debug`.
#[macro_export]
macro_rules! debug {
    ($($arg:tt)+) => { $crate::__log!(DEBUG, "debug", $($arg)+) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip_in_order() {
        for (i, level) in Level::ALL.iter().enumerate() {
            assert_eq!(Level::from_name(level.name()), Some(*level));
            assert!(Level::ALL[..i].iter().all(|more| more < level));
        }
        assert_eq!(Level::from_name("warn"), None);
    }

    #[test]
    fn the_macros_expand() {
        crate::emergency!("{}", 1);
        crate::alert!("a");
        crate::critical!("a");
        crate::error!("a");
        crate::warning!(height = 5, "a");
        crate::notice!("a");
        crate::info!("a");
        crate::debug!("a");
    }
}
