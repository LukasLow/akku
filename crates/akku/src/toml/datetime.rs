//! Dates, times and date-times as defined by RFC 3339 and TOML.

use std::fmt;
use std::str::FromStr;

use super::error::{position_at, Error, ErrorKind, Result};

/// A calendar date.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Date {
    /// The year, for example `1979`.
    pub year: i16,
    /// The month, `1` through `12`.
    pub month: u8,
    /// The day of the month, `1` through `31`.
    pub day: u8,
}

/// A wall clock time.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Time {
    /// The hour, `0` through `23`.
    pub hour: u8,
    /// The minute, `0` through `59`.
    pub minute: u8,
    /// The second, `0` through `60` to allow leap seconds.
    pub second: u8,
    /// The nanosecond fraction, `0` through `999_999_999`.
    pub nanosecond: u32,
}

/// The UTC offset of a date-time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Offset {
    /// `Z`, the zero offset.
    Zulu,
    /// A custom offset; both fields carry the sign.
    Custom {
        /// The hour part of the offset.
        hours: i8,
        /// The minute part of the offset.
        minutes: i8,
    },
}

/// A TOML date, time or date-time.
///
/// All four TOML variants are represented by the same type: a date only, a
/// time only, a local date-time and an offset date-time.
///
/// ```
/// use akku::toml::{Datetime, Offset};
///
/// let datetime = Datetime::parse("1979-05-27T07:32:00Z").unwrap();
/// assert_eq!(datetime.date.unwrap().day, 27);
/// assert_eq!(datetime.offset, Some(Offset::Zulu));
/// assert_eq!(datetime.to_string(), "1979-05-27T07:32:00Z");
/// ```
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Datetime {
    /// The date part, if the value has one.
    pub date: Option<Date>,
    /// The time part, if the value has one.
    pub time: Option<Time>,
    /// The UTC offset, if the value has one.
    pub offset: Option<Offset>,
}

impl Datetime {
    /// Parse a TOML date, time or date-time.
    ///
    /// Calendar dates are validated, so `1979-02-30` and `25:00:00` are
    /// rejected.
    pub fn parse(input: &str) -> Result<Self> {
        parse_datetime(input.as_bytes()).map_err(|reason| {
            Error::new(
                ErrorKind::InvalidDatetime { reason },
                position_at(input.as_bytes(), 0),
            )
        })
    }
}

impl FromStr for Datetime {
    type Err = Error;

    fn from_str(input: &str) -> Result<Self> {
        Datetime::parse(input)
    }
}

impl fmt::Display for Datetime {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(date) = self.date {
            write!(formatter, "{date}")?;
        }
        if let Some(time) = self.time {
            if self.date.is_some() {
                formatter.write_str("T")?;
            }
            write!(formatter, "{time}")?;
            if let Some(offset) = self.offset {
                write!(formatter, "{offset}")?;
            }
        }
        Ok(())
    }
}

impl fmt::Display for Date {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{:04}-{:02}-{:02}",
            self.year, self.month, self.day
        )
    }
}

impl fmt::Display for Time {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{:02}:{:02}:{:02}",
            self.hour, self.minute, self.second
        )?;
        if self.nanosecond > 0 {
            let digits = format!("{:09}", self.nanosecond);
            formatter.write_str(".")?;
            formatter.write_str(digits.trim_end_matches('0'))?;
        }
        Ok(())
    }
}

impl fmt::Display for Offset {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Zulu => formatter.write_str("Z"),
            Self::Custom { hours, minutes } => {
                let negative = *hours < 0 || *minutes < 0;
                let sign = if negative { '-' } else { '+' };
                write!(formatter, "{sign}{:02}:{:02}", hours.abs(), minutes.abs())
            }
        }
    }
}

fn parse_datetime(bytes: &[u8]) -> std::result::Result<Datetime, &'static str> {
    let mut position = 0usize;
    let mut datetime = Datetime::default();
    let has_date = looks_like_date(bytes, 0);
    if has_date {
        let year = take_digits(bytes, &mut position, 4).ok_or("invalid year")?;
        expect_byte(bytes, &mut position, b'-').ok_or("invalid date separator")?;
        let month = take_digits(bytes, &mut position, 2).ok_or("invalid month")?;
        expect_byte(bytes, &mut position, b'-').ok_or("invalid date separator")?;
        let day = take_digits(bytes, &mut position, 2).ok_or("invalid day")?;

        let year = i16::try_from(year).map_err(|_| "year out of range")?;
        let month = u8::try_from(month).map_err(|_| "month out of range")?;
        let day = u8::try_from(day).map_err(|_| "day out of range")?;
        if !(1..=12).contains(&month) {
            return Err("month out of range");
        }
        if day == 0 || day > days_in_month(year, month) {
            return Err("day out of range");
        }
        datetime.date = Some(Date { year, month, day });

        if position == bytes.len() {
            return Ok(datetime);
        }
        match bytes.get(position) {
            Some(b'T') | Some(b't') | Some(b' ') => position += 1,
            _ => return Err("unexpected character after date"),
        }
    }

    let hour = take_digits(bytes, &mut position, 2).ok_or("invalid hour")?;
    expect_byte(bytes, &mut position, b':').ok_or("invalid time separator")?;
    let minute = take_digits(bytes, &mut position, 2).ok_or("invalid minute")?;
    expect_byte(bytes, &mut position, b':').ok_or("invalid time separator")?;
    let second = take_digits(bytes, &mut position, 2).ok_or("invalid second")?;

    if hour > 23 {
        return Err("hour out of range");
    }
    if minute > 59 {
        return Err("minute out of range");
    }
    if second > 60 {
        return Err("second out of range");
    }

    let mut nanosecond = 0u32;
    if bytes.get(position) == Some(&b'.') {
        position += 1;
        let start = position;
        while matches!(bytes.get(position), Some(byte) if byte.is_ascii_digit()) {
            position += 1;
        }
        let digits = position - start;
        if digits == 0 {
            return Err("empty fractional second");
        }
        for index in 0..9 {
            nanosecond *= 10;
            if index < digits {
                nanosecond += u32::from(bytes[start + index] - b'0');
            }
        }
    }

    datetime.time = Some(Time {
        hour: hour as u8,
        minute: minute as u8,
        second: second as u8,
        nanosecond,
    });

    match bytes.get(position) {
        None => {}
        Some(b'Z') | Some(b'z') => {
            position += 1;
            datetime.offset = Some(Offset::Zulu);
        }
        Some(b'+') | Some(b'-') => {
            let negative = bytes[position] == b'-';
            position += 1;
            let hours = take_digits(bytes, &mut position, 2).ok_or("invalid offset hour")?;
            expect_byte(bytes, &mut position, b':').ok_or("invalid offset separator")?;
            let minutes = take_digits(bytes, &mut position, 2).ok_or("invalid offset minute")?;
            if hours > 23 || minutes > 59 {
                return Err("offset out of range");
            }
            let sign: i8 = if negative { -1 } else { 1 };
            datetime.offset = Some(Offset::Custom {
                hours: sign * hours as i8,
                minutes: sign * minutes as i8,
            });
        }
        Some(_) => return Err("unexpected character after time"),
    }

    if position != bytes.len() {
        return Err("trailing characters");
    }
    Ok(datetime)
}

fn looks_like_date(bytes: &[u8], offset: usize) -> bool {
    let end = offset + 5;
    end <= bytes.len()
        && bytes[offset..offset + 4].iter().all(u8::is_ascii_digit)
        && bytes[offset + 4] == b'-'
}

fn take_digits(bytes: &[u8], position: &mut usize, count: usize) -> Option<u32> {
    let end = position.checked_add(count)?;
    let slice = bytes.get(*position..end)?;
    let mut value = 0u32;
    for &byte in slice {
        if !byte.is_ascii_digit() {
            return None;
        }
        value = value * 10 + u32::from(byte - b'0');
    }
    *position = end;
    Some(value)
}

fn expect_byte(bytes: &[u8], position: &mut usize, byte: u8) -> Option<()> {
    if bytes.get(*position) == Some(&byte) {
        *position += 1;
        Some(())
    } else {
        None
    }
}

fn days_in_month(year: i16, month: u8) -> u8 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => 0,
    }
}

fn is_leap_year(year: i16) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}
