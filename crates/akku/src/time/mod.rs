//! Pure-Rust date, time, duration, time zone, and clock primitives, plus the
//! full `std::time` surface.
//!
//! `akku::time` is a `std::time` superset: it re-exports [`Duration`],
//! [`Instant`], [`SystemTime`], [`SystemTimeError`],
//! [`TryFromFloatSecsError`], and [`UNIX_EPOCH`] unchanged, and adds:
//! - Signed nanosecond-precision [`SignedDuration`] values (named after
//!   [`jiff::SignedDuration`](https://docs.rs/jiff/latest/jiff/struct.SignedDuration.html)).
//! - Proleptic Gregorian [`Date`] / [`Time`] / [`DateTime`] /
//!   [`OffsetDateTime`] types with RFC 3339 parsing and formatting.
//! - Fixed [`UtcOffset`] / [`TimeZone`] support.
//! - An injectable [`Clock`] trait producing [`ClockInstant`] readings, with
//!   [`SystemClock`] and [`MockClock`] for deterministic testing.
//!
//! # Examples
//!
//! ```
//! use akku::time::{Clock, MockClock, OffsetDateTime, SignedDuration, UtcOffset};
//!
//! let start: OffsetDateTime = "2024-06-01T12:00:00Z".parse().unwrap();
//! let clock = MockClock::new(start);
//!
//! let t0 = clock.monotonic();
//! clock.advance("1h30m".parse::<SignedDuration>().unwrap());
//!
//! assert_eq!(t0.elapsed_from(&clock), SignedDuration::from_mins(90));
//! assert_eq!(
//!     clock.now().to_offset(UtcOffset::from_hms(2, 0, 0).unwrap()).format_rfc3339(),
//!     "2024-06-01T15:30:00+02:00"
//! );
//! ```

use std::error::Error;
use std::fmt;

pub mod clock;
pub mod datetime;
pub mod duration;
pub mod timezone;

pub use std::time::{
    Duration, Instant, SystemTime, SystemTimeError, TryFromFloatSecsError, UNIX_EPOCH,
};

pub use clock::{Clock, ClockInstant, MockClock, SystemClock};
pub use datetime::{Date, DateTime, OffsetDateTime, Time, Weekday};
pub use duration::SignedDuration;
pub use timezone::{TimeZone, UtcOffset};

/// An error returned when parsing a duration, date, time, timestamp, or time zone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    /// The input was empty.
    Empty,
    /// The input did not match the expected syntax.
    InvalidFormat(&'static str),
    /// A numeric component could not be parsed.
    InvalidNumber(&'static str),
    /// A unit or time zone identifier was not recognized.
    UnknownUnit(&'static str),
    /// A parsed component was outside its valid calendar or clock range.
    OutOfRange(&'static str),
    /// The parsed value overflowed the target representation.
    Overflow,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("cannot parse empty input"),
            Self::InvalidFormat(msg) => write!(f, "invalid format: {msg}"),
            Self::InvalidNumber(msg) => write!(f, "invalid number: {msg}"),
            Self::UnknownUnit(msg) => write!(f, "unknown unit or zone: {msg}"),
            Self::OutOfRange(msg) => write!(f, "value out of range: {msg}"),
            Self::Overflow => f.write_str("parsed value overflowed representable range"),
        }
    }
}

impl Error for ParseError {}

/// An error returned when constructing or converting time values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TimeError {
    /// The calendar date does not exist (for example, February 29 in a non-leap year).
    InvalidDate {
        /// Calendar year.
        year: i32,
        /// Calendar month (`1..=12`).
        month: u8,
        /// Calendar day of month (`1..=31`).
        day: u8,
    },
    /// The wall-clock time component is out of range.
    InvalidTime {
        /// Hour (`0..=23`).
        hour: u8,
        /// Minute (`0..=59`).
        minute: u8,
        /// Second (`0..=59`).
        second: u8,
        /// Nanosecond (`0..=999_999_999`).
        nanosecond: u32,
    },
    /// The UTC offset components or total seconds are out of range or inconsistently signed.
    InvalidOffset,
    /// A negative [`SignedDuration`] cannot be converted into an unsigned [`Duration`].
    NegativeDuration,
    /// A mock [`ClockInstant`] cannot be converted into an OS [`Instant`].
    MockInstant,
    /// A value fell outside the valid range for the requested operation.
    OutOfRange,
    /// An arithmetic or conversion operation overflowed.
    Overflow,
    /// Parsing failed.
    Parse(ParseError),
}

impl From<ParseError> for TimeError {
    fn from(err: ParseError) -> Self {
        Self::Parse(err)
    }
}

impl fmt::Display for TimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidDate { year, month, day } => {
                write!(f, "invalid calendar date: {year:04}-{month:02}-{day:02}")
            }
            Self::InvalidTime {
                hour,
                minute,
                second,
                nanosecond,
            } => {
                write!(
                    f,
                    "invalid wall-clock time: {hour:02}:{minute:02}:{second:02}.{nanosecond:09}"
                )
            }
            Self::InvalidOffset => f.write_str("invalid UTC offset"),
            Self::NegativeDuration => {
                f.write_str("cannot convert negative duration to std::time::Duration")
            }
            Self::MockInstant => {
                f.write_str("cannot convert mock ClockInstant to std::time::Instant")
            }
            Self::OutOfRange => f.write_str("time value out of representable range"),
            Self::Overflow => f.write_str("time arithmetic overflow"),
            Self::Parse(err) => err.fmt(f),
        }
    }
}

impl Error for TimeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Parse(err) => Some(err),
            _ => None,
        }
    }
}
