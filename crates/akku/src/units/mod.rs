//! Strongly typed physical quantities, SI unit conversions, and dimensional arithmetic.
//!
//! Every quantity stores its value in canonical SI base units (except [`TimeSpan`],
//! which preserves exact nanosecond representation when created from
//! [`crate::time::SignedDuration`]).
//!
//! # Examples
//!
//! ```
//! use akku::units::{Length, Speed, Temperature, TimeSpan};
//!
//! let distance = Length::from_kilometers(100.0);
//! let elapsed = TimeSpan::from_hours(2.0);
//! let speed: Speed = distance / elapsed;
//! assert!((speed.as_kph() - 50.0).abs() < 1e-9);
//!
//! let freezing = Temperature::try_from_celsius(0.0).unwrap();
//! assert!((freezing.as_fahrenheit() - 32.0).abs() < 1e-9);
//! ```

use std::cmp::Ordering;
use std::error::Error;
use std::fmt;
use std::ops::{Add, AddAssign, Div, DivAssign, Mul, MulAssign, Neg, Sub, SubAssign};

use crate::time::SignedDuration;

/// An error returned when constructing a physically invalid quantity.
#[derive(Debug, Clone, PartialEq)]
pub enum UnitError {
    /// A temperature fell below absolute zero (`0 K`).
    BelowAbsoluteZero {
        /// The rejected temperature in kelvin.
        kelvin: f64,
    },
    /// A non-finite floating-point value (NaN or infinity) was rejected.
    NotFinite,
}

impl fmt::Display for UnitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BelowAbsoluteZero { kelvin } => {
                write!(f, "temperature {kelvin} K is below absolute zero (0 K)")
            }
            Self::NotFinite => f.write_str("physical quantity value must be finite"),
        }
    }
}

impl Error for UnitError {}

/// A physical quantity expressible in a canonical SI unit.
pub trait Quantity: Copy + PartialEq + PartialOrd {
    /// Returns the numeric value in the canonical SI base unit.
    fn si_value(self) -> f64;

    /// Constructs a quantity from a numeric value in the canonical SI base unit.
    fn from_si(value: f64) -> Self;

    /// Returns the canonical SI unit symbol (for example, `"m"`, `"kg"`, `"s"`).
    fn unit_symbol() -> &'static str;
}

macro_rules! impl_linear_quantity {
    ($ty:ident, $symbol:literal) => {
        impl Quantity for $ty {
            fn si_value(self) -> f64 {
                self.0
            }

            fn from_si(value: f64) -> Self {
                Self(value)
            }

            fn unit_symbol() -> &'static str {
                $symbol
            }
        }

        impl Add for $ty {
            type Output = Self;

            fn add(self, rhs: Self) -> Self::Output {
                Self(self.0 + rhs.0)
            }
        }

        impl AddAssign for $ty {
            fn add_assign(&mut self, rhs: Self) {
                self.0 += rhs.0;
            }
        }

        impl Sub for $ty {
            type Output = Self;

            fn sub(self, rhs: Self) -> Self::Output {
                Self(self.0 - rhs.0)
            }
        }

        impl SubAssign for $ty {
            fn sub_assign(&mut self, rhs: Self) {
                self.0 -= rhs.0;
            }
        }

        impl Neg for $ty {
            type Output = Self;

            fn neg(self) -> Self::Output {
                Self(-self.0)
            }
        }

        impl Mul<f64> for $ty {
            type Output = Self;

            fn mul(self, rhs: f64) -> Self::Output {
                Self(self.0 * rhs)
            }
        }

        impl Mul<$ty> for f64 {
            type Output = $ty;

            fn mul(self, rhs: $ty) -> Self::Output {
                $ty(self * rhs.0)
            }
        }

        impl MulAssign<f64> for $ty {
            fn mul_assign(&mut self, rhs: f64) {
                self.0 *= rhs;
            }
        }

        impl Div<f64> for $ty {
            type Output = Self;

            fn div(self, rhs: f64) -> Self::Output {
                Self(self.0 / rhs)
            }
        }

        impl DivAssign<f64> for $ty {
            fn div_assign(&mut self, rhs: f64) {
                self.0 /= rhs;
            }
        }

        impl Div<$ty> for $ty {
            type Output = f64;

            fn div(self, rhs: $ty) -> Self::Output {
                self.0 / rhs.0
            }
        }

        impl fmt::Display for $ty {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{} {}", self.0, $symbol)
            }
        }
    };
}

// ---------------------------------------------------------------------------
// Length
// ---------------------------------------------------------------------------

const METERS_PER_KILOMETER: f64 = 1_000.0;
const METERS_PER_CENTIMETER: f64 = 0.01;
const METERS_PER_MILLIMETER: f64 = 0.001;
const METERS_PER_MICROMETER: f64 = 1e-6;
const METERS_PER_NANOMETER: f64 = 1e-9;
const METERS_PER_INCH: f64 = 0.0254;
const METERS_PER_FOOT: f64 = 0.3048;
const METERS_PER_YARD: f64 = 0.9144;
const METERS_PER_MILE: f64 = 1_609.344;
const METERS_PER_NAUTICAL_MILE: f64 = 1_852.0;

/// A physical length stored in SI meters (`m`).
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Default)]
pub struct Length(f64);

impl Length {
    /// Zero length.
    pub const ZERO: Self = Self(0.0);

    /// Creates a `Length` from meters.
    pub const fn from_meters(value: f64) -> Self {
        Self(value)
    }

    /// Creates a `Length` from kilometers.
    pub fn from_kilometers(value: f64) -> Self {
        Self(value * METERS_PER_KILOMETER)
    }

    /// Creates a `Length` from centimeters.
    pub fn from_centimeters(value: f64) -> Self {
        Self(value * METERS_PER_CENTIMETER)
    }

    /// Creates a `Length` from millimeters.
    pub fn from_millimeters(value: f64) -> Self {
        Self(value * METERS_PER_MILLIMETER)
    }

    /// Creates a `Length` from micrometers (`µm`).
    pub fn from_micrometers(value: f64) -> Self {
        Self(value * METERS_PER_MICROMETER)
    }

    /// Creates a `Length` from nanometers (`nm`).
    pub fn from_nanometers(value: f64) -> Self {
        Self(value * METERS_PER_NANOMETER)
    }

    /// Creates a `Length` from international inches (`0.0254 m`).
    pub fn from_inches(value: f64) -> Self {
        Self(value * METERS_PER_INCH)
    }

    /// Creates a `Length` from international feet (`0.3048 m`).
    pub fn from_feet(value: f64) -> Self {
        Self(value * METERS_PER_FOOT)
    }

    /// Creates a `Length` from international yards (`0.9144 m`).
    pub fn from_yards(value: f64) -> Self {
        Self(value * METERS_PER_YARD)
    }

    /// Creates a `Length` from statute miles (`1609.344 m`).
    pub fn from_miles(value: f64) -> Self {
        Self(value * METERS_PER_MILE)
    }

    /// Creates a `Length` from nautical miles (`1852 m`).
    pub fn from_nautical_miles(value: f64) -> Self {
        Self(value * METERS_PER_NAUTICAL_MILE)
    }

    /// Returns this length in meters.
    pub const fn as_meters(self) -> f64 {
        self.0
    }

    /// Returns this length in kilometers.
    pub fn as_kilometers(self) -> f64 {
        self.0 / METERS_PER_KILOMETER
    }

    /// Returns this length in centimeters.
    pub fn as_centimeters(self) -> f64 {
        self.0 / METERS_PER_CENTIMETER
    }

    /// Returns this length in millimeters.
    pub fn as_millimeters(self) -> f64 {
        self.0 / METERS_PER_MILLIMETER
    }

    /// Returns this length in micrometers (`µm`).
    pub fn as_micrometers(self) -> f64 {
        self.0 / METERS_PER_MICROMETER
    }

    /// Returns this length in nanometers (`nm`).
    pub fn as_nanometers(self) -> f64 {
        self.0 / METERS_PER_NANOMETER
    }

    /// Returns this length in international inches.
    pub fn as_inches(self) -> f64 {
        self.0 / METERS_PER_INCH
    }

    /// Returns this length in international feet.
    pub fn as_feet(self) -> f64 {
        self.0 / METERS_PER_FOOT
    }

    /// Returns this length in international yards.
    pub fn as_yards(self) -> f64 {
        self.0 / METERS_PER_YARD
    }

    /// Returns this length in statute miles.
    pub fn as_miles(self) -> f64 {
        self.0 / METERS_PER_MILE
    }

    /// Returns this length in nautical miles.
    pub fn as_nautical_miles(self) -> f64 {
        self.0 / METERS_PER_NAUTICAL_MILE
    }
}

impl_linear_quantity!(Length, "m");

// ---------------------------------------------------------------------------
// Mass
// ---------------------------------------------------------------------------

const KG_PER_GRAM: f64 = 1e-3;
const KG_PER_MILLIGRAM: f64 = 1e-6;
const KG_PER_MICROGRAM: f64 = 1e-9;
const KG_PER_TONNE: f64 = 1_000.0;
const KG_PER_POUND: f64 = 0.453_592_37;
const KG_PER_OUNCE: f64 = KG_PER_POUND / 16.0;

/// A physical mass stored in SI kilograms (`kg`).
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Default)]
pub struct Mass(f64);

impl Mass {
    /// Zero mass.
    pub const ZERO: Self = Self(0.0);

    /// Creates a `Mass` from kilograms.
    pub const fn from_kilograms(value: f64) -> Self {
        Self(value)
    }

    /// Creates a `Mass` from grams.
    pub fn from_grams(value: f64) -> Self {
        Self(value * KG_PER_GRAM)
    }

    /// Creates a `Mass` from milligrams.
    pub fn from_milligrams(value: f64) -> Self {
        Self(value * KG_PER_MILLIGRAM)
    }

    /// Creates a `Mass` from micrograms (`µg`).
    pub fn from_micrograms(value: f64) -> Self {
        Self(value * KG_PER_MICROGRAM)
    }

    /// Creates a `Mass` from metric tonnes (`1000 kg`).
    pub fn from_tonnes(value: f64) -> Self {
        Self(value * KG_PER_TONNE)
    }

    /// Creates a `Mass` from international avoirdupois pounds (`0.45359237 kg`).
    pub fn from_pounds(value: f64) -> Self {
        Self(value * KG_PER_POUND)
    }

    /// Creates a `Mass` from international avoirdupois ounces (`1/16 lb`).
    pub fn from_ounces(value: f64) -> Self {
        Self(value * KG_PER_OUNCE)
    }

    /// Returns this mass in kilograms.
    pub const fn as_kilograms(self) -> f64 {
        self.0
    }

    /// Returns this mass in grams.
    pub fn as_grams(self) -> f64 {
        self.0 / KG_PER_GRAM
    }

    /// Returns this mass in milligrams.
    pub fn as_milligrams(self) -> f64 {
        self.0 / KG_PER_MILLIGRAM
    }

    /// Returns this mass in micrograms (`µg`).
    pub fn as_micrograms(self) -> f64 {
        self.0 / KG_PER_MICROGRAM
    }

    /// Returns this mass in metric tonnes.
    pub fn as_tonnes(self) -> f64 {
        self.0 / KG_PER_TONNE
    }

    /// Returns this mass in international avoirdupois pounds.
    pub fn as_pounds(self) -> f64 {
        self.0 / KG_PER_POUND
    }

    /// Returns this mass in international avoirdupois ounces.
    pub fn as_ounces(self) -> f64 {
        self.0 / KG_PER_OUNCE
    }
}

impl_linear_quantity!(Mass, "kg");

// ---------------------------------------------------------------------------
// TimeSpan (Exact Duration or Float seconds)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
enum TimeSpanRepr {
    Exact(SignedDuration),
    Float(f64),
}

/// A physical time interval in SI seconds (`s`) with lossless [`SignedDuration`] interop.
///
/// Constructing a `TimeSpan` via `TimeSpan::from(duration)` stores the exact
/// nanosecond [`SignedDuration`] internally so [`TimeSpan::to_duration`] round-trips
/// without floating-point precision loss.
///
/// # Examples
///
/// ```
/// use akku::time::SignedDuration;
/// use akku::units::TimeSpan;
///
/// let exact = SignedDuration::new(10_000_000_000, 1);
/// let span = TimeSpan::from(exact);
/// assert_eq!(span.to_duration(), Some(exact));
/// ```
#[derive(Debug, Clone, Copy)]
pub struct TimeSpan(TimeSpanRepr);

impl Default for TimeSpan {
    fn default() -> Self {
        Self::ZERO
    }
}

impl TimeSpan {
    /// Zero time span.
    pub const ZERO: Self = Self(TimeSpanRepr::Exact(SignedDuration::ZERO));

    /// Creates a `TimeSpan` from floating-point seconds.
    pub const fn from_secs(secs: f64) -> Self {
        Self(TimeSpanRepr::Float(secs))
    }

    /// Creates a `TimeSpan` from milliseconds.
    pub fn from_millis(millis: f64) -> Self {
        Self::from_secs(millis * 1e-3)
    }

    /// Creates a `TimeSpan` from microseconds.
    pub fn from_micros(micros: f64) -> Self {
        Self::from_secs(micros * 1e-6)
    }

    /// Creates a `TimeSpan` from nanoseconds.
    pub fn from_nanos(nanos: f64) -> Self {
        Self::from_secs(nanos * 1e-9)
    }

    /// Creates a `TimeSpan` from minutes.
    pub fn from_mins(mins: f64) -> Self {
        Self::from_secs(mins * 60.0)
    }

    /// Creates a `TimeSpan` from hours.
    pub fn from_hours(hours: f64) -> Self {
        Self::from_secs(hours * 3_600.0)
    }

    /// Creates a `TimeSpan` from 24-hour days.
    pub fn from_days(days: f64) -> Self {
        Self::from_secs(days * 86_400.0)
    }

    /// Converts this `TimeSpan` into an [`akku::time::SignedDuration`](SignedDuration).
    ///
    /// When backed by an exact `SignedDuration`, this returns it losslessly. When
    /// backed by floating-point seconds, this returns `None` if the value is
    /// non-finite or overflows `SignedDuration`.
    pub fn to_duration(self) -> Option<SignedDuration> {
        match self.0 {
            TimeSpanRepr::Exact(d) => Some(d),
            TimeSpanRepr::Float(s) => SignedDuration::try_from_secs_f64(s).ok(),
        }
    }

    /// Returns this time span in seconds as `f64`.
    pub fn as_secs(self) -> f64 {
        match self.0 {
            TimeSpanRepr::Exact(d) => d.as_secs_f64(),
            TimeSpanRepr::Float(s) => s,
        }
    }

    /// Returns this time span in milliseconds as `f64`.
    pub fn as_millis(self) -> f64 {
        self.as_secs() * 1e3
    }

    /// Returns this time span in microseconds as `f64`.
    pub fn as_micros(self) -> f64 {
        self.as_secs() * 1e6
    }

    /// Returns this time span in nanoseconds as `f64`.
    pub fn as_nanos(self) -> f64 {
        self.as_secs() * 1e9
    }

    /// Returns this time span in minutes as `f64`.
    pub fn as_mins(self) -> f64 {
        self.as_secs() / 60.0
    }

    /// Returns this time span in hours as `f64`.
    pub fn as_hours(self) -> f64 {
        self.as_secs() / 3_600.0
    }

    /// Returns this time span in 24-hour days as `f64`.
    pub fn as_days(self) -> f64 {
        self.as_secs() / 86_400.0
    }
}

impl From<SignedDuration> for TimeSpan {
    fn from(duration: SignedDuration) -> Self {
        Self(TimeSpanRepr::Exact(duration))
    }
}

impl PartialEq for TimeSpan {
    fn eq(&self, other: &Self) -> bool {
        match (self.0, other.0) {
            (TimeSpanRepr::Exact(a), TimeSpanRepr::Exact(b)) => a == b,
            _ => self.as_secs() == other.as_secs(),
        }
    }
}

impl PartialOrd for TimeSpan {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        match (self.0, other.0) {
            (TimeSpanRepr::Exact(a), TimeSpanRepr::Exact(b)) => Some(a.cmp(&b)),
            _ => self.as_secs().partial_cmp(&other.as_secs()),
        }
    }
}

impl Quantity for TimeSpan {
    fn si_value(self) -> f64 {
        self.as_secs()
    }

    fn from_si(value: f64) -> Self {
        Self::from_secs(value)
    }

    fn unit_symbol() -> &'static str {
        "s"
    }
}

impl Add for TimeSpan {
    type Output = Self;

    fn add(self, rhs: Self) -> Self::Output {
        match (self.0, rhs.0) {
            (TimeSpanRepr::Exact(a), TimeSpanRepr::Exact(b)) => match a.checked_add(b) {
                Some(sum) => Self(TimeSpanRepr::Exact(sum)),
                None => Self::from_secs(a.as_secs_f64() + b.as_secs_f64()),
            },
            _ => Self::from_secs(self.as_secs() + rhs.as_secs()),
        }
    }
}

impl AddAssign for TimeSpan {
    fn add_assign(&mut self, rhs: Self) {
        *self = *self + rhs;
    }
}

impl Sub for TimeSpan {
    type Output = Self;

    fn sub(self, rhs: Self) -> Self::Output {
        match (self.0, rhs.0) {
            (TimeSpanRepr::Exact(a), TimeSpanRepr::Exact(b)) => match a.checked_sub(b) {
                Some(diff) => Self(TimeSpanRepr::Exact(diff)),
                None => Self::from_secs(a.as_secs_f64() - b.as_secs_f64()),
            },
            _ => Self::from_secs(self.as_secs() - rhs.as_secs()),
        }
    }
}

impl SubAssign for TimeSpan {
    fn sub_assign(&mut self, rhs: Self) {
        *self = *self - rhs;
    }
}

impl Neg for TimeSpan {
    type Output = Self;

    fn neg(self) -> Self::Output {
        match self.0 {
            TimeSpanRepr::Exact(d) => match d.checked_neg() {
                Some(neg) => Self(TimeSpanRepr::Exact(neg)),
                None => Self::from_secs(-d.as_secs_f64()),
            },
            TimeSpanRepr::Float(s) => Self::from_secs(-s),
        }
    }
}

impl Mul<f64> for TimeSpan {
    type Output = Self;

    fn mul(self, rhs: f64) -> Self::Output {
        Self::from_secs(self.as_secs() * rhs)
    }
}

impl Mul<TimeSpan> for f64 {
    type Output = TimeSpan;

    fn mul(self, rhs: TimeSpan) -> Self::Output {
        TimeSpan::from_secs(self * rhs.as_secs())
    }
}

impl MulAssign<f64> for TimeSpan {
    fn mul_assign(&mut self, rhs: f64) {
        *self = *self * rhs;
    }
}

impl Div<f64> for TimeSpan {
    type Output = Self;

    fn div(self, rhs: f64) -> Self::Output {
        Self::from_secs(self.as_secs() / rhs)
    }
}

impl DivAssign<f64> for TimeSpan {
    fn div_assign(&mut self, rhs: f64) {
        *self = *self / rhs;
    }
}

impl Div<TimeSpan> for TimeSpan {
    type Output = f64;

    fn div(self, rhs: TimeSpan) -> Self::Output {
        self.as_secs() / rhs.as_secs()
    }
}

impl fmt::Display for TimeSpan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} s", self.as_secs())
    }
}

// ---------------------------------------------------------------------------
// Temperature & TempDelta
// ---------------------------------------------------------------------------

const KELVIN_CELSIUS_OFFSET: f64 = 273.15;
const FAHRENHEIT_ABSOLUTE_ZERO_OFFSET: f64 = 459.67;

/// A temperature difference (relative interval) stored in kelvin (`K` / `°C`).
///
/// Unlike [`Temperature`], a `TempDelta` can be negative or zero and supports
/// full linear arithmetic.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Default)]
pub struct TempDelta(f64);

impl TempDelta {
    /// Zero temperature difference.
    pub const ZERO: Self = Self(0.0);

    /// Creates a `TempDelta` from a difference in kelvin.
    pub const fn from_kelvin(delta: f64) -> Self {
        Self(delta)
    }

    /// Creates a `TempDelta` from a difference in degrees Celsius (equal in magnitude to kelvin).
    pub const fn from_celsius(delta: f64) -> Self {
        Self(delta)
    }

    /// Creates a `TempDelta` from a difference in degrees Fahrenheit (`Δ°F × 5/9`).
    pub fn from_fahrenheit(delta: f64) -> Self {
        Self(delta * 5.0 / 9.0)
    }

    /// Returns this temperature difference in kelvin.
    pub const fn as_kelvin(self) -> f64 {
        self.0
    }

    /// Returns this temperature difference in degrees Celsius.
    pub const fn as_celsius(self) -> f64 {
        self.0
    }

    /// Returns this temperature difference in degrees Fahrenheit (`ΔK × 9/5`).
    pub fn as_fahrenheit(self) -> f64 {
        self.0 * 9.0 / 5.0
    }
}

impl_linear_quantity!(TempDelta, "K");

/// An absolute thermodynamic temperature stored in SI kelvin (`K`), guaranteed `>= 0 K`.
///
/// Because absolute temperatures form an affine space rather than a vector space:
/// - Subtracting two `Temperature` values yields a [`TempDelta`].
/// - Adding or subtracting a [`TempDelta`] shifts the `Temperature`.
///
/// # Examples
///
/// ```
/// use akku::units::{TempDelta, Temperature};
///
/// let t1 = Temperature::try_from_celsius(20.0).unwrap();
/// let t2 = Temperature::try_from_celsius(30.0).unwrap();
/// let delta: TempDelta = t2 - t1;
/// assert!((delta.as_fahrenheit() - 18.0).abs() < 1e-9);
/// assert!(Temperature::try_from_kelvin(-1.0).is_err());
/// ```
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct Temperature(f64);

impl Temperature {
    /// Absolute zero (`0 K`, `-273.15 °C`, `-459.67 °F`).
    pub const ABSOLUTE_ZERO: Self = Self(0.0);

    /// Fallibly creates a `Temperature` from kelvin, rejecting `< 0 K` or non-finite values.
    pub fn try_from_kelvin(kelvin: f64) -> Result<Self, UnitError> {
        if !kelvin.is_finite() {
            return Err(UnitError::NotFinite);
        }
        if kelvin < 0.0 {
            return Err(UnitError::BelowAbsoluteZero { kelvin });
        }
        let normalized = if kelvin == 0.0 { 0.0 } else { kelvin };
        Ok(Self(normalized))
    }

    /// Fallibly creates a `Temperature` from degrees Celsius, rejecting `< -273.15 °C`.
    pub fn try_from_celsius(celsius: f64) -> Result<Self, UnitError> {
        if !celsius.is_finite() {
            return Err(UnitError::NotFinite);
        }
        Self::try_from_kelvin(celsius + KELVIN_CELSIUS_OFFSET)
    }

    /// Fallibly creates a `Temperature` from degrees Fahrenheit, rejecting `< -459.67 °F`.
    pub fn try_from_fahrenheit(fahrenheit: f64) -> Result<Self, UnitError> {
        if !fahrenheit.is_finite() {
            return Err(UnitError::NotFinite);
        }
        Self::try_from_kelvin((fahrenheit + FAHRENHEIT_ABSOLUTE_ZERO_OFFSET) * 5.0 / 9.0)
    }

    /// Creates a `Temperature` from kelvin.
    ///
    /// # Panics
    ///
    /// Panics if `kelvin` is below `0.0` or non-finite. Use [`Self::try_from_kelvin`]
    /// for fallible construction.
    pub fn from_kelvin(kelvin: f64) -> Self {
        Self::try_from_kelvin(kelvin).expect("temperature must be finite and >= 0 K")
    }

    /// Creates a `Temperature` from degrees Celsius.
    ///
    /// # Panics
    ///
    /// Panics if `celsius` is below `-273.15` or non-finite. Use [`Self::try_from_celsius`]
    /// for fallible construction.
    pub fn from_celsius(celsius: f64) -> Self {
        Self::try_from_celsius(celsius).expect("temperature must be finite and >= -273.15 °C")
    }

    /// Creates a `Temperature` from degrees Fahrenheit.
    ///
    /// # Panics
    ///
    /// Panics if `fahrenheit` is below `-459.67` or non-finite. Use [`Self::try_from_fahrenheit`]
    /// for fallible construction.
    pub fn from_fahrenheit(fahrenheit: f64) -> Self {
        Self::try_from_fahrenheit(fahrenheit).expect("temperature must be finite and >= -459.67 °F")
    }

    /// Returns this temperature in kelvin (`K`).
    pub const fn as_kelvin(self) -> f64 {
        self.0
    }

    /// Returns this temperature in degrees Celsius (`°C`).
    pub fn as_celsius(self) -> f64 {
        self.0 - KELVIN_CELSIUS_OFFSET
    }

    /// Returns this temperature in degrees Fahrenheit (`°F`).
    pub fn as_fahrenheit(self) -> f64 {
        (self.0 - KELVIN_CELSIUS_OFFSET) * 9.0 / 5.0 + 32.0
    }

    /// Fallibly shifts this temperature by `delta`, rejecting results below absolute zero.
    pub fn checked_add(self, delta: TempDelta) -> Result<Self, UnitError> {
        Self::try_from_kelvin(self.0 + delta.as_kelvin())
    }

    /// Fallibly lowers this temperature by `delta`, rejecting results below absolute zero.
    pub fn checked_sub(self, delta: TempDelta) -> Result<Self, UnitError> {
        Self::try_from_kelvin(self.0 - delta.as_kelvin())
    }
}

impl Quantity for Temperature {
    fn si_value(self) -> f64 {
        self.0
    }

    fn from_si(value: f64) -> Self {
        Self::from_kelvin(value)
    }

    fn unit_symbol() -> &'static str {
        "K"
    }
}

impl Sub<Temperature> for Temperature {
    type Output = TempDelta;

    fn sub(self, rhs: Temperature) -> Self::Output {
        TempDelta::from_kelvin(self.0 - rhs.0)
    }
}

impl Add<TempDelta> for Temperature {
    type Output = Self;

    fn add(self, rhs: TempDelta) -> Self::Output {
        self.checked_add(rhs)
            .expect("temperature addition fell below absolute zero")
    }
}

impl Sub<TempDelta> for Temperature {
    type Output = Self;

    fn sub(self, rhs: TempDelta) -> Self::Output {
        self.checked_sub(rhs)
            .expect("temperature subtraction fell below absolute zero")
    }
}

impl fmt::Display for Temperature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} K", self.0)
    }
}

// ---------------------------------------------------------------------------
// Speed
// ---------------------------------------------------------------------------

const MPS_PER_KPH: f64 = 1_000.0 / 3_600.0;
const MPS_PER_MPH: f64 = METERS_PER_MILE / 3_600.0;
const MPS_PER_KNOT: f64 = METERS_PER_NAUTICAL_MILE / 3_600.0;
const MPS_PER_MACH_SEA_LEVEL: f64 = 340.29;

/// A physical speed stored in SI meters per second (`m/s`).
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Default)]
pub struct Speed(f64);

impl Speed {
    /// Zero speed.
    pub const ZERO: Self = Self(0.0);

    /// Creates a `Speed` from meters per second (`m/s`).
    pub const fn from_mps(value: f64) -> Self {
        Self(value)
    }

    /// Creates a `Speed` from kilometers per hour (`km/h`).
    pub fn from_kph(value: f64) -> Self {
        Self(value * MPS_PER_KPH)
    }

    /// Creates a `Speed` from miles per hour (`mph`).
    pub fn from_mph(value: f64) -> Self {
        Self(value * MPS_PER_MPH)
    }

    /// Creates a `Speed` from knots (nautical miles per hour).
    pub fn from_knots(value: f64) -> Self {
        Self(value * MPS_PER_KNOT)
    }

    /// Creates a `Speed` from sea-level Mach number (ISA standard `340.29 m/s`).
    pub fn from_mach_sea_level(value: f64) -> Self {
        Self(value * MPS_PER_MACH_SEA_LEVEL)
    }

    /// Returns this speed in meters per second (`m/s`).
    pub const fn as_mps(self) -> f64 {
        self.0
    }

    /// Returns this speed in kilometers per hour (`km/h`).
    pub fn as_kph(self) -> f64 {
        self.0 / MPS_PER_KPH
    }

    /// Returns this speed in miles per hour (`mph`).
    pub fn as_mph(self) -> f64 {
        self.0 / MPS_PER_MPH
    }

    /// Returns this speed in knots.
    pub fn as_knots(self) -> f64 {
        self.0 / MPS_PER_KNOT
    }

    /// Returns this speed as sea-level Mach number (`340.29 m/s`).
    pub fn as_mach_sea_level(self) -> f64 {
        self.0 / MPS_PER_MACH_SEA_LEVEL
    }
}

impl_linear_quantity!(Speed, "m/s");

// ---------------------------------------------------------------------------
// Energy
// ---------------------------------------------------------------------------

const JOULES_PER_KILOJOULE: f64 = 1_000.0;
const JOULES_PER_MEGAJOULE: f64 = 1_000_000.0;
const JOULES_PER_CALORIE: f64 = 4.184;
const JOULES_PER_KILOCALORIE: f64 = 4_184.0;
const JOULES_PER_WATT_HOUR: f64 = 3_600.0;
const JOULES_PER_KILOWATT_HOUR: f64 = 3_600_000.0;
const JOULES_PER_ELECTRONVOLT: f64 = 1.602_176_634e-19;
const SPEED_OF_LIGHT_MPS: f64 = 299_792_458.0;

/// A physical energy stored in SI joules (`J`).
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Default)]
pub struct Energy(f64);

impl Energy {
    /// Zero energy.
    pub const ZERO: Self = Self(0.0);

    /// Creates an `Energy` from joules (`J`).
    pub const fn from_joules(value: f64) -> Self {
        Self(value)
    }

    /// Creates an `Energy` from kilojoules (`kJ`).
    pub fn from_kilojoules(value: f64) -> Self {
        Self(value * JOULES_PER_KILOJOULE)
    }

    /// Creates an `Energy` from megajoules (`MJ`).
    pub fn from_megajoules(value: f64) -> Self {
        Self(value * JOULES_PER_MEGAJOULE)
    }

    /// Creates an `Energy` from thermochemical calories (`4.184 J`).
    pub fn from_calories(value: f64) -> Self {
        Self(value * JOULES_PER_CALORIE)
    }

    /// Creates an `Energy` from kilocalories (`4184 J`).
    pub fn from_kilocalories(value: f64) -> Self {
        Self(value * JOULES_PER_KILOCALORIE)
    }

    /// Creates an `Energy` from watt-hours (`3600 J`).
    pub fn from_watt_hours(value: f64) -> Self {
        Self(value * JOULES_PER_WATT_HOUR)
    }

    /// Creates an `Energy` from kilowatt-hours (`3.6e6 J`).
    pub fn from_kilowatt_hours(value: f64) -> Self {
        Self(value * JOULES_PER_KILOWATT_HOUR)
    }

    /// Creates an `Energy` from electronvolts (`1.602176634e-19 J`).
    pub fn from_electronvolts(value: f64) -> Self {
        Self(value * JOULES_PER_ELECTRONVOLT)
    }

    /// Computes classical kinetic energy `E = 1/2 * m * v^2`.
    pub fn kinetic(mass: Mass, speed: Speed) -> Self {
        let v = speed.as_mps();
        Self(0.5 * mass.as_kilograms() * v * v)
    }

    /// Computes relativistic rest mass-energy `E = m * c^2`.
    pub fn mass_energy(mass: Mass) -> Self {
        Self(mass.as_kilograms() * SPEED_OF_LIGHT_MPS * SPEED_OF_LIGHT_MPS)
    }

    /// Computes gravitational potential energy `E = m * g * h`.
    pub fn potential(mass: Mass, height: Length, gravity_mps2: f64) -> Self {
        Self(mass.as_kilograms() * gravity_mps2 * height.as_meters())
    }

    /// Returns this energy in joules (`J`).
    pub const fn as_joules(self) -> f64 {
        self.0
    }

    /// Returns this energy in kilojoules (`kJ`).
    pub fn as_kilojoules(self) -> f64 {
        self.0 / JOULES_PER_KILOJOULE
    }

    /// Returns this energy in megajoules (`MJ`).
    pub fn as_megajoules(self) -> f64 {
        self.0 / JOULES_PER_MEGAJOULE
    }

    /// Returns this energy in thermochemical calories.
    pub fn as_calories(self) -> f64 {
        self.0 / JOULES_PER_CALORIE
    }

    /// Returns this energy in kilocalories.
    pub fn as_kilocalories(self) -> f64 {
        self.0 / JOULES_PER_KILOCALORIE
    }

    /// Returns this energy in watt-hours (`Wh`).
    pub fn as_watt_hours(self) -> f64 {
        self.0 / JOULES_PER_WATT_HOUR
    }

    /// Returns this energy in kilowatt-hours (`kWh`).
    pub fn as_kilowatt_hours(self) -> f64 {
        self.0 / JOULES_PER_KILOWATT_HOUR
    }

    /// Returns this energy in electronvolts (`eV`).
    pub fn as_electronvolts(self) -> f64 {
        self.0 / JOULES_PER_ELECTRONVOLT
    }
}

impl_linear_quantity!(Energy, "J");

// ---------------------------------------------------------------------------
// Pressure
// ---------------------------------------------------------------------------

const PA_PER_KILOPASCAL: f64 = 1_000.0;
const PA_PER_MEGAPASCAL: f64 = 1_000_000.0;
const PA_PER_BAR: f64 = 100_000.0;
const PA_PER_MILLIBAR: f64 = 100.0;
const PA_PER_ATM: f64 = 101_325.0;
const PA_PER_PSI: f64 = 6_894.757_293_168_361;
const PA_PER_MMHG: f64 = 133.322_387_415;

/// A physical pressure stored in SI pascals (`Pa`).
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Default)]
pub struct Pressure(f64);

impl Pressure {
    /// Zero pressure.
    pub const ZERO: Self = Self(0.0);

    /// Creates a `Pressure` from pascals (`Pa`).
    pub const fn from_pascals(value: f64) -> Self {
        Self(value)
    }

    /// Creates a `Pressure` from kilopascals (`kPa`).
    pub fn from_kilopascals(value: f64) -> Self {
        Self(value * PA_PER_KILOPASCAL)
    }

    /// Creates a `Pressure` from megapascals (`MPa`).
    pub fn from_megapascals(value: f64) -> Self {
        Self(value * PA_PER_MEGAPASCAL)
    }

    /// Creates a `Pressure` from bars (`100_000 Pa`).
    pub fn from_bar(value: f64) -> Self {
        Self(value * PA_PER_BAR)
    }

    /// Creates a `Pressure` from millibars (`100 Pa`).
    pub fn from_millibar(value: f64) -> Self {
        Self(value * PA_PER_MILLIBAR)
    }

    /// Creates a `Pressure` from standard atmospheres (`101_325 Pa`).
    pub fn from_atm(value: f64) -> Self {
        Self(value * PA_PER_ATM)
    }

    /// Creates a `Pressure` from pounds per square inch (`psi`).
    pub fn from_psi(value: f64) -> Self {
        Self(value * PA_PER_PSI)
    }

    /// Creates a `Pressure` from millimeters of mercury (`mmHg`).
    pub fn from_mmhg(value: f64) -> Self {
        Self(value * PA_PER_MMHG)
    }

    /// Returns this pressure in pascals (`Pa`).
    pub const fn as_pascals(self) -> f64 {
        self.0
    }

    /// Returns this pressure in kilopascals (`kPa`).
    pub fn as_kilopascals(self) -> f64 {
        self.0 / PA_PER_KILOPASCAL
    }

    /// Returns this pressure in megapascals (`MPa`).
    pub fn as_megapascals(self) -> f64 {
        self.0 / PA_PER_MEGAPASCAL
    }

    /// Returns this pressure in bars.
    pub fn as_bar(self) -> f64 {
        self.0 / PA_PER_BAR
    }

    /// Returns this pressure in millibars.
    pub fn as_millibar(self) -> f64 {
        self.0 / PA_PER_MILLIBAR
    }

    /// Returns this pressure in standard atmospheres (`atm`).
    pub fn as_atm(self) -> f64 {
        self.0 / PA_PER_ATM
    }

    /// Returns this pressure in pounds per square inch (`psi`).
    pub fn as_psi(self) -> f64 {
        self.0 / PA_PER_PSI
    }

    /// Returns this pressure in millimeters of mercury (`mmHg`).
    pub fn as_mmhg(self) -> f64 {
        self.0 / PA_PER_MMHG
    }
}

impl_linear_quantity!(Pressure, "Pa");

// ---------------------------------------------------------------------------
// Dimensional operators
// ---------------------------------------------------------------------------

impl Div<TimeSpan> for Length {
    type Output = Speed;

    fn div(self, rhs: TimeSpan) -> Self::Output {
        Speed::from_mps(self.as_meters() / rhs.as_secs())
    }
}

impl Div<Speed> for Length {
    type Output = TimeSpan;

    fn div(self, rhs: Speed) -> Self::Output {
        TimeSpan::from_secs(self.as_meters() / rhs.as_mps())
    }
}

impl Mul<TimeSpan> for Speed {
    type Output = Length;

    fn mul(self, rhs: TimeSpan) -> Self::Output {
        Length::from_meters(self.as_mps() * rhs.as_secs())
    }
}

impl Mul<Speed> for TimeSpan {
    type Output = Length;

    fn mul(self, rhs: Speed) -> Self::Output {
        Length::from_meters(self.as_secs() * rhs.as_mps())
    }
}
