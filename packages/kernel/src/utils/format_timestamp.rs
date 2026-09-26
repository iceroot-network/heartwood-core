//! A timestamp in seconds since the network epoch, as the API and log lines show it.

use crate::errors::TimestampError;

/// The three forms of a timestamp.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormattedTimestamp {
    /// Seconds since the network epoch, as given.
    pub epoch: i64,
    /// Seconds since the Unix epoch.
    pub unix: i64,
    /// The UTC date and time with milliseconds, as JavaScript's `toISOString` writes it
    /// (`2026-01-01T00:00:08.000Z`).
    pub human: String,
}

/// `epoch_stamp` seconds after the milestone epoch `epoch` (the milestone's `epoch` text).
pub fn format_timestamp(
    epoch_stamp: i64,
    epoch: &str,
) -> Result<FormattedTimestamp, TimestampError> {
    let _ = (epoch_stamp, epoch);
    todo!()
}
