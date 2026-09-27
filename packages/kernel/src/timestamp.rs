//! A timestamp in seconds since the network epoch, as the API and log lines show it.
//!
//! The reference adds the seconds to the milestone epoch and writes the result with
//! `Date.prototype.toISOString`, which refuses a time more than 8.64 × 10^15 milliseconds from the
//! Unix epoch (`RangeError`); [`format_timestamp`] refuses the same times.

use heartwood_crypto::crypto::slots;

use crate::errors::TimestampError;

/// The widest distance, in milliseconds, of a JavaScript `Date` from the Unix epoch.
const MAX_DATE_MS: i64 = 8_640_000_000_000_000;

/// The three forms of a timestamp.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormattedTimestamp {
    /// Seconds since the network epoch, as given.
    pub epoch: i64,
    /// Seconds since the Unix epoch.
    pub unix: i64,
    /// The UTC date and time with milliseconds, as JavaScript's `toISOString` writes it
    /// (`2026-01-01T00:00:08.000Z`; a year outside 0 to 9999 as `+010000` or `-000001`).
    pub human: String,
}

/// `epoch_stamp` seconds after the milestone epoch `epoch` (the milestone's `epoch` text).
pub fn format_timestamp(
    epoch_stamp: i64,
    epoch: &str,
) -> Result<FormattedTimestamp, TimestampError> {
    let epoch_ms =
        slots::epoch_unix_ms(epoch).ok_or_else(|| TimestampError::Epoch(epoch.to_owned()))?;
    let unix_ms = epoch_stamp
        .checked_mul(1_000)
        .and_then(|ms| ms.checked_add(epoch_ms))
        .ok_or(TimestampError::OutOfRange { epoch_stamp })?;
    let date =
        UtcDateTime::from_unix_ms(unix_ms).ok_or(TimestampError::OutOfRange { epoch_stamp })?;
    Ok(FormattedTimestamp {
        epoch: epoch_stamp,
        unix: unix_ms.div_euclid(1_000),
        human: date.to_iso_string(),
    })
}

/// A UTC date and time of the proleptic Gregorian calendar, as a JavaScript `Date` reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UtcDateTime {
    /// The year; 0 is 1 BC, and negative years count further back.
    pub year: i64,
    /// The month, 1 to 12.
    pub month: u8,
    /// The day of the month, 1 to 31.
    pub day: u8,
    /// The hour, 0 to 23.
    pub hour: u8,
    /// The minute, 0 to 59.
    pub minute: u8,
    /// The second, 0 to 59.
    pub second: u8,
    /// The millisecond, 0 to 999.
    pub millisecond: u16,
}

impl UtcDateTime {
    /// The date and time `unix_ms` milliseconds after the Unix epoch, or `None` when that is more
    /// than 8.64 × 10^15 milliseconds away, outside the range of a JavaScript `Date`.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "each field is reduced to its range before the cast"
    )]
    pub fn from_unix_ms(unix_ms: i64) -> Option<UtcDateTime> {
        if !(-MAX_DATE_MS..=MAX_DATE_MS).contains(&unix_ms) {
            return None;
        }
        let ms_of_day = unix_ms.rem_euclid(86_400_000);
        // Days from 1970-01-01 to the date, then the civil date (the era-based algorithm of the
        // proleptic Gregorian calendar, with years starting on 1 March).
        let shifted = unix_ms.div_euclid(86_400_000) + 719_468;
        let era = shifted.div_euclid(146_097);
        let day_of_era = shifted.rem_euclid(146_097);
        let year_of_era =
            (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
        let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
        let month_index = (5 * day_of_year + 2) / 153;
        let month = if month_index < 10 {
            month_index + 3
        } else {
            month_index - 9
        };
        Some(UtcDateTime {
            year: year_of_era + era * 400 + i64::from(month <= 2),
            month: month as u8,
            day: (day_of_year - (153 * month_index + 2) / 5 + 1) as u8,
            hour: (ms_of_day / 3_600_000) as u8,
            minute: (ms_of_day / 60_000 % 60) as u8,
            second: (ms_of_day / 1_000 % 60) as u8,
            millisecond: (ms_of_day % 1_000) as u16,
        })
    }

    /// The date and time as `Date.prototype.toISOString` writes it: `2026-01-01T00:00:08.000Z`,
    /// with a year outside 0 to 9999 as `+010000` or `-000001`.
    pub fn to_iso_string(&self) -> String {
        let year = if (0..=9_999).contains(&self.year) {
            format!("{:04}", self.year)
        } else {
            let sign = if self.year < 0 { '-' } else { '+' };
            format!("{sign}{:06}", self.year.unsigned_abs())
        };
        format!(
            "{year}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
            self.month, self.day, self.hour, self.minute, self.second, self.millisecond
        )
    }
}
