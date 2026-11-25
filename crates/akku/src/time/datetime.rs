//! Proleptic Gregorian calendar dates, wall-clock times, naive datetimes, and
//! UTC-offset timestamps with RFC 3339 formatting and parsing.

use std::cmp::Ordering;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::str::FromStr;
use std::time::SystemTime;

use super::duration::SignedDuration;
use super::timezone::{parse_two_digits, UtcOffset};
use super::{ParseError, TimeError};

const SECS_PER_DAY: i64 = 86_400;
const NANOS_PER_SEC: i128 = 1_000_000_000;

/// A day of the week in the Gregorian calendar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Weekday {
    /// Monday.
    Mon,
    /// Tuesday.
    Tue,
    /// Wednesday.
    Wed,
    /// Thursday.
    Thu,
    /// Friday.
    Fri,
    /// Saturday.
    Sat,
    /// Sunday.
    Sun,
}

impl Weekday {
    /// Returns the following day of the week.
    pub const fn next(self) -> Self {
        match self {
            Self::Mon => Self::Tue,
            Self::Tue => Self::Wed,
            Self::Wed => Self::Thu,
            Self::Thu => Self::Fri,
            Self::Fri => Self::Sat,
            Self::Sat => Self::Sun,
            Self::Sun => Self::Mon,
        }
    }

    /// Returns the preceding day of the week.
    pub const fn prev(self) -> Self {
        match self {
            Self::Mon => Self::Sun,
            Self::Tue => Self::Mon,
            Self::Wed => Self::Tue,
            Self::Thu => Self::Wed,
            Self::Fri => Self::Thu,
            Self::Sat => Self::Fri,
            Self::Sun => Self::Sat,
        }
    }

    /// Returns the ISO 8601 weekday number where Monday is `1` and Sunday is `7`.
    pub const fn number_from_monday(self) -> u8 {
        match self {
            Self::Mon => 1,
            Self::Tue => 2,
            Self::Wed => 3,
            Self::Thu => 4,
            Self::Fri => 5,
            Self::Sat => 6,
            Self::Sun => 7,
        }
    }

    /// Returns the weekday number where Sunday is `1` and Saturday is `7`.
    pub const fn number_from_sunday(self) -> u8 {
        match self {
            Self::Sun => 1,
            Self::Mon => 2,
            Self::Tue => 3,
            Self::Wed => 4,
            Self::Thu => 5,
            Self::Fri => 6,
            Self::Sat => 7,
        }
    }

    /// Computes the weekday from days since 1970-01-01 (which was a Thursday).
    const fn from_unix_days(days: i64) -> Self {
        // Compute (days + 3) mod 7 without risking i64 overflow.
        let idx = (days.rem_euclid(7) + 3) % 7;
        match idx {
            0 => Self::Mon,
            1 => Self::Tue,
            2 => Self::Wed,
            3 => Self::Thu,
            4 => Self::Fri,
            5 => Self::Sat,
            _ => Self::Sun,
        }
    }
}

impl fmt::Display for Weekday {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Mon => "Mon",
            Self::Tue => "Tue",
            Self::Wed => "Wed",
            Self::Thu => "Thu",
            Self::Fri => "Fri",
            Self::Sat => "Sat",
            Self::Sun => "Sun",
        };
        f.write_str(s)
    }
}

/// A calendar date in the proleptic Gregorian calendar (including negative years).
///
/// Conversions between civil `(year, month, day)` and Unix day counts use
/// Howard Hinnant's Euclidean civil-date algorithms, supporting negative years
/// without rounding discontinuities.
///
/// # Examples
///
/// ```
/// use akku::time::{Date, Weekday};
///
/// let date = Date::new(2024, 2, 29).unwrap();
/// assert!(Date::is_leap_year(2024));
/// assert_eq!(date.weekday(), Weekday::Thu);
/// assert_eq!(date.ordinal(), 60);
/// assert_eq!(date.to_string(), "2024-02-29");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Date {
    year: i32,
    month: u8,
    day: u8,
}

impl Date {
    /// Creates a new calendar `Date`, validating month and day bounds (including leap years).
    pub fn new(year: i32, month: u8, day: u8) -> Result<Self, TimeError> {
        if !(1..=12).contains(&month) {
            return Err(TimeError::InvalidDate { year, month, day });
        }
        let max_day = Self::days_in_month(year, month);
        if day < 1 || day > max_day {
            return Err(TimeError::InvalidDate { year, month, day });
        }
        Ok(Self { year, month, day })
    }

    /// Converts a signed count of days since `1970-01-01` into a `Date`.
    ///
    /// Uses Howard Hinnant's `civil_from_days` algorithm with Euclidean division.
    pub fn from_unix_days(days: i64) -> Result<Self, TimeError> {
        let z = i128::from(days) + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z.rem_euclid(146_097); // [0, 146_096]
        let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
        let y = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
        let mp = (5 * doy + 2) / 153; // [0, 11]
        let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
        let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
        let y = y + i128::from(m <= 2);
        let year = i32::try_from(y).map_err(|_| TimeError::OutOfRange)?;
        Ok(Self {
            year,
            month: m as u8,
            day: d as u8,
        })
    }

    /// Converts this `Date` to a signed count of days since `1970-01-01`.
    ///
    /// Uses Howard Hinnant's `days_from_civil` algorithm with Euclidean division.
    pub const fn to_unix_days(&self) -> i64 {
        let y = (self.year as i64) - if self.month <= 2 { 1 } else { 0 };
        let era = y.div_euclid(400);
        let yoe = y.rem_euclid(400); // [0, 399]
        let m_adj = (self.month as i64) + if self.month > 2 { -3 } else { 9 }; // [0, 11]
        let doy = (153 * m_adj + 2) / 5 + (self.day as i64) - 1; // [0, 365]
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146_096]
        era * 146_097 + doe - 719_468
    }

    /// Returns the astronomical calendar year (`0` = 1 BCE, `-1` = 2 BCE).
    pub const fn year(&self) -> i32 {
        self.year
    }

    /// Returns the calendar month (`1..=12`).
    pub const fn month(&self) -> u8 {
        self.month
    }

    /// Returns the day of the month (`1..=31`).
    pub const fn day(&self) -> u8 {
        self.day
    }

    /// Returns the day of the week for this date.
    pub const fn weekday(&self) -> Weekday {
        Weekday::from_unix_days(self.to_unix_days())
    }

    /// Returns `true` if `year` is a leap year in the proleptic Gregorian calendar.
    pub const fn is_leap_year(year: i32) -> bool {
        (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
    }

    /// Returns the number of days in `month` (`1..=12`) of `year`, or `0` for an invalid month.
    pub const fn days_in_month(year: i32, month: u8) -> u8 {
        match month {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            2 => {
                if Self::is_leap_year(year) {
                    29
                } else {
                    28
                }
            }
            _ => 0,
        }
    }

    /// Returns the 1-based ordinal day of the year (`1..=365`, or `1..=366` in leap years).
    pub const fn ordinal(&self) -> u16 {
        const CUMULATIVE: [u16; 12] = [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];
        let leap = if self.month > 2 && Self::is_leap_year(self.year) {
            1
        } else {
            0
        };
        CUMULATIVE[(self.month - 1) as usize] + leap + (self.day as u16)
    }

    /// Adds a signed number of days to this date, returning `None` on overflow.
    pub fn add_days(&self, days: i64) -> Option<Self> {
        let unix_days = self.to_unix_days().checked_add(days)?;
        Self::from_unix_days(unix_days).ok()
    }
}

impl fmt::Display for Date {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.year < 0 {
            write!(
                f,
                "-{:04}-{:02}-{:02}",
                self.year.unsigned_abs(),
                self.month,
                self.day
            )
        } else if self.year > 9999 {
            write!(f, "+{:04}-{:02}-{:02}", self.year, self.month, self.day)
        } else {
            write!(f, "{:04}-{:02}-{:02}", self.year, self.month, self.day)
        }
    }
}

impl FromStr for Date {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.is_empty() {
            return Err(ParseError::Empty);
        }
        if !s.is_ascii() {
            return Err(ParseError::InvalidFormat("expected YYYY-MM-DD"));
        }

        let (neg, rest) = if let Some(stripped) = s.strip_prefix('-') {
            (true, stripped)
        } else if let Some(stripped) = s.strip_prefix('+') {
            (false, stripped)
        } else {
            (false, s)
        };

        let mut parts = rest.split('-');
        let year_str = parts
            .next()
            .ok_or(ParseError::InvalidFormat("expected YYYY-MM-DD"))?;
        let month_str = parts
            .next()
            .ok_or(ParseError::InvalidFormat("expected YYYY-MM-DD"))?;
        let day_str = parts
            .next()
            .ok_or(ParseError::InvalidFormat("expected YYYY-MM-DD"))?;
        if parts.next().is_some() {
            return Err(ParseError::InvalidFormat("unexpected trailing segments"));
        }

        if year_str.len() < 4 || !year_str.bytes().all(|b| b.is_ascii_digit()) {
            return Err(ParseError::InvalidNumber("expected at least 4 year digits"));
        }
        let year_abs: i64 = year_str.parse().map_err(|_| ParseError::Overflow)?;
        let year_signed = if neg { -year_abs } else { year_abs };
        let year = i32::try_from(year_signed).map_err(|_| ParseError::Overflow)?;

        let month = parse_two_digits(month_str)?;
        let day = parse_two_digits(day_str)?;

        Self::new(year, month, day).map_err(|_| ParseError::OutOfRange("invalid calendar date"))
    }
}

/// A wall-clock time of day with nanosecond precision (`00:00:00`..=`23:59:59.999999999`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Time {
    hour: u8,
    minute: u8,
    second: u8,
    nanosecond: u32,
}

impl Time {
    /// Midnight (`00:00:00`).
    pub const MIDNIGHT: Self = Self {
        hour: 0,
        minute: 0,
        second: 0,
        nanosecond: 0,
    };

    /// Creates a new `Time`, validating component ranges.
    pub fn new(hour: u8, minute: u8, second: u8, nanosecond: u32) -> Result<Self, TimeError> {
        if hour > 23 || minute > 59 || second > 59 || nanosecond > 999_999_999 {
            return Err(TimeError::InvalidTime {
                hour,
                minute,
                second,
                nanosecond,
            });
        }
        Ok(Self {
            hour,
            minute,
            second,
            nanosecond,
        })
    }

    /// Creates a new `Time` with zero nanoseconds.
    pub fn from_hms(hour: u8, minute: u8, second: u8) -> Result<Self, TimeError> {
        Self::new(hour, minute, second, 0)
    }

    /// Returns the hour (`0..=23`).
    pub const fn hour(&self) -> u8 {
        self.hour
    }

    /// Returns the minute (`0..=59`).
    pub const fn minute(&self) -> u8 {
        self.minute
    }

    /// Returns the second (`0..=59`).
    pub const fn second(&self) -> u8 {
        self.second
    }

    /// Returns the nanosecond (`0..=999_999_999`).
    pub const fn nanosecond(&self) -> u32 {
        self.nanosecond
    }

    /// Returns the whole seconds elapsed since midnight (`0..=86_399`).
    pub const fn seconds_since_midnight(&self) -> u32 {
        (self.hour as u32) * 3_600 + (self.minute as u32) * 60 + (self.second as u32)
    }
}

impl fmt::Display for Time {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:02}:{:02}:{:02}", self.hour, self.minute, self.second)?;
        if self.nanosecond > 0 {
            let mut buf = [b'0'; 9];
            let mut n = self.nanosecond;
            for i in (0..9).rev() {
                buf[i] = b'0' + (n % 10) as u8;
                n /= 10;
            }
            let mut end = 9;
            while end > 0 && buf[end - 1] == b'0' {
                end -= 1;
            }
            let frac = std::str::from_utf8(&buf[..end]).expect("ASCII digits");
            write!(f, ".{frac}")?;
        }
        Ok(())
    }
}

impl FromStr for Time {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.is_empty() {
            return Err(ParseError::Empty);
        }
        if !s.is_ascii() {
            return Err(ParseError::InvalidFormat("expected HH:MM:SS[.fffffffff]"));
        }
        let b = s.as_bytes();
        if b.len() < 8 || b[2] != b':' || b[5] != b':' {
            return Err(ParseError::InvalidFormat("expected HH:MM:SS[.fffffffff]"));
        }
        let hour = parse_two_digits(&s[0..2])?;
        let minute = parse_two_digits(&s[3..5])?;
        let second = parse_two_digits(&s[6..8])?;

        let nanosecond = if b.len() == 8 {
            0
        } else if b[8] == b'.' {
            parse_fractional_nanos(&s[9..])?
        } else {
            return Err(ParseError::InvalidFormat("unexpected characters in time"));
        };

        Self::new(hour, minute, second, nanosecond)
            .map_err(|_| ParseError::OutOfRange("invalid wall-clock time"))
    }
}

fn parse_fractional_nanos(frac: &str) -> Result<u32, ParseError> {
    if frac.is_empty() || frac.len() > 9 {
        return Err(ParseError::InvalidFormat(
            "fractional seconds must have 1 to 9 digits",
        ));
    }
    let mut nanos: u32 = 0;
    let mut scale: u32 = 100_000_000;
    for byte in frac.bytes() {
        if !byte.is_ascii_digit() {
            return Err(ParseError::InvalidNumber("non-digit in fractional seconds"));
        }
        nanos += u32::from(byte - b'0') * scale;
        scale /= 10;
    }
    Ok(nanos)
}

/// A civil (timezone-naive) calendar date and wall-clock time.
///
/// Unix timestamp conversions follow the Euclidean floor-division convention for
/// negative pre-1970 timestamps: `-1` second with `0` nanoseconds corresponds to
/// `1969-12-31T23:59:59`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DateTime {
    date: Date,
    time: Time,
}

impl DateTime {
    /// Combines a [`Date`] and [`Time`] into a civil `DateTime`.
    pub const fn new(date: Date, time: Time) -> Self {
        Self { date, time }
    }

    /// Returns the date component.
    pub const fn date(&self) -> Date {
        self.date
    }

    /// Returns the time component.
    pub const fn time(&self) -> Time {
        self.time
    }

    /// Creates a `DateTime` from Unix seconds and subsecond nanoseconds (`0..1_000_000_000`).
    ///
    /// Negative timestamps use floor division (`div_euclid` / `rem_euclid`) so
    /// subsecond nanoseconds always count forward within the second.
    pub fn from_unix_timestamp(secs: i64, nanos: u32) -> Result<Self, TimeError> {
        if nanos >= 1_000_000_000 {
            return Err(TimeError::InvalidTime {
                hour: 0,
                minute: 0,
                second: 0,
                nanosecond: nanos,
            });
        }
        let days = secs.div_euclid(SECS_PER_DAY);
        let secs_of_day = secs.rem_euclid(SECS_PER_DAY) as u32;
        let date = Date::from_unix_days(days)?;
        let hour = (secs_of_day / 3_600) as u8;
        let minute = ((secs_of_day % 3_600) / 60) as u8;
        let second = (secs_of_day % 60) as u8;
        let time = Time::new(hour, minute, second, nanos)?;
        Ok(Self { date, time })
    }

    /// Returns the whole-second Unix timestamp (using floor convention for pre-1970 dates).
    pub const fn unix_timestamp(&self) -> i64 {
        self.date.to_unix_days() * SECS_PER_DAY + (self.time.seconds_since_midnight() as i64)
    }

    /// Returns the Unix timestamp in nanoseconds as `i128`.
    pub const fn unix_timestamp_nanos(&self) -> i128 {
        (self.unix_timestamp() as i128) * NANOS_PER_SEC + (self.time.nanosecond() as i128)
    }

    /// Adds a signed [`SignedDuration`] to this `DateTime`, returning `None` on overflow.
    pub fn checked_add(&self, duration: SignedDuration) -> Option<Self> {
        let total_nanos = self
            .unix_timestamp_nanos()
            .checked_add(duration.as_nanos())?;
        let secs = i64::try_from(total_nanos.div_euclid(NANOS_PER_SEC)).ok()?;
        let nanos = total_nanos.rem_euclid(NANOS_PER_SEC) as u32;
        Self::from_unix_timestamp(secs, nanos).ok()
    }

    /// Subtracts a signed [`SignedDuration`] from this `DateTime`, returning `None` on overflow.
    pub fn checked_sub(&self, duration: SignedDuration) -> Option<Self> {
        let total_nanos = self
            .unix_timestamp_nanos()
            .checked_sub(duration.as_nanos())?;
        let secs = i64::try_from(total_nanos.div_euclid(NANOS_PER_SEC)).ok()?;
        let nanos = total_nanos.rem_euclid(NANOS_PER_SEC) as u32;
        Self::from_unix_timestamp(secs, nanos).ok()
    }
}

impl fmt::Display for DateTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}T{}", self.date, self.time)
    }
}

/// A timestamp consisting of a local [`DateTime`] and a fixed [`UtcOffset`].
///
/// Equality ([`PartialEq`], [`Eq`]), ordering ([`PartialOrd`], [`Ord`]), and
/// hashing ([`Hash`]) compare the underlying **UTC instant**, so two
/// `OffsetDateTime` values representing the same moment in different offsets
/// compare equal and produce identical hashes.
///
/// # Examples
///
/// ```
/// use akku::time::{OffsetDateTime, UtcOffset};
///
/// let utc: OffsetDateTime = "2024-06-01T12:00:00Z".parse().unwrap();
/// let cest: OffsetDateTime = "2024-06-01T14:00:00+02:00".parse().unwrap();
/// assert_eq!(utc, cest);
/// assert_eq!(utc.to_offset(UtcOffset::from_hms(2, 0, 0).unwrap()).format_rfc3339(), "2024-06-01T14:00:00+02:00");
/// ```
#[derive(Debug, Clone, Copy)]
pub struct OffsetDateTime {
    datetime: DateTime,
    offset: UtcOffset,
}

impl OffsetDateTime {
    /// Creates an `OffsetDateTime` from a local civil [`DateTime`] and its [`UtcOffset`].
    pub const fn new(datetime: DateTime, offset: UtcOffset) -> Self {
        Self { datetime, offset }
    }

    /// Returns the current system wall-clock time in UTC.
    pub fn now_utc() -> Self {
        Self::try_from(SystemTime::now()).expect("system time within representable range")
    }

    /// Creates a UTC `OffsetDateTime` from Unix seconds and subsecond nanoseconds (`0..1_000_000_000`).
    pub fn from_unix_timestamp(secs: i64, nanos: u32) -> Result<Self, TimeError> {
        let datetime = DateTime::from_unix_timestamp(secs, nanos)?;
        Ok(Self {
            datetime,
            offset: UtcOffset::UTC,
        })
    }

    /// Returns the UTC Unix timestamp in whole seconds (floor convention for pre-1970).
    pub const fn unix_timestamp(&self) -> i64 {
        self.datetime.unix_timestamp() - (self.offset.whole_seconds() as i64)
    }

    /// Returns the UTC Unix timestamp in nanoseconds as `i128`.
    pub const fn unix_timestamp_nanos(&self) -> i128 {
        (self.unix_timestamp() as i128) * NANOS_PER_SEC
            + (self.datetime.time().nanosecond() as i128)
    }

    /// Converts this instant to the given [`UtcOffset`], preserving the exact UTC instant.
    ///
    /// Returns [`TimeError::OutOfRange`] if the shifted local date falls outside the
    /// representable `i32` year range.
    pub fn checked_to_offset(&self, offset: UtcOffset) -> Result<Self, TimeError> {
        let local_secs = self
            .unix_timestamp()
            .checked_add(i64::from(offset.whole_seconds()))
            .ok_or(TimeError::OutOfRange)?;
        let nanos = self.datetime.time().nanosecond();
        let datetime = DateTime::from_unix_timestamp(local_secs, nanos)?;
        Ok(Self { datetime, offset })
    }

    /// Converts this instant to the given [`UtcOffset`], preserving the exact UTC instant.
    ///
    /// # Panics
    ///
    /// Panics if the shifted local date falls outside the representable `i32` year range.
    /// Use [`checked_to_offset`](Self::checked_to_offset) for fallible conversion near year
    /// extremes.
    pub fn to_offset(&self, offset: UtcOffset) -> Self {
        self.checked_to_offset(offset)
            .expect("offset conversion within representable date range")
    }

    /// Converts this instant to UTC (`Z`).
    pub fn to_utc(&self) -> Self {
        self.to_offset(UtcOffset::UTC)
    }

    /// Returns the local calendar [`Date`] in this timestamp's offset.
    pub const fn date(&self) -> Date {
        self.datetime.date()
    }

    /// Returns the local wall-clock [`Time`] in this timestamp's offset.
    pub const fn time(&self) -> Time {
        self.datetime.time()
    }

    /// Returns the local civil [`DateTime`] in this timestamp's offset.
    pub const fn datetime(&self) -> DateTime {
        self.datetime
    }

    /// Returns the fixed [`UtcOffset`] attached to this timestamp.
    pub const fn offset(&self) -> UtcOffset {
        self.offset
    }

    /// Adds a signed [`SignedDuration`], preserving the current [`UtcOffset`].
    pub fn checked_add(&self, duration: SignedDuration) -> Option<Self> {
        self.datetime
            .checked_add(duration)
            .map(|dt| Self::new(dt, self.offset))
    }

    /// Subtracts a signed [`SignedDuration`], preserving the current [`UtcOffset`].
    pub fn checked_sub(&self, duration: SignedDuration) -> Option<Self> {
        self.datetime
            .checked_sub(duration)
            .map(|dt| Self::new(dt, self.offset))
    }

    /// Returns the signed [`SignedDuration`] elapsed from `earlier` to `self`.
    ///
    /// If `earlier` is later than `self`, the returned duration is negative.
    pub fn duration_since(&self, earlier: Self) -> SignedDuration {
        let diff = self.unix_timestamp_nanos() - earlier.unix_timestamp_nanos();
        SignedDuration::from_total_nanos(diff).expect("duration between timestamps overflowed")
    }

    /// Formats this timestamp according to RFC 3339 (for example, `"2024-06-01T12:00:00Z"`).
    pub fn format_rfc3339(&self) -> String {
        self.to_string()
    }

    /// Parses an RFC 3339 timestamp string.
    ///
    /// Supports `'T'`, `'t'`, or `' '` as the date-time separator, optional
    /// fractional seconds with 1 to 9 digits, `'Z'`/`'z'`, and `±HH:MM` offsets.
    pub fn parse_rfc3339(input: &str) -> Result<Self, ParseError> {
        if input.is_empty() {
            return Err(ParseError::Empty);
        }
        if !input.is_ascii() {
            return Err(ParseError::InvalidFormat(
                "RFC 3339 timestamp must be ASCII",
            ));
        }
        let b = input.as_bytes();
        // Minimum valid RFC 3339 length: "YYYY-MM-DDTHH:MM:SSZ" = 20 bytes.
        if b.len() < 20 {
            return Err(ParseError::InvalidFormat("RFC 3339 timestamp too short"));
        }

        // Strict 4-digit date part: YYYY-MM-DD.
        if b[4] != b'-' || b[7] != b'-' {
            return Err(ParseError::InvalidFormat("expected YYYY-MM-DD date prefix"));
        }
        if !b[0..4].iter().all(u8::is_ascii_digit) {
            return Err(ParseError::InvalidNumber("expected 4-digit year"));
        }
        let year: i32 = input[0..4]
            .parse()
            .map_err(|_| ParseError::InvalidNumber("invalid year"))?;
        let month = parse_two_digits(&input[5..7])?;
        let day = parse_two_digits(&input[8..10])?;
        let date = Date::new(year, month, day)
            .map_err(|_| ParseError::OutOfRange("invalid calendar date"))?;

        // Date/time separator: T, t, or space.
        if !matches!(b[10], b'T' | b't' | b' ') {
            return Err(ParseError::InvalidFormat(
                "expected 'T', 't', or space separator",
            ));
        }

        // Time part: HH:MM:SS.
        if b[13] != b':' || b[16] != b':' {
            return Err(ParseError::InvalidFormat("expected HH:MM:SS time"));
        }
        let hour = parse_two_digits(&input[11..13])?;
        let minute = parse_two_digits(&input[14..16])?;
        let second = parse_two_digits(&input[17..19])?;

        // Optional fractional seconds followed by offset.
        let mut idx = 19;
        let nanosecond = if idx < b.len() && b[idx] == b'.' {
            idx += 1;
            let frac_start = idx;
            while idx < b.len() && b[idx].is_ascii_digit() {
                idx += 1;
            }
            parse_fractional_nanos(&input[frac_start..idx])?
        } else {
            0
        };

        let time = Time::new(hour, minute, second, nanosecond)
            .map_err(|_| ParseError::OutOfRange("invalid wall-clock time"))?;

        // Offset: Z, z, or ±HH:MM (strict RFC 3339).
        let offset_str = &input[idx..];
        let offset = if offset_str == "Z" || offset_str == "z" {
            UtcOffset::UTC
        } else if offset_str.len() == 6
            && (offset_str.starts_with('+') || offset_str.starts_with('-'))
        {
            offset_str.parse::<UtcOffset>()?
        } else {
            return Err(ParseError::InvalidFormat(
                "expected RFC 3339 offset ('Z', 'z', or ±HH:MM)",
            ));
        };

        Ok(Self::new(DateTime::new(date, time), offset))
    }
}

impl TryFrom<SystemTime> for OffsetDateTime {
    type Error = TimeError;

    fn try_from(value: SystemTime) -> Result<Self, Self::Error> {
        match value.duration_since(SystemTime::UNIX_EPOCH) {
            Ok(dur) => {
                let secs = i64::try_from(dur.as_secs()).map_err(|_| TimeError::OutOfRange)?;
                Self::from_unix_timestamp(secs, dur.subsec_nanos())
            }
            Err(err) => {
                let dur = err.duration();
                let total_nanos =
                    -(i128::from(dur.as_secs()) * NANOS_PER_SEC + i128::from(dur.subsec_nanos()));
                let secs = i64::try_from(total_nanos.div_euclid(NANOS_PER_SEC))
                    .map_err(|_| TimeError::OutOfRange)?;
                let nanos = total_nanos.rem_euclid(NANOS_PER_SEC) as u32;
                Self::from_unix_timestamp(secs, nanos)
            }
        }
    }
}

impl TryFrom<OffsetDateTime> for SystemTime {
    type Error = TimeError;

    fn try_from(value: OffsetDateTime) -> Result<Self, Self::Error> {
        let secs = value.unix_timestamp();
        let nanos = value.time().nanosecond();
        if secs >= 0 {
            let dur = std::time::Duration::new(secs as u64, nanos);
            SystemTime::UNIX_EPOCH
                .checked_add(dur)
                .ok_or(TimeError::OutOfRange)
        } else {
            let abs_nanos = value.unix_timestamp_nanos().unsigned_abs();
            let dur_secs = u64::try_from(abs_nanos / (NANOS_PER_SEC as u128))
                .map_err(|_| TimeError::OutOfRange)?;
            let dur_nanos = (abs_nanos % (NANOS_PER_SEC as u128)) as u32;
            let dur = std::time::Duration::new(dur_secs, dur_nanos);
            SystemTime::UNIX_EPOCH
                .checked_sub(dur)
                .ok_or(TimeError::OutOfRange)
        }
    }
}

impl PartialEq for OffsetDateTime {
    fn eq(&self, other: &Self) -> bool {
        self.unix_timestamp_nanos() == other.unix_timestamp_nanos()
    }
}

impl Eq for OffsetDateTime {}

impl PartialOrd for OffsetDateTime {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for OffsetDateTime {
    fn cmp(&self, other: &Self) -> Ordering {
        self.unix_timestamp_nanos()
            .cmp(&other.unix_timestamp_nanos())
    }
}

impl Hash for OffsetDateTime {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.unix_timestamp_nanos().hash(state);
    }
}

impl fmt::Display for OffsetDateTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", self.datetime, self.offset)
    }
}

impl FromStr for OffsetDateTime {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse_rfc3339(s)
    }
}
