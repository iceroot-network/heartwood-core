//! Slot arithmetic with one block time from height 1.
//!
//! Times are whole seconds since the network epoch, as the reference's `Slots.getTime` gives them;
//! they can be negative before the epoch, and every division rounds toward negative infinity, as
//! `Math.floor` does.
//!
//! The block time is fixed from height 1 (the milestone loader refuses any change), so there is a
//! single span: slot `s` starts at `s × blockTime` seconds. A block time of 0 is treated as 1, so
//! no function here divides by zero; the loader never yields one.

/// The slot that a timestamp falls in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlotInfo {
    /// First second of the slot.
    pub start_time: i64,
    /// Last second of the slot.
    pub end_time: i64,
    /// The block time, in seconds.
    pub block_time: u64,
    /// The slot number.
    pub slot_number: i64,
    /// Whether the timestamp is in the first half of the slot, where forging is allowed.
    pub forging_status: bool,
}

/// Milliseconds since the Unix epoch of the milestone `epoch` text, or `None` if the text is not
/// a date and time this function reads the same way on every machine.
///
/// The accepted form is the ECMAScript date-time format with a time zone:
/// `YYYY-MM-DDTHH:mm`, optionally followed by `:ss` and a fraction of 1 to 9 digits (read to the
/// millisecond), then `Z` or an offset `±HH:mm`. For example `2026-01-01T00:00:00.000Z`. The
/// reference reads a text without a zone in the machine's local time, so such texts are refused.
pub fn epoch_unix_ms(epoch: &str) -> Option<i64> {
    let bytes = epoch.as_bytes();
    let mut cursor = Cursor { bytes, at: 0 };
    let year = cursor.digits(4)?;
    cursor.expect(b'-')?;
    let month = cursor.digits(2)?;
    cursor.expect(b'-')?;
    let day = cursor.digits(2)?;
    cursor.expect(b'T')?;
    let hour = cursor.digits(2)?;
    cursor.expect(b':')?;
    let minute = cursor.digits(2)?;
    let mut second = 0;
    let mut millisecond = 0;
    if cursor.peek() == Some(b':') {
        cursor.expect(b':')?;
        second = cursor.digits(2)?;
        if cursor.peek() == Some(b'.') {
            cursor.expect(b'.')?;
            let start = cursor.at;
            while cursor.peek().is_some_and(|byte| byte.is_ascii_digit()) {
                cursor.at += 1;
            }
            let fraction = bytes.get(start..cursor.at)?;
            if fraction.is_empty() || fraction.len() > 9 {
                return None;
            }
            millisecond = fraction
                .iter()
                .chain(b"00")
                .take(3)
                .fold(0, |total, digit| total * 10 + i64::from(digit - b'0'));
        }
    }
    let offset_minutes = match cursor.next()? {
        b'Z' => 0,
        sign @ (b'+' | b'-') => {
            let hours = cursor.digits(2)?;
            cursor.expect(b':')?;
            let minutes = cursor.digits(2)?;
            if hours > 23 || minutes > 59 {
                return None;
            }
            let offset = hours * 60 + minutes;
            if sign == b'+' { offset } else { -offset }
        }
        _ => return None,
    };
    if cursor.at != bytes.len() {
        return None;
    }
    if !(1..=12).contains(&month)
        || day < 1
        || day > days_in_month(year, month)
        || hour > 23
        || minute > 59
        || second > 59
    {
        return None;
    }
    let days = days_from_civil(year, month, day);
    let seconds = days * 86_400 + hour * 3_600 + minute * 60 + second - offset_minutes * 60;
    Some(seconds * 1_000 + millisecond)
}

/// Seconds since the network epoch at `now_unix_ms`: `floor((now − epoch) / 1000)`.
pub fn epoch_time(now_unix_ms: i64, epoch_unix_ms: i64) -> i64 {
    let elapsed = i128::from(now_unix_ms) - i128::from(epoch_unix_ms);
    saturate(elapsed.div_euclid(1_000))
}

/// The slot of `timestamp` with `block_time`-second slots.
pub fn slot_info(timestamp: i64, block_time: u64) -> SlotInfo {
    let block_time = block_time.max(1);
    let span = i128::from(block_time);
    let slot = i128::from(timestamp).div_euclid(span);
    let start = slot * span;
    SlotInfo {
        start_time: saturate(start),
        end_time: saturate(start + span - 1),
        block_time,
        slot_number: saturate(slot),
        forging_status: i128::from(timestamp) < start + span / 2,
    }
}

/// The slot number of `timestamp`.
pub fn slot_number(timestamp: i64, block_time: u64) -> i64 {
    slot_info(timestamp, block_time).slot_number
}

/// The first second of `slot`.
pub fn slot_time(slot: i64, block_time: u64) -> i64 {
    saturate(i128::from(slot) * i128::from(block_time.max(1)))
}

/// Whether forging is allowed at `timestamp`: it is in the first half of its slot.
pub fn is_forging_allowed(timestamp: i64, block_time: u64) -> bool {
    slot_info(timestamp, block_time).forging_status
}

fn saturate(value: i128) -> i64 {
    i64::try_from(value).unwrap_or(if value < 0 { i64::MIN } else { i64::MAX })
}

fn is_leap_year(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        2 if is_leap_year(year) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Days from 1970-01-01 to the given date of the proleptic Gregorian calendar.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month_index = (month + 9) % 12;
    let day_of_year = (153 * month_index + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// A cursor over the epoch text.
struct Cursor<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Cursor<'_> {
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.at).copied()
    }

    fn next(&mut self) -> Option<u8> {
        let byte = self.peek()?;
        self.at += 1;
        Some(byte)
    }

    fn expect(&mut self, wanted: u8) -> Option<()> {
        (self.next()? == wanted).then_some(())
    }

    fn digits(&mut self, count: usize) -> Option<i64> {
        let mut value = 0;
        for _ in 0..count {
            let byte = self.next()?;
            if !byte.is_ascii_digit() {
                return None;
            }
            value = value * 10 + i64::from(byte - b'0');
        }
        Some(value)
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    #[test]
    fn epochs() {
        assert_eq!(epoch_unix_ms("1970-01-01T00:00:00.000Z"), Some(0));
        assert_eq!(
            epoch_unix_ms("2026-01-01T00:00:00.000Z"),
            Some(1_767_225_600_000)
        );
        assert_eq!(
            epoch_unix_ms("2022-03-28T18:00:00.000Z"),
            Some(1_648_490_400_000)
        );
        assert_eq!(epoch_unix_ms("2026-01-01T00:00Z"), Some(1_767_225_600_000));
        assert_eq!(
            epoch_unix_ms("2026-01-01T00:00:00.5Z"),
            Some(1_767_225_600_500)
        );
        assert_eq!(
            epoch_unix_ms("2026-01-01T00:00:00.123456Z"),
            Some(1_767_225_600_123)
        );
        assert_eq!(
            epoch_unix_ms("2026-01-01T02:00:00+02:00"),
            Some(1_767_225_600_000)
        );
        assert_eq!(
            epoch_unix_ms("2025-12-31T23:00:00-01:00"),
            Some(1_767_225_600_000)
        );
        assert_eq!(
            epoch_unix_ms("2024-02-29T00:00:00Z"),
            Some(1_709_164_800_000)
        );
        assert_eq!(epoch_unix_ms("1969-12-31T23:59:59.999Z"), Some(-1));
    }

    #[test]
    fn epochs_refused() {
        for text in [
            "",
            "2026-01-01",
            "2026-01-01T00:00:00",
            "2026-01-01T00:00:00.000",
            "2026-01-01 00:00:00Z",
            "2026-01-01T00:00:00.Z",
            "2026-01-01T00:00:00.0000000000Z",
            "2026-13-01T00:00:00Z",
            "2026-02-29T00:00:00Z",
            "2026-01-32T00:00:00Z",
            "2026-01-01T24:00:00Z",
            "2026-01-01T00:60:00Z",
            "2026-01-01T00:00:60Z",
            "2026-01-01T00:00:00+24:00",
            "2026-01-01T00:00:00zz",
            "2026-01-01T00:00:00Zx",
            "+002026-01-01T00:00:00Z",
        ] {
            assert_eq!(epoch_unix_ms(text), None, "{text}");
        }
    }

    #[test]
    fn epoch_time_rounds_down() {
        assert_eq!(epoch_time(1_999, 0), 1);
        assert_eq!(epoch_time(-1, 0), -1);
        assert_eq!(epoch_time(-1_000, 0), -1);
        assert_eq!(epoch_time(-1_001, 0), -2);
        assert_eq!(epoch_time(i64::MAX, i64::MIN), 18_446_744_073_709_551);
    }

    #[test]
    fn slots_with_eight_second_blocks() {
        assert_eq!(
            slot_info(0, 8),
            SlotInfo {
                start_time: 0,
                end_time: 7,
                block_time: 8,
                slot_number: 0,
                forging_status: true,
            }
        );
        let info = slot_info(13, 8);
        assert_eq!(
            (info.slot_number, info.start_time, info.end_time),
            (1, 8, 15)
        );
        assert!(!info.forging_status);
        assert!(is_forging_allowed(11, 8));
        assert!(!is_forging_allowed(12, 8));
        let before = slot_info(-1, 8);
        assert_eq!(
            (before.slot_number, before.start_time, before.end_time),
            (-1, -8, -1)
        );
        assert!(!before.forging_status);
        assert_eq!(slot_number(-9, 8), -2);
        assert_eq!(slot_time(3, 8), 24);
        assert_eq!(slot_time(-2, 8), -16);
    }

    #[test]
    fn odd_and_degenerate_block_times() {
        // floor(9 / 2) = 4 seconds of forging per 9-second slot.
        assert!(is_forging_allowed(3, 9));
        assert!(!is_forging_allowed(4, 9));
        assert_eq!(slot_info(5, 0), slot_info(5, 1));
        assert_eq!(slot_time(i64::MAX, u64::MAX), i64::MAX);
        assert_eq!(slot_info(i64::MIN, u64::MAX).slot_number, -1);
    }

    proptest! {
        #[test]
        fn a_timestamp_is_inside_its_slot(timestamp in -1_000_000_000_i64..1_000_000_000, block_time in 1_u64..1_000) {
            let info = slot_info(timestamp, block_time);
            prop_assert!(info.start_time <= timestamp && timestamp <= info.end_time);
            prop_assert_eq!(info.end_time - info.start_time + 1, block_time as i64);
            prop_assert_eq!(slot_time(info.slot_number, block_time), info.start_time);
        }
    }
}
