//! Fixed UTC offsets and lightweight time zone identifiers.

use std::fmt;
use std::str::FromStr;

use super::{ParseError, TimeError};

const MAX_OFFSET_SECONDS: i32 = 86_399; // 23:59:59

/// A fixed offset from UTC, accurate to the second.
///
/// Valid offsets lie in the range `-23:59:59..=+23:59:59`.
///
/// # Examples
///
/// ```
/// use akku::time::UtcOffset;
///
/// let berlin = UtcOffset::from_hms(2, 0, 0).unwrap();
/// assert_eq!(berlin.to_string(), "+02:00");
/// assert_eq!(berlin.whole_seconds(), 7200);
///
/// let parsed: UtcOffset = "-05:30".parse().unwrap();
/// assert_eq!(parsed.hours(), -5);
/// assert_eq!(parsed.minutes(), -30);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct UtcOffset {
    seconds: i32,
}

impl UtcOffset {
    /// The zero offset representing Coordinated Universal Time (`Z`).
    pub const UTC: Self = Self { seconds: 0 };

    /// Creates a `UtcOffset` from signed hours, minutes, and seconds.
    ///
    /// All non-zero components must share the same sign, and their magnitudes
    /// must satisfy `|hours| <= 23`, `|minutes| <= 59`, `|seconds| <= 59`.
    /// For example, `-05:30` is constructed with `UtcOffset::from_hms(-5, -30, 0)`.
    pub fn from_hms(hours: i8, minutes: i8, seconds: i8) -> Result<Self, TimeError> {
        if hours.unsigned_abs() > 23 || minutes.unsigned_abs() > 59 || seconds.unsigned_abs() > 59 {
            return Err(TimeError::InvalidOffset);
        }

        let has_pos = hours > 0 || minutes > 0 || seconds > 0;
        let has_neg = hours < 0 || minutes < 0 || seconds < 0;
        if has_pos && has_neg {
            return Err(TimeError::InvalidOffset);
        }

        let total = i32::from(hours) * 3_600 + i32::from(minutes) * 60 + i32::from(seconds);
        Self::from_whole_seconds(total)
    }

    /// Creates a `UtcOffset` from a total number of seconds east of UTC.
    ///
    /// Returns [`TimeError::InvalidOffset`] if `|seconds| > 86_399`.
    pub fn from_whole_seconds(seconds: i32) -> Result<Self, TimeError> {
        if !(-MAX_OFFSET_SECONDS..=MAX_OFFSET_SECONDS).contains(&seconds) {
            return Err(TimeError::InvalidOffset);
        }
        Ok(Self { seconds })
    }

    /// Returns the total offset in seconds east of UTC.
    pub const fn whole_seconds(self) -> i32 {
        self.seconds
    }

    /// Returns the signed whole-hours component (`-23..=23`).
    pub const fn hours(self) -> i8 {
        (self.seconds / 3_600) as i8
    }

    /// Returns the signed minutes component (`-59..=59`).
    pub const fn minutes(self) -> i8 {
        ((self.seconds % 3_600) / 60) as i8
    }

    /// Returns the signed sub-minute seconds component (`-59..=59`).
    pub const fn seconds(self) -> i8 {
        (self.seconds % 60) as i8
    }

    /// Returns `true` if this offset is UTC (`+00:00` / `Z`).
    pub const fn is_utc(self) -> bool {
        self.seconds == 0
    }
}

impl fmt::Display for UtcOffset {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.seconds == 0 {
            return f.write_str("Z");
        }
        let sign = if self.seconds < 0 { '-' } else { '+' };
        let abs = self.seconds.unsigned_abs();
        let hours = abs / 3_600;
        let minutes = (abs % 3_600) / 60;
        let secs = abs % 60;
        if secs == 0 {
            write!(f, "{sign}{hours:02}:{minutes:02}")
        } else {
            write!(f, "{sign}{hours:02}:{minutes:02}:{secs:02}")
        }
    }
}

impl FromStr for UtcOffset {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.is_empty() {
            return Err(ParseError::Empty);
        }
        if !s.is_ascii() {
            return Err(ParseError::InvalidFormat("expected ±HH:MM or ±HH:MM:SS"));
        }
        if s == "Z" || s == "z" {
            return Ok(Self::UTC);
        }

        let bytes = s.as_bytes();
        let sign: i32 = match bytes[0] {
            b'+' => 1,
            b'-' => -1,
            _ => return Err(ParseError::InvalidFormat("expected 'Z', '+', or '-'")),
        };

        let rest = &s[1..];
        let (hh, mm, ss) = match rest.len() {
            5 => {
                if rest.as_bytes()[2] != b':' {
                    return Err(ParseError::InvalidFormat("expected ±HH:MM"));
                }
                let hh = parse_two_digits(&rest[0..2])?;
                let mm = parse_two_digits(&rest[3..5])?;
                (hh, mm, 0u8)
            }
            8 => {
                if rest.as_bytes()[2] != b':' || rest.as_bytes()[5] != b':' {
                    return Err(ParseError::InvalidFormat("expected ±HH:MM:SS"));
                }
                let hh = parse_two_digits(&rest[0..2])?;
                let mm = parse_two_digits(&rest[3..5])?;
                let ss = parse_two_digits(&rest[6..8])?;
                (hh, mm, ss)
            }
            _ => return Err(ParseError::InvalidFormat("expected ±HH:MM or ±HH:MM:SS")),
        };

        if hh > 23 || mm > 59 || ss > 59 {
            return Err(ParseError::OutOfRange("UTC offset component out of range"));
        }

        let total = sign * (i32::from(hh) * 3_600 + i32::from(mm) * 60 + i32::from(ss));
        Self::from_whole_seconds(total).map_err(|_| ParseError::OutOfRange("UTC offset"))
    }
}

pub(crate) fn parse_two_digits(s: &str) -> Result<u8, ParseError> {
    let b = s.as_bytes();
    if b.len() != 2 || !b[0].is_ascii_digit() || !b[1].is_ascii_digit() {
        return Err(ParseError::InvalidNumber("expected two ASCII digits"));
    }
    Ok((b[0] - b'0') * 10 + (b[1] - b'0'))
}

/// A time zone representation supporting UTC and fixed offsets.
///
/// Parsing accepts `"UTC"`, `"Z"`, `"±HH:MM"`, and IANA `"Etc/GMT±N"` identifiers.
/// Following the POSIX / IANA `Etc/GMT` convention, signs in `Etc/GMT±N` are
/// inverted relative to ISO 8601: `"Etc/GMT+5"` corresponds to `UTC-05:00` and
/// `"Etc/GMT-9"` corresponds to `UTC+09:00`.
///
/// # Examples
///
/// ```
/// use akku::time::{TimeZone, UtcOffset};
///
/// assert_eq!("UTC".parse::<TimeZone>().unwrap(), TimeZone::Utc);
/// assert_eq!(
///     "+02:00".parse::<TimeZone>().unwrap(),
///     TimeZone::Fixed(UtcOffset::from_hms(2, 0, 0).unwrap())
/// );
/// assert_eq!(
///     "Etc/GMT+5".parse::<TimeZone>().unwrap().offset(),
///     UtcOffset::from_hms(-5, 0, 0).unwrap()
/// );
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum TimeZone {
    /// Coordinated Universal Time.
    #[default]
    Utc,
    /// A fixed offset from UTC.
    Fixed(UtcOffset),
}

impl TimeZone {
    /// Returns the [`UtcOffset`] associated with this time zone.
    pub const fn offset(&self) -> UtcOffset {
        match self {
            Self::Utc => UtcOffset::UTC,
            Self::Fixed(offset) => *offset,
        }
    }

    /// Parses a time zone identifier (`"UTC"`, `"Z"`, `"±HH:MM"`, or `"Etc/GMT±N"`).
    pub fn parse(input: &str) -> Result<Self, ParseError> {
        input.parse()
    }
}

impl fmt::Display for TimeZone {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Utc => f.write_str("UTC"),
            Self::Fixed(offset) => offset.fmt(f),
        }
    }
}

impl FromStr for TimeZone {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.is_empty() {
            return Err(ParseError::Empty);
        }

        if s.eq_ignore_ascii_case("UTC")
            || s.eq_ignore_ascii_case("Z")
            || s == "Etc/UTC"
            || s == "Etc/GMT"
            || s == "GMT"
        {
            return Ok(Self::Utc);
        }

        if let Some(rest) = s.strip_prefix("Etc/GMT") {
            let bytes = rest.as_bytes();
            if bytes.len() < 2 {
                return Err(ParseError::InvalidFormat("expected Etc/GMT±N"));
            }
            // IANA Etc/GMT uses POSIX sign convention: + is west (negative UTC offset),
            // - is east (positive UTC offset).
            let sign: i8 = match bytes[0] {
                b'+' => -1,
                b'-' => 1,
                _ => {
                    return Err(ParseError::InvalidFormat(
                        "expected '+' or '-' in Etc/GMT±N",
                    ))
                }
            };
            let digits = &rest[1..];
            if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
                return Err(ParseError::InvalidNumber("invalid hour in Etc/GMT±N"));
            }
            let hours: u8 = digits
                .parse()
                .map_err(|_| ParseError::OutOfRange("Etc/GMT hour out of range"))?;
            // IANA tzdb defines Etc/GMT+0..=12 and Etc/GMT-0..=14.
            let max_hours = if sign < 0 { 12 } else { 14 };
            if hours > max_hours {
                return Err(ParseError::OutOfRange("Etc/GMT offset out of IANA range"));
            }
            if hours == 0 {
                return Ok(Self::Utc);
            }
            let offset = UtcOffset::from_hms(sign * (hours as i8), 0, 0)
                .map_err(|_| ParseError::OutOfRange("Etc/GMT offset"))?;
            return Ok(Self::Fixed(offset));
        }

        if s.starts_with('+') || s.starts_with('-') {
            let offset: UtcOffset = s.parse()?;
            return Ok(Self::Fixed(offset));
        }

        Err(ParseError::UnknownUnit("unrecognized time zone"))
    }
}
