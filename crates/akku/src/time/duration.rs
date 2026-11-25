//! Signed nanosecond-precision durations with human-readable formatting and parsing.

use std::fmt;
use std::ops::{Add, AddAssign, Div, DivAssign, Mul, MulAssign, Neg, Sub, SubAssign};
use std::str::FromStr;

use super::{ParseError, TimeError};

const NANOS_PER_SEC: i128 = 1_000_000_000;
const NANOS_PER_MILLI: i128 = 1_000_000;
const NANOS_PER_MICRO: i128 = 1_000;
const SECS_PER_MIN: i64 = 60;
const SECS_PER_HOUR: i64 = 3_600;
const SECS_PER_DAY: i64 = 86_400;

const MIN_TOTAL_NANOS: i128 = (i64::MIN as i128) * NANOS_PER_SEC - 999_999_999;
const MAX_TOTAL_NANOS: i128 = (i64::MAX as i128) * NANOS_PER_SEC + 999_999_999;

/// A signed span of time with nanosecond precision.
///
/// Named to match [`jiff::SignedDuration`](https://docs.rs/jiff/latest/jiff/struct.SignedDuration.html)
/// and avoid shadowing [`std::time::Duration`] (re-exported at [`crate::time::Duration`]).
///
/// Internally stored as a normalized `(secs, nanos)` pair where
/// `-999_999_999 <= nanos <= 999_999_999`, and `secs` and `nanos` never have
/// opposite signs.
///
/// # Examples
///
/// ```
/// use akku::time::SignedDuration;
///
/// let d: SignedDuration = "1h30m500ms".parse().unwrap();
/// assert_eq!(d.as_secs(), 5400);
/// assert_eq!(d.subsec_nanos(), 500_000_000);
/// assert_eq!(d.to_string(), "1h30m500ms");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct SignedDuration {
    secs: i64,
    nanos: i32,
}

impl SignedDuration {
    /// A zero-length duration.
    pub const ZERO: Self = Self { secs: 0, nanos: 0 };

    /// The minimum representable duration.
    pub const MIN: Self = Self {
        secs: i64::MIN,
        nanos: -999_999_999,
    };

    /// The maximum representable duration.
    pub const MAX: Self = Self {
        secs: i64::MAX,
        nanos: 999_999_999,
    };

    /// Creates a new `SignedDuration` from whole seconds and additional nanoseconds,
    /// normalizing the result.
    ///
    /// # Panics
    ///
    /// Panics if the normalized duration overflows the representable range.
    pub fn new(secs: i64, nanos: i32) -> Self {
        let total = i128::from(secs) * NANOS_PER_SEC + i128::from(nanos);
        Self::from_total_nanos(total).expect("duration overflow")
    }

    /// Creates a `SignedDuration` from a total nanosecond count, returning `None` on overflow.
    pub(crate) fn from_total_nanos(total: i128) -> Option<Self> {
        if !(MIN_TOTAL_NANOS..=MAX_TOTAL_NANOS).contains(&total) {
            return None;
        }
        let secs = (total / NANOS_PER_SEC) as i64;
        let nanos = (total % NANOS_PER_SEC) as i32;
        Some(Self { secs, nanos })
    }

    /// Creates a `SignedDuration` from a number of whole seconds.
    pub const fn from_secs(secs: i64) -> Self {
        Self { secs, nanos: 0 }
    }

    /// Creates a `SignedDuration` from a number of milliseconds.
    pub fn from_millis(millis: i64) -> Self {
        let total = i128::from(millis) * NANOS_PER_MILLI;
        Self::from_total_nanos(total).expect("duration overflow")
    }

    /// Creates a `SignedDuration` from a number of microseconds.
    pub fn from_micros(micros: i64) -> Self {
        let total = i128::from(micros) * NANOS_PER_MICRO;
        Self::from_total_nanos(total).expect("duration overflow")
    }

    /// Creates a `SignedDuration` from a number of nanoseconds.
    pub fn from_nanos(nanos: i64) -> Self {
        Self::from_total_nanos(i128::from(nanos)).expect("duration overflow")
    }

    /// Creates a `SignedDuration` from a number of minutes.
    ///
    /// # Panics
    ///
    /// Panics if `mins * 60` overflows `i64`.
    pub fn from_mins(mins: i64) -> Self {
        Self::from_secs(mins.checked_mul(SECS_PER_MIN).expect("duration overflow"))
    }

    /// Creates a `SignedDuration` from a number of hours.
    ///
    /// # Panics
    ///
    /// Panics if `hours * 3600` overflows `i64`.
    pub fn from_hours(hours: i64) -> Self {
        Self::from_secs(hours.checked_mul(SECS_PER_HOUR).expect("duration overflow"))
    }

    /// Creates a `SignedDuration` from a number of 24-hour days.
    ///
    /// # Panics
    ///
    /// Panics if `days * 86_400` overflows `i64`.
    pub fn from_days(days: i64) -> Self {
        Self::from_secs(days.checked_mul(SECS_PER_DAY).expect("duration overflow"))
    }

    /// Creates a `SignedDuration` from a floating-point number of seconds.
    ///
    /// # Panics
    ///
    /// Panics if `secs` is NaN, infinite, or outside the representable range.
    pub fn from_secs_f64(secs: f64) -> Self {
        Self::try_from_secs_f64(secs).expect("invalid or out-of-range f64 duration")
    }

    /// Fallibly creates a `SignedDuration` from a floating-point number of seconds.
    ///
    /// Returns [`TimeError::OutOfRange`] if `secs` is NaN or infinite, or
    /// [`TimeError::Overflow`] if the result exceeds [`SignedDuration::MIN`]..=[`SignedDuration::MAX`].
    pub fn try_from_secs_f64(secs: f64) -> Result<Self, TimeError> {
        if !secs.is_finite() {
            return Err(TimeError::OutOfRange);
        }
        let whole = secs.trunc();
        // i64::MAX as f64 rounds up to 2^63 (9_223_372_036_854_775_808.0),
        // which is strictly greater than i64::MAX.
        if whole < (i64::MIN as f64) || whole >= 9_223_372_036_854_775_808.0 {
            return Err(TimeError::Overflow);
        }
        let frac_nanos = (secs.fract() * 1_000_000_000.0).round() as i128;
        let total = i128::from(whole as i64) * NANOS_PER_SEC + frac_nanos;
        Self::from_total_nanos(total).ok_or(TimeError::Overflow)
    }

    /// Returns the whole-second component of the duration, truncated toward zero.
    pub const fn as_secs(&self) -> i64 {
        self.secs
    }

    /// Returns the fractional nanosecond component (`-999_999_999..=999_999_999`).
    pub const fn subsec_nanos(&self) -> i32 {
        self.nanos
    }

    /// Returns the total duration in seconds as `f64`.
    pub fn as_secs_f64(&self) -> f64 {
        (self.secs as f64) + f64::from(self.nanos) / 1_000_000_000.0
    }

    /// Returns the total number of whole milliseconds, truncated toward zero.
    pub const fn as_millis(&self) -> i128 {
        self.as_nanos() / NANOS_PER_MILLI
    }

    /// Returns the total number of whole microseconds, truncated toward zero.
    pub const fn as_micros(&self) -> i128 {
        self.as_nanos() / NANOS_PER_MICRO
    }

    /// Returns the total number of nanoseconds as `i128`.
    pub const fn as_nanos(&self) -> i128 {
        (self.secs as i128) * NANOS_PER_SEC + (self.nanos as i128)
    }

    /// Returns `true` if this duration is zero.
    pub const fn is_zero(&self) -> bool {
        self.secs == 0 && self.nanos == 0
    }

    /// Returns `true` if this duration is strictly less than zero.
    pub const fn is_negative(&self) -> bool {
        self.secs < 0 || self.nanos < 0
    }

    /// Returns `true` if this duration is strictly greater than zero.
    pub const fn is_positive(&self) -> bool {
        self.secs > 0 || self.nanos > 0
    }

    /// Returns the absolute value of this duration.
    ///
    /// # Panics
    ///
    /// Panics if `self` is [`SignedDuration::MIN`] (whose negation overflows `i64`).
    pub fn abs(&self) -> Self {
        self.checked_abs().expect("duration abs overflow")
    }

    /// Checked absolute value, returning `None` if negating `self` overflows.
    pub fn checked_abs(&self) -> Option<Self> {
        if self.is_negative() {
            self.checked_neg()
        } else {
            Some(*self)
        }
    }

    /// Checked negation, returning `None` on overflow.
    pub fn checked_neg(self) -> Option<Self> {
        self.as_nanos()
            .checked_neg()
            .and_then(Self::from_total_nanos)
    }

    /// Checked addition, returning `None` on overflow.
    pub fn checked_add(self, rhs: Self) -> Option<Self> {
        self.as_nanos()
            .checked_add(rhs.as_nanos())
            .and_then(Self::from_total_nanos)
    }

    /// Checked subtraction, returning `None` on overflow.
    pub fn checked_sub(self, rhs: Self) -> Option<Self> {
        self.as_nanos()
            .checked_sub(rhs.as_nanos())
            .and_then(Self::from_total_nanos)
    }

    /// Checked integer multiplication, returning `None` on overflow.
    pub fn checked_mul(self, rhs: i32) -> Option<Self> {
        self.as_nanos()
            .checked_mul(i128::from(rhs))
            .and_then(Self::from_total_nanos)
    }

    /// Checked integer division, returning `None` if `rhs == 0` or on overflow.
    pub fn checked_div(self, rhs: i32) -> Option<Self> {
        if rhs == 0 {
            return None;
        }
        self.as_nanos()
            .checked_div(i128::from(rhs))
            .and_then(Self::from_total_nanos)
    }

    /// Parses a human-readable duration string.
    ///
    /// Supported units are `ns`, `us`, `µs` (U+00B5), `μs` (U+03BC), `ms`, `s`,
    /// `m`, `h`, and `d`. Compound forms such as `"1h30m500ms"` and an optional
    /// leading `-` or `+` sign are supported.
    ///
    /// # Examples
    ///
    /// ```
    /// use akku::time::SignedDuration;
    ///
    /// assert_eq!(
    ///     SignedDuration::parse_human("1h30m500ms").unwrap(),
    ///     SignedDuration::from_millis(5_400_500)
    /// );
    /// assert_eq!(
    ///     SignedDuration::parse_human("-250ms").unwrap(),
    ///     SignedDuration::from_millis(-250)
    /// );
    /// ```
    pub fn parse_human(input: &str) -> Result<Self, ParseError> {
        if input.is_empty() {
            return Err(ParseError::Empty);
        }

        let (negative, mut rest) = if let Some(stripped) = input.strip_prefix('-') {
            (true, stripped)
        } else if let Some(stripped) = input.strip_prefix('+') {
            (false, stripped)
        } else {
            (false, input)
        };

        if rest.is_empty() {
            return Err(ParseError::InvalidFormat("missing duration value"));
        }

        let mut total_nanos: u128 = 0;

        while !rest.is_empty() {
            // Parse integer part.
            let int_end = rest
                .bytes()
                .position(|b| !b.is_ascii_digit())
                .unwrap_or(rest.len());
            if int_end == 0 {
                return Err(ParseError::InvalidNumber("expected digits before unit"));
            }
            let int_str = &rest[..int_end];
            rest = &rest[int_end..];

            let int_val: u128 = int_str.parse().map_err(|_| ParseError::Overflow)?;

            // Parse optional fractional part.
            let mut frac_val: u128 = 0;
            let mut frac_scale: u128 = 1;
            if let Some(after_dot) = rest.strip_prefix('.') {
                let frac_end = after_dot
                    .bytes()
                    .position(|b| !b.is_ascii_digit())
                    .unwrap_or(after_dot.len());
                if frac_end == 0 {
                    return Err(ParseError::InvalidNumber(
                        "expected digits after decimal point",
                    ));
                }
                let frac_str = &after_dot[..frac_end];
                rest = &after_dot[frac_end..];

                // Keep up to 18 fractional digits to avoid u128 overflow; beyond
                // that is sub-nanosecond even for days.
                for byte in frac_str.bytes().take(18) {
                    frac_val = frac_val * 10 + u128::from(byte - b'0');
                    frac_scale *= 10;
                }
            }

            // Parse unit identifier (up to next ASCII digit).
            let unit_end = rest
                .bytes()
                .position(|b| b.is_ascii_digit())
                .unwrap_or(rest.len());
            if unit_end == 0 {
                return Err(ParseError::InvalidFormat("missing unit suffix"));
            }
            let unit_str = &rest[..unit_end];
            rest = &rest[unit_end..];

            let unit_nanos: u128 = match unit_str {
                "ns" => 1,
                "us" | "µs" | "μs" => 1_000,
                "ms" => 1_000_000,
                "s" => 1_000_000_000,
                "m" => 60 * 1_000_000_000,
                "h" => 3_600 * 1_000_000_000,
                "d" => 86_400 * 1_000_000_000,
                _ => return Err(ParseError::UnknownUnit("unrecognized duration unit")),
            };

            let whole_ns = int_val
                .checked_mul(unit_nanos)
                .ok_or(ParseError::Overflow)?;
            let frac_ns = if frac_val > 0 {
                // Compute (frac_val * unit_nanos) / frac_scale without overflow.
                // Since frac_scale <= 10^18 and unit_nanos <= 8.64e13, product fits in u128.
                (frac_val
                    .checked_mul(unit_nanos)
                    .ok_or(ParseError::Overflow)?)
                    / frac_scale
            } else {
                0
            };

            total_nanos = total_nanos
                .checked_add(whole_ns)
                .and_then(|v| v.checked_add(frac_ns))
                .ok_or(ParseError::Overflow)?;
        }

        let signed: i128 = if negative {
            let max_neg = MIN_TOTAL_NANOS.unsigned_abs();
            if total_nanos > max_neg {
                return Err(ParseError::Overflow);
            }
            if total_nanos == 0 {
                0
            } else {
                -(total_nanos as i128)
            }
        } else {
            if total_nanos > (MAX_TOTAL_NANOS as u128) {
                return Err(ParseError::Overflow);
            }
            total_nanos as i128
        };

        Self::from_total_nanos(signed).ok_or(ParseError::Overflow)
    }
}

impl Add for SignedDuration {
    type Output = Self;

    fn add(self, rhs: Self) -> Self::Output {
        self.checked_add(rhs).expect("duration addition overflow")
    }
}

impl AddAssign for SignedDuration {
    fn add_assign(&mut self, rhs: Self) {
        *self = *self + rhs;
    }
}

impl Sub for SignedDuration {
    type Output = Self;

    fn sub(self, rhs: Self) -> Self::Output {
        self.checked_sub(rhs)
            .expect("duration subtraction overflow")
    }
}

impl SubAssign for SignedDuration {
    fn sub_assign(&mut self, rhs: Self) {
        *self = *self - rhs;
    }
}

impl Neg for SignedDuration {
    type Output = Self;

    fn neg(self) -> Self::Output {
        self.checked_neg().expect("duration negation overflow")
    }
}

impl Mul<i32> for SignedDuration {
    type Output = Self;

    fn mul(self, rhs: i32) -> Self::Output {
        self.checked_mul(rhs)
            .expect("duration multiplication overflow")
    }
}

impl Mul<SignedDuration> for i32 {
    type Output = SignedDuration;

    fn mul(self, rhs: SignedDuration) -> Self::Output {
        rhs.checked_mul(self)
            .expect("duration multiplication overflow")
    }
}

impl MulAssign<i32> for SignedDuration {
    fn mul_assign(&mut self, rhs: i32) {
        *self = *self * rhs;
    }
}

impl Div<i32> for SignedDuration {
    type Output = Self;

    fn div(self, rhs: i32) -> Self::Output {
        self.checked_div(rhs)
            .expect("duration division by zero or overflow")
    }
}

impl DivAssign<i32> for SignedDuration {
    fn div_assign(&mut self, rhs: i32) {
        *self = *self / rhs;
    }
}

impl TryFrom<std::time::Duration> for SignedDuration {
    type Error = TimeError;

    fn try_from(value: std::time::Duration) -> Result<Self, Self::Error> {
        let secs = i64::try_from(value.as_secs()).map_err(|_| TimeError::Overflow)?;
        let nanos = value.subsec_nanos() as i32;
        Ok(Self { secs, nanos })
    }
}

impl TryFrom<SignedDuration> for std::time::Duration {
    type Error = TimeError;

    fn try_from(value: SignedDuration) -> Result<Self, Self::Error> {
        if value.is_negative() {
            return Err(TimeError::NegativeDuration);
        }
        Ok(Self::new(value.secs as u64, value.nanos as u32))
    }
}

impl fmt::Display for SignedDuration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_zero() {
            return f.write_str("0s");
        }
        if self.is_negative() {
            f.write_str("-")?;
        }

        let mut rem: u128 = self.as_nanos().unsigned_abs();
        let units: [(u128, &str); 7] = [
            (86_400 * 1_000_000_000, "d"),
            (3_600 * 1_000_000_000, "h"),
            (60 * 1_000_000_000, "m"),
            (1_000_000_000, "s"),
            (1_000_000, "ms"),
            (1_000, "us"),
            (1, "ns"),
        ];

        for (scale, suffix) in units {
            let count = rem / scale;
            if count > 0 {
                write!(f, "{count}{suffix}")?;
                rem %= scale;
            }
        }
        Ok(())
    }
}

impl FromStr for SignedDuration {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse_human(s)
    }
}
