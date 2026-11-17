// SPDX-License-Identifier: MPL-2.0
//! Convert Discord timestamps to the viewer's Windows time zone at display time.
//! The original timestamp remains unchanged for message ordering and transport.

const TICKS_PER_SECOND: u64 = 10_000_000;

/// Parse the RFC 3339 forms used by Discord into UTC FILETIME ticks. Fractional
/// seconds are validated but discarded: the UI displays only hours and minutes.
fn timestamp_filetime(timestamp: &str) -> Option<u64> {
    let bytes = timestamp.as_bytes();
    if !(20..=64).contains(&bytes.len())
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || !matches!(bytes[10], b'T' | b't')
        || bytes[13] != b':'
        || bytes[16] != b':'
    {
        return None;
    }
    let year = decimal(bytes.get(0..4)?)?;
    let month = decimal(bytes.get(5..7)?)?;
    let day = decimal(bytes.get(8..10)?)?;
    let hour = decimal(bytes.get(11..13)?)?;
    let minute = decimal(bytes.get(14..16)?)?;
    let second = decimal(bytes.get(17..19)?)?;
    if !(1601..=9999).contains(&year)
        || !(1..=12).contains(&month)
        || hour > 23
        || minute > 59
        || second > 59
    {
        return None;
    }
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let month_days = [
        31,
        28 + u32::from(leap),
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    if day == 0 || day > month_days[(month - 1) as usize] {
        return None;
    }
    let mut end = 19;
    if bytes.get(end) == Some(&b'.') {
        end += 1;
        let fraction_start = end;
        while bytes.get(end).is_some_and(u8::is_ascii_digit) {
            end += 1;
        }
        if end == fraction_start {
            return None;
        }
    }
    let offset_seconds = match bytes.get(end..)? {
        [b'Z' | b'z'] => 0_i64,
        [sign @ (b'+' | b'-'), h1, h2, b':', m1, m2] => {
            let offset_hour = decimal(&[*h1, *h2])?;
            let offset_minute = decimal(&[*m1, *m2])?;
            if offset_hour > 23 || offset_minute > 59 {
                return None;
            }
            let seconds = i64::from(offset_hour * 3600 + offset_minute * 60);
            if *sign == b'+' {
                seconds
            } else {
                -seconds
            }
        }
        _ => return None,
    };

    // Gregorian day count from FILETIME's 1601-01-01 epoch. The input bounds
    // keep every intermediate well inside i64, including a day/year rollover.
    let previous_year = i64::from(year - 1);
    let days_before_year = 365 * i64::from(year - 1601) + (previous_year / 4 - 1600 / 4)
        - (previous_year / 100 - 1600 / 100)
        + (previous_year / 400 - 1600 / 400);
    let days_before_month: u32 = month_days[..(month - 1) as usize].iter().sum();
    let days = days_before_year + i64::from(days_before_month + day - 1);
    let seconds = days * 86400 + i64::from(hour * 3600 + minute * 60 + second) - offset_seconds;
    u64::try_from(seconds).ok()?.checked_mul(TICKS_PER_SECOND)
}

fn decimal(bytes: &[u8]) -> Option<u32> {
    bytes.iter().try_fold(0_u32, |value, digit| {
        digit
            .is_ascii_digit()
            .then(|| value * 10 + u32::from(digit - b'0'))
    })
}

#[cfg(windows)]
use windows_sys::Win32::{
    Foundation::{FILETIME, SYSTEMTIME},
    System::Time::{
        FileTimeToSystemTime, GetDynamicTimeZoneInformation, SystemTimeToTzSpecificLocalTimeEx,
        DYNAMIC_TIME_ZONE_INFORMATION,
    },
};

/// Snapshot the Windows zone once per chat layout, not once per message. A new
/// layout refreshes it so a Windows time-zone change is not cached indefinitely.
#[cfg(windows)]
pub(crate) struct LocalTimeFormatter {
    zone: Option<DYNAMIC_TIME_ZONE_INFORMATION>,
}

#[cfg(windows)]
impl LocalTimeFormatter {
    pub(crate) fn current() -> Self {
        let mut zone = unsafe { std::mem::zeroed() };
        let valid = unsafe { GetDynamicTimeZoneInformation(&mut zone) } != u32::MAX;
        Self {
            zone: valid.then_some(zone),
        }
    }

    pub(crate) fn format(&self, timestamp: &str) -> String {
        self.try_format(timestamp).unwrap_or_default()
    }

    fn try_format(&self, timestamp: &str) -> Option<String> {
        let zone = self.zone.as_ref()?;
        let ticks = timestamp_filetime(timestamp)?;
        let filetime = FILETIME {
            dwLowDateTime: ticks as u32,
            dwHighDateTime: (ticks >> 32) as u32,
        };
        let mut utc: SYSTEMTIME = unsafe { std::mem::zeroed() };
        let mut local: SYSTEMTIME = unsafe { std::mem::zeroed() };
        // Ex uses the message date and the system's dynamic DST rules, rather
        // than adding the offset currently in effect to every historical row.
        if unsafe { FileTimeToSystemTime(&filetime, &mut utc) } == 0
            || unsafe { SystemTimeToTzSpecificLocalTimeEx(zone, &utc, &mut local) } == 0
        {
            // Omit an unconvertible time instead of presenting UTC as local.
            return None;
        }
        Some(format!("{:02}:{:02}", local.wHour, local.wMinute))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equivalent_discord_offsets_identify_the_same_instant() {
        let expected = Some(134_394_046_200_000_000);
        for input in [
            "2026-11-17T15:57:00Z",
            "2026-11-17T15:57:00.000000+00:00",
            "2026-11-18T00:57:00+09:00",
            "2026-11-17T07:57:00-08:00",
            "2026-11-17T21:27:00+05:30",
            "2026-11-17t15:57:00.123456789z",
        ] {
            assert_eq!(timestamp_filetime(input), expected, "{input}");
        }
    }

    #[test]
    fn timestamp_offsets_cross_dates_and_years() {
        for (input, expected) in [
            ("2026-01-01T00:10:00+09:00", 134_116_674_000_000_000),
            ("2026-12-31T23:45:00-03:30", 134_432_469_000_000_000),
            ("2000-02-29T23:59:59Z", 125_963_423_990_000_000),
            ("1970-01-01T00:00:00Z", 116_444_736_000_000_000),
            ("1601-01-01T00:00:00Z", 0),
        ] {
            assert_eq!(timestamp_filetime(input), Some(expected), "{input}");
        }
    }

    #[test]
    fn malformed_or_ambiguous_timestamps_have_no_display_time() {
        for input in [
            "",
            "2026-11-17T15:57:00",
            "2026-11-17 15:57:00Z",
            "2026-11-17T15:57:00.Z",
            "2026-11-17T15:57:00Zjunk",
            "2026-11-17T15:57:00+24:00",
            "2026-11-17T15:57:00+09:60",
            "2026-11-17T15:57:00+0900",
            "2026-11-17T15:57:00+ab:cd",
            "2026-11-17T24:00:00Z",
            "2026-11-17T15:60:00Z",
            "2026-11-17T15:57:60Z",
            "2026-00-17T15:57:00Z",
            "2026-13-17T15:57:00Z",
            "2026-11-00T15:57:00Z",
            "2026-04-31T15:57:00Z",
            "1900-02-29T15:57:00Z",
            "2100-02-29T15:57:00Z",
            "2026-02-29T15:57:00Z",
            "1600-12-31T23:59:59Z",
            "1601-01-01T00:00:00+01:00",
            "２０２６-11-17T15:57:00Z",
            "2026-11-17T한글:00Z",
        ] {
            assert_eq!(timestamp_filetime(input), None, "{input}");
        }
        let valid = "2026-11-17T15:57:00.000000+00:00";
        for end in 0..valid.len() {
            assert_eq!(timestamp_filetime(&valid[..end]), None, "prefix {end}");
        }
        assert_eq!(timestamp_filetime(&format!("{}Z", "0".repeat(64))), None);
    }

    #[cfg(windows)]
    #[test]
    fn windows_formats_message_in_the_selected_zone_across_midnight() {
        let mut zone: DYNAMIC_TIME_ZONE_INFORMATION = unsafe { std::mem::zeroed() };
        zone.Bias = -9 * 60;
        let clock = LocalTimeFormatter { zone: Some(zone) };
        assert_eq!(clock.format("2026-11-17T15:57:00.000000+00:00"), "00:57");
        assert_eq!(clock.format("2026-11-18T00:57:00+09:00"), "00:57");
        assert_eq!(clock.format("invalid"), "");
        let missing = LocalTimeFormatter { zone: None };
        assert_eq!(missing.format("2026-11-17T15:57:00Z"), "");
    }
}
