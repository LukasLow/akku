//! Exact fixed-point decimal arithmetic backed by `i128` mantissa and 256-bit
//! intermediate arithmetic.

use std::cmp::Ordering;
use std::error::Error;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::ops::{Add, Div, Mul, Neg, Rem, Sub};
use std::str::FromStr;

/// Maximum fractional decimal places supported by [`Decimal`].
pub const MAX_SCALE: u8 = 28;

const POW10_U128: [u128; 39] = {
    let mut table = [0u128; 39];
    let mut val = 1u128;
    let mut i = 0;
    while i < 39 {
        table[i] = val;
        if i < 38 {
            val *= 10;
        }
        i += 1;
    }
    table
};

/// Errors produced by [`Decimal`] construction, parsing, and arithmetic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecimalError {
    /// Parsing failed because the input was not a valid decimal number.
    InvalidFormat(&'static str),
    /// A scale exceeded [`MAX_SCALE`] (`28`).
    ScaleOutOfRange(u8),
    /// Division or remainder by zero.
    DivisionByZero,
    /// An arithmetic operation or conversion overflowed `i128`.
    Overflow,
    /// A floating-point input was NaN or infinite.
    NotFinite,
}

impl fmt::Display for DecimalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidFormat(msg) => write!(f, "invalid decimal format: {msg}"),
            Self::ScaleOutOfRange(s) => {
                write!(f, "decimal scale {s} exceeds maximum ({MAX_SCALE})")
            }
            Self::DivisionByZero => f.write_str("decimal division by zero"),
            Self::Overflow => f.write_str("decimal arithmetic overflow"),
            Self::NotFinite => f.write_str("cannot convert non-finite float to Decimal"),
        }
    }
}

impl Error for DecimalError {}

/// Rounding strategies for [`Decimal::round_dp`].
///
/// Behavior on halfway values (`2.5`, `3.5`, `-2.5`, `-3.5`) rounded to `0`
/// decimal places:
///
/// | Mode | `2.5` | `3.5` | `-2.5` | `-3.5` |
/// |------|-------|-------|--------|--------|
/// | [`HalfEven`](Self::HalfEven) | `2` | `4` | `-2` | `-4` |
/// | [`HalfUp`](Self::HalfUp) | `3` | `4` | `-3` | `-4` |
/// | [`HalfDown`](Self::HalfDown) | `2` | `3` | `-2` | `-3` |
/// | [`Floor`](Self::Floor) | `2` | `3` | `-3` | `-4` |
/// | [`Ceil`](Self::Ceil) | `3` | `4` | `-2` | `-3` |
/// | [`Trunc`](Self::Trunc) | `2` | `3` | `-2` | `-3` |
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RoundingMode {
    /// Round to nearest; ties round to the nearest even digit (banker's rounding).
    HalfEven,
    /// Round to nearest; ties round away from zero.
    HalfUp,
    /// Round to nearest; ties round toward zero.
    HalfDown,
    /// Round toward negative infinity.
    Floor,
    /// Round toward positive infinity.
    Ceil,
    /// Round toward zero (truncate dropped digits).
    Trunc,
}

// ---------------------------------------------------------------------------
// Private safe U256 helper for overflow-free intermediate calculations
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct U256([u64; 4]); // little-endian limbs

impl U256 {
    const ZERO: Self = Self([0, 0, 0, 0]);
    const ONE: Self = Self([1, 0, 0, 0]);

    const fn from_u128(v: u128) -> Self {
        Self([v as u64, (v >> 64) as u64, 0, 0])
    }

    const fn is_zero(self) -> bool {
        self.0[0] == 0 && self.0[1] == 0 && self.0[2] == 0 && self.0[3] == 0
    }

    const fn to_u128(self) -> Option<u128> {
        if self.0[2] == 0 && self.0[3] == 0 {
            Some((self.0[0] as u128) | ((self.0[1] as u128) << 64))
        } else {
            None
        }
    }

    const fn is_odd(self) -> bool {
        (self.0[0] & 1) == 1
    }

    fn bit(self, idx: usize) -> bool {
        let limb = idx / 64;
        let shift = idx % 64;
        ((self.0[limb] >> shift) & 1) == 1
    }

    fn set_bit(&mut self, idx: usize) {
        let limb = idx / 64;
        let shift = idx % 64;
        self.0[limb] |= 1u64 << shift;
    }

    fn bit_len(self) -> usize {
        for i in (0..4).rev() {
            if self.0[i] != 0 {
                return i * 64 + (64 - self.0[i].leading_zeros() as usize);
            }
        }
        0
    }

    fn shl1(self) -> Self {
        Self([
            self.0[0] << 1,
            (self.0[1] << 1) | (self.0[0] >> 63),
            (self.0[2] << 1) | (self.0[1] >> 63),
            (self.0[3] << 1) | (self.0[2] >> 63),
        ])
    }

    fn checked_add(self, rhs: Self) -> Option<Self> {
        let mut out = [0u64; 4];
        let mut carry = 0u128;
        for (i, slot) in out.iter_mut().enumerate() {
            let sum = u128::from(self.0[i]) + u128::from(rhs.0[i]) + carry;
            *slot = sum as u64;
            carry = sum >> 64;
        }
        if carry != 0 {
            None
        } else {
            Some(Self(out))
        }
    }

    fn wrapping_sub(self, rhs: Self) -> Self {
        let mut out = [0u64; 4];
        let mut borrow = 0u64;
        for (i, slot) in out.iter_mut().enumerate() {
            let (d1, b1) = self.0[i].overflowing_sub(rhs.0[i]);
            let (d2, b2) = d1.overflowing_sub(borrow);
            *slot = d2;
            borrow = u64::from(b1 || b2);
        }
        Self(out)
    }

    fn mul_u128(a: u128, b: u128) -> Self {
        let a0 = a as u64;
        let a1 = (a >> 64) as u64;
        let b0 = b as u64;
        let b1 = (b >> 64) as u64;

        let p00 = u128::from(a0) * u128::from(b0);
        let p01 = u128::from(a0) * u128::from(b1);
        let p10 = u128::from(a1) * u128::from(b0);
        let p11 = u128::from(a1) * u128::from(b1);

        let l0 = p00 as u64;

        let mid = (p00 >> 64) + (p01 & 0xFFFF_FFFF_FFFF_FFFF) + (p10 & 0xFFFF_FFFF_FFFF_FFFF);
        let l1 = mid as u64;

        let hi = (mid >> 64) + (p01 >> 64) + (p10 >> 64) + (p11 & 0xFFFF_FFFF_FFFF_FFFF);
        let l2 = hi as u64;

        let l3 = ((hi >> 64) + (p11 >> 64)) as u64;
        Self([l0, l1, l2, l3])
    }

    fn checked_mul_u64(self, rhs: u64) -> Option<Self> {
        let mut out = [0u64; 4];
        let mut carry = 0u128;
        let r = u128::from(rhs);
        for (i, slot) in out.iter_mut().enumerate() {
            let prod = u128::from(self.0[i]) * r + carry;
            *slot = prod as u64;
            carry = prod >> 64;
        }
        if carry != 0 {
            None
        } else {
            Some(Self(out))
        }
    }

    fn pow10(exp: u32) -> Option<Self> {
        if exp <= 38 {
            return Some(Self::from_u128(POW10_U128[exp as usize]));
        }
        let mut val = Self::from_u128(POW10_U128[38]);
        let mut rem = exp - 38;
        while rem > 0 {
            let step = rem.min(19);
            val = val.checked_mul_u64(POW10_U128[step as usize] as u64)?;
            rem -= step;
        }
        Some(val)
    }

    fn div_rem(self, divisor: Self) -> (Self, Self) {
        assert!(!divisor.is_zero(), "U256 division by zero");
        if self < divisor {
            return (Self::ZERO, self);
        }
        if let (Some(n), Some(d)) = (self.to_u128(), divisor.to_u128()) {
            return (Self::from_u128(n / d), Self::from_u128(n % d));
        }

        let top = self.bit_len();
        let mut q = Self::ZERO;
        let mut r = Self::ZERO;
        for bit in (0..top).rev() {
            let carry = r.bit(255);
            r = r.shl1();
            if self.bit(bit) {
                r.0[0] |= 1;
            }
            if carry || r >= divisor {
                r = r.wrapping_sub(divisor);
                q.set_bit(bit);
            }
        }
        (q, r)
    }
}

impl Ord for U256 {
    fn cmp(&self, other: &Self) -> Ordering {
        for i in (0..4).rev() {
            match self.0[i].cmp(&other.0[i]) {
                Ordering::Equal => continue,
                ord => return ord,
            }
        }
        Ordering::Equal
    }
}

impl PartialOrd for U256 {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

// ---------------------------------------------------------------------------
// Rounding & rescaling helpers
// ---------------------------------------------------------------------------

fn should_round_up(
    negative: bool,
    q_is_odd: bool,
    rem: U256,
    divisor: U256,
    mode: RoundingMode,
) -> bool {
    if rem.is_zero() {
        return false;
    }
    let half_cmp = rem.cmp(&divisor.wrapping_sub(rem));
    match mode {
        RoundingMode::Trunc => false,
        RoundingMode::Ceil => !negative,
        RoundingMode::Floor => negative,
        RoundingMode::HalfUp => half_cmp != Ordering::Less,
        RoundingMode::HalfDown => half_cmp == Ordering::Greater,
        RoundingMode::HalfEven => {
            half_cmp == Ordering::Greater || (half_cmp == Ordering::Equal && q_is_odd)
        }
    }
}

fn max_mag_for_sign(negative: bool) -> u128 {
    if negative {
        (i128::MAX as u128) + 1
    } else {
        i128::MAX as u128
    }
}

fn from_sign_mag_scale(negative: bool, mag: u128, scale: u8) -> Result<Decimal, DecimalError> {
    if scale > MAX_SCALE {
        return Err(DecimalError::ScaleOutOfRange(scale));
    }
    if mag == 0 {
        return Ok(Decimal { mantissa: 0, scale });
    }
    if negative {
        if mag == (i128::MAX as u128) + 1 {
            return Ok(Decimal {
                mantissa: i128::MIN,
                scale,
            });
        }
        if mag > i128::MAX as u128 {
            return Err(DecimalError::Overflow);
        }
        Ok(Decimal {
            mantissa: -(mag as i128),
            scale,
        })
    } else {
        if mag > i128::MAX as u128 {
            return Err(DecimalError::Overflow);
        }
        Ok(Decimal {
            mantissa: mag as i128,
            scale,
        })
    }
}

/// Rescales `(negative, value, scale)` into a valid `Decimal`, reducing scale
/// using `mode` rounding only when required to satisfy `scale <= MAX_SCALE` and
/// `value <= max_mag_for_sign(negative)`.
fn rescale_u256(
    negative: bool,
    value: U256,
    scale: u32,
    mode: RoundingMode,
) -> Result<Decimal, DecimalError> {
    if value.is_zero() {
        let s = scale.min(u32::from(MAX_SCALE)) as u8;
        return Ok(Decimal {
            mantissa: 0,
            scale: s,
        });
    }

    let max_mag = U256::from_u128(max_mag_for_sign(negative));
    let mut drop = scale.saturating_sub(u32::from(MAX_SCALE));

    loop {
        let (rounded, new_scale) = if drop == 0 {
            (value, scale as u8)
        } else {
            let Some(div) = U256::pow10(drop) else {
                // Dividing by > 10^77 yields 0 with non-zero remainder.
                let q = if should_round_up(negative, false, U256::ONE, U256::from_u128(10), mode) {
                    U256::ONE
                } else {
                    U256::ZERO
                };
                return from_sign_mag_scale(
                    negative,
                    q.to_u128().unwrap_or(0),
                    (scale - drop) as u8,
                );
            };
            let (mut q, rem) = value.div_rem(div);
            if should_round_up(negative, q.is_odd(), rem, div, mode) {
                q = q.checked_add(U256::ONE).ok_or(DecimalError::Overflow)?;
            }
            (q, (scale - drop) as u8)
        };

        if rounded <= max_mag {
            let mag = rounded.to_u128().ok_or(DecimalError::Overflow)?;
            return from_sign_mag_scale(negative, mag, new_scale);
        }

        if drop >= scale {
            return Err(DecimalError::Overflow);
        }
        drop += 1;
    }
}

// ---------------------------------------------------------------------------
// Decimal type
// ---------------------------------------------------------------------------

/// A fixed-point base-10 decimal number with `i128` mantissa and scale `0..=28`.
///
/// The numerical value is `mantissa * 10^(-scale)`. Equality ([`PartialEq`],
/// [`Eq`]), ordering ([`PartialOrd`], [`Ord`]), and hashing ([`Hash`]) are
/// **scale-normalized**, so `Decimal("1.0") == Decimal("1.00")` and both
/// produce identical hashes.
///
/// # Examples
///
/// ```
/// use akku::math::{Decimal, RoundingMode};
///
/// let a: Decimal = "0.1".parse().unwrap();
/// let b: Decimal = "0.2".parse().unwrap();
/// assert_eq!(a + b, "0.3".parse().unwrap());
///
/// let x: Decimal = "2.5".parse().unwrap();
/// assert_eq!(x.round_dp(0, RoundingMode::HalfEven), "2".parse().unwrap());
/// assert_eq!(x.round_dp(0, RoundingMode::HalfUp), "3".parse().unwrap());
/// ```
#[derive(Debug, Clone, Copy)]
pub struct Decimal {
    mantissa: i128,
    scale: u8,
}

impl Default for Decimal {
    fn default() -> Self {
        Self::ZERO
    }
}

impl Decimal {
    /// Additive identity (`0`).
    pub const ZERO: Self = Self {
        mantissa: 0,
        scale: 0,
    };

    /// Multiplicative identity (`1`).
    pub const ONE: Self = Self {
        mantissa: 1,
        scale: 0,
    };

    /// The constant `10`.
    pub const TEN: Self = Self {
        mantissa: 10,
        scale: 0,
    };

    /// Creates a `Decimal` from a signed `i128` mantissa and fractional scale (`0..=28`).
    pub const fn from_i128_with_scale(mantissa: i128, scale: u8) -> Result<Self, DecimalError> {
        if scale > MAX_SCALE {
            return Err(DecimalError::ScaleOutOfRange(scale));
        }
        Ok(Self { mantissa, scale })
    }

    /// Creates a `Decimal` from an `i64`.
    pub const fn from_i64(value: i64) -> Self {
        Self {
            mantissa: value as i128,
            scale: 0,
        }
    }

    /// Creates a `Decimal` from a `u64`.
    pub const fn from_u64(value: u64) -> Self {
        Self {
            mantissa: value as i128,
            scale: 0,
        }
    }

    /// Fallibly converts an `f64` into a `Decimal` using its shortest exact decimal form.
    ///
    /// Rejects NaN, infinities, and magnitudes outside the representable range.
    pub fn try_from_f64(value: f64) -> Result<Self, DecimalError> {
        if !value.is_finite() {
            return Err(DecimalError::NotFinite);
        }
        if value == 0.0 {
            return Ok(Self::ZERO);
        }
        format!("{value}").parse()
    }

    /// Returns the raw signed `i128` mantissa.
    pub const fn mantissa(&self) -> i128 {
        self.mantissa
    }

    /// Returns the fractional decimal scale (`0..=28`).
    pub const fn scale(&self) -> u8 {
        self.scale
    }

    /// Returns `true` if this decimal equals zero.
    pub const fn is_zero(&self) -> bool {
        self.mantissa == 0
    }

    /// Returns `true` if this decimal is strictly negative.
    pub const fn is_negative(&self) -> bool {
        self.mantissa < 0
    }

    /// Returns the absolute value of this decimal.
    ///
    /// # Panics
    ///
    /// Panics if `mantissa == i128::MIN`.
    pub fn abs(&self) -> Self {
        Self {
            mantissa: self.mantissa.abs(),
            scale: self.scale,
        }
    }

    /// Strips trailing fractional zeros, returning the canonical representation.
    ///
    /// Zero values always normalize to `(mantissa = 0, scale = 0)`.
    pub fn normalize(&self) -> Self {
        if self.mantissa == 0 {
            return Self::ZERO;
        }
        let mut m = self.mantissa;
        let mut s = self.scale;
        while s > 0 && m % 10 == 0 {
            m /= 10;
            s -= 1;
        }
        Self {
            mantissa: m,
            scale: s,
        }
    }

    /// Rounds this decimal to `dp` decimal places using the given [`RoundingMode`].
    ///
    /// If `dp >= self.scale()`, returns `self` unchanged.
    pub fn round_dp(&self, dp: u8, mode: RoundingMode) -> Self {
        if dp >= self.scale {
            return *self;
        }
        let drop = self.scale - dp;
        let divisor = U256::from_u128(POW10_U128[drop as usize]);
        let negative = self.mantissa < 0;
        let mag = U256::from_u128(self.mantissa.unsigned_abs());
        let (mut q, rem) = mag.div_rem(divisor);
        if should_round_up(negative, q.is_odd(), rem, divisor, mode) {
            q = q.checked_add(U256::ONE).expect("round_dp overflow");
        }
        let q_u128 = q.to_u128().expect("round_dp fits in u128");
        from_sign_mag_scale(negative, q_u128, dp).expect("round_dp fits in i128")
    }

    /// Checked addition using 256-bit intermediate scaling.
    pub fn checked_add(self, rhs: Self) -> Result<Self, DecimalError> {
        let target_scale = self.scale.max(rhs.scale);
        let a_mag = U256::mul_u128(
            self.mantissa.unsigned_abs(),
            POW10_U128[(target_scale - self.scale) as usize],
        );
        let b_mag = U256::mul_u128(
            rhs.mantissa.unsigned_abs(),
            POW10_U128[(target_scale - rhs.scale) as usize],
        );
        let a_neg = self.mantissa < 0;
        let b_neg = rhs.mantissa < 0;

        let (neg, mag) = if a_neg == b_neg {
            (
                a_neg,
                a_mag.checked_add(b_mag).ok_or(DecimalError::Overflow)?,
            )
        } else if a_mag >= b_mag {
            (a_neg, a_mag.wrapping_sub(b_mag))
        } else {
            (b_neg, b_mag.wrapping_sub(a_mag))
        };

        rescale_u256(neg, mag, u32::from(target_scale), RoundingMode::HalfEven)
    }

    /// Checked subtraction using 256-bit intermediate scaling.
    pub fn checked_sub(self, rhs: Self) -> Result<Self, DecimalError> {
        let target_scale = self.scale.max(rhs.scale);
        let a_mag = U256::mul_u128(
            self.mantissa.unsigned_abs(),
            POW10_U128[(target_scale - self.scale) as usize],
        );
        let b_mag = U256::mul_u128(
            rhs.mantissa.unsigned_abs(),
            POW10_U128[(target_scale - rhs.scale) as usize],
        );
        let a_neg = self.mantissa < 0;
        let b_neg = rhs.mantissa > 0; // flipped sign of rhs (0 is handled cleanly since b_mag == 0)

        let (neg, mag) = if rhs.mantissa == 0 {
            (a_neg, a_mag)
        } else if a_neg == b_neg {
            (
                a_neg,
                a_mag.checked_add(b_mag).ok_or(DecimalError::Overflow)?,
            )
        } else if a_mag >= b_mag {
            (a_neg, a_mag.wrapping_sub(b_mag))
        } else {
            (b_neg, b_mag.wrapping_sub(a_mag))
        };

        rescale_u256(neg, mag, u32::from(target_scale), RoundingMode::HalfEven)
    }

    /// Checked multiplication using exact 256-bit intermediate products.
    pub fn checked_mul(self, rhs: Self) -> Result<Self, DecimalError> {
        if self.is_zero() || rhs.is_zero() {
            let scale = self.scale.saturating_add(rhs.scale).min(MAX_SCALE);
            return Ok(Self { mantissa: 0, scale });
        }
        let neg = (self.mantissa < 0) ^ (rhs.mantissa < 0);
        let prod = U256::mul_u128(self.mantissa.unsigned_abs(), rhs.mantissa.unsigned_abs());
        let scale = u32::from(self.scale) + u32::from(rhs.scale);
        rescale_u256(neg, prod, scale, RoundingMode::HalfEven)
    }

    /// Checked division using exact 256-bit intermediate arithmetic.
    ///
    /// Returns [`DecimalError::DivisionByZero`] when `rhs` is zero.
    pub fn checked_div(self, rhs: Self) -> Result<Self, DecimalError> {
        if rhs.is_zero() {
            return Err(DecimalError::DivisionByZero);
        }
        if self.is_zero() {
            return Ok(Self::ZERO);
        }

        let neg = (self.mantissa < 0) ^ (rhs.mantissa < 0);
        let a = self.normalize();
        let b = rhs.normalize();
        let a_mag = a.mantissa.unsigned_abs();
        let b_mag = b.mantissa.unsigned_abs();

        let scale_diff = i32::from(a.scale) - i32::from(b.scale);
        let (num, mut out_scale): (U256, u32) = if scale_diff < 0 {
            let shift = (-scale_diff) as usize;
            (U256::mul_u128(a_mag, POW10_U128[shift]), 0)
        } else {
            (U256::from_u128(a_mag), scale_diff as u32)
        };

        let den = U256::from_u128(b_mag);
        let (mut q, mut rem) = num.div_rem(den);
        let max_mag = U256::from_u128(max_mag_for_sign(neg));

        // If integer quotient already exceeds i128 range, drop scale digits if available.
        if q > max_mag {
            if out_scale == 0 {
                return Err(DecimalError::Overflow);
            }
            return rescale_u256(neg, q, out_scale, RoundingMode::HalfEven);
        }

        // Extract fractional digits up to MAX_SCALE while staying within i128 capacity.
        while !rem.is_zero() && out_scale < u32::from(MAX_SCALE) {
            let Some(rem10) = rem.checked_mul_u64(10) else {
                break;
            };
            let (digit, next_rem) = rem10.div_rem(den);
            let Some(q10) = q.checked_mul_u64(10) else {
                break;
            };
            let Some(q_next) = q10.checked_add(digit) else {
                break;
            };
            if q_next > max_mag {
                break;
            }
            q = q_next;
            rem = next_rem;
            out_scale += 1;
        }

        // Round the final quotient digit with HalfEven using the leftover remainder.
        if !rem.is_zero() && should_round_up(neg, q.is_odd(), rem, den, RoundingMode::HalfEven) {
            if let Some(inc) = q.checked_add(U256::ONE) {
                if inc <= max_mag {
                    q = inc;
                } else if out_scale > 0 {
                    return rescale_u256(neg, inc, out_scale, RoundingMode::HalfEven);
                } else {
                    return Err(DecimalError::Overflow);
                }
            } else {
                return Err(DecimalError::Overflow);
            }
        }

        let mag = q.to_u128().ok_or(DecimalError::Overflow)?;
        from_sign_mag_scale(neg, mag, out_scale as u8)
    }

    /// Checked truncated remainder (`self % rhs`).
    ///
    /// Returns [`DecimalError::DivisionByZero`] when `rhs` is zero.
    pub fn checked_rem(self, rhs: Self) -> Result<Self, DecimalError> {
        if rhs.is_zero() {
            return Err(DecimalError::DivisionByZero);
        }
        if self.is_zero() {
            return Ok(Self::ZERO);
        }
        let target_scale = self.scale.max(rhs.scale);
        let a_scaled = U256::mul_u128(
            self.mantissa.unsigned_abs(),
            POW10_U128[(target_scale - self.scale) as usize],
        );
        let b_scaled = U256::mul_u128(
            rhs.mantissa.unsigned_abs(),
            POW10_U128[(target_scale - rhs.scale) as usize],
        );
        let (_, rem) = a_scaled.div_rem(b_scaled);
        rescale_u256(
            self.mantissa < 0,
            rem,
            u32::from(target_scale),
            RoundingMode::HalfEven,
        )
    }

    /// Converts this decimal to the nearest `f64`.
    pub fn to_f64(&self) -> f64 {
        self.to_string().parse::<f64>().unwrap_or(0.0)
    }
}

impl PartialEq for Decimal {
    fn eq(&self, other: &Self) -> bool {
        let a = self.normalize();
        let b = other.normalize();
        a.mantissa == b.mantissa && a.scale == b.scale
    }
}

impl Eq for Decimal {}

impl Ord for Decimal {
    fn cmp(&self, other: &Self) -> Ordering {
        let sa = self.mantissa.signum();
        let sb = other.mantissa.signum();
        match sa.cmp(&sb) {
            Ordering::Equal => {}
            ord => return ord,
        }
        if sa == 0 {
            return Ordering::Equal;
        }

        let lhs = U256::mul_u128(
            self.mantissa.unsigned_abs(),
            POW10_U128[other.scale as usize],
        );
        let rhs = U256::mul_u128(
            other.mantissa.unsigned_abs(),
            POW10_U128[self.scale as usize],
        );
        if sa > 0 {
            lhs.cmp(&rhs)
        } else {
            rhs.cmp(&lhs)
        }
    }
}

impl PartialOrd for Decimal {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Hash for Decimal {
    fn hash<H: Hasher>(&self, state: &mut H) {
        let n = self.normalize();
        n.mantissa.hash(state);
        n.scale.hash(state);
    }
}

impl Add for Decimal {
    type Output = Self;

    fn add(self, rhs: Self) -> Self::Output {
        self.checked_add(rhs).expect("decimal addition overflow")
    }
}

impl Sub for Decimal {
    type Output = Self;

    fn sub(self, rhs: Self) -> Self::Output {
        self.checked_sub(rhs).expect("decimal subtraction overflow")
    }
}

impl Mul for Decimal {
    type Output = Self;

    fn mul(self, rhs: Self) -> Self::Output {
        self.checked_mul(rhs)
            .expect("decimal multiplication overflow")
    }
}

impl Div for Decimal {
    type Output = Self;

    fn div(self, rhs: Self) -> Self::Output {
        self.checked_div(rhs)
            .expect("decimal division by zero or overflow")
    }
}

impl Rem for Decimal {
    type Output = Self;

    fn rem(self, rhs: Self) -> Self::Output {
        self.checked_rem(rhs)
            .expect("decimal remainder by zero or overflow")
    }
}

impl Neg for Decimal {
    type Output = Self;

    fn neg(self) -> Self::Output {
        Self {
            mantissa: self
                .mantissa
                .checked_neg()
                .expect("decimal negation overflow"),
            scale: self.scale,
        }
    }
}

impl fmt::Display for Decimal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.mantissa < 0 {
            f.write_str("-")?;
        }
        let digits = self.mantissa.unsigned_abs().to_string();
        let scale = usize::from(self.scale);
        if scale == 0 {
            f.write_str(&digits)
        } else if digits.len() <= scale {
            f.write_str("0.")?;
            for _ in 0..(scale - digits.len()) {
                f.write_str("0")?;
            }
            f.write_str(&digits)
        } else {
            let split = digits.len() - scale;
            write!(f, "{}.{}", &digits[..split], &digits[split..])
        }
    }
}

impl FromStr for Decimal {
    type Err = DecimalError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.is_empty() {
            return Err(DecimalError::InvalidFormat("empty string"));
        }

        let (negative, rest) = if let Some(r) = s.strip_prefix('-') {
            (true, r)
        } else if let Some(r) = s.strip_prefix('+') {
            (false, r)
        } else {
            (false, s)
        };

        if rest.is_empty() {
            return Err(DecimalError::InvalidFormat("missing digits"));
        }

        // Split optional exponent ('e' or 'E').
        let (mantissa_part, exp): (&str, i32) = if let Some(pos) = rest.find(['e', 'E']) {
            let exp_str = &rest[pos + 1..];
            if exp_str.is_empty() {
                return Err(DecimalError::InvalidFormat("missing exponent digits"));
            }
            let (exp_neg, exp_digits) = if let Some(r) = exp_str.strip_prefix('-') {
                (true, r)
            } else if let Some(r) = exp_str.strip_prefix('+') {
                (false, r)
            } else {
                (false, exp_str)
            };
            if exp_digits.is_empty() || !exp_digits.bytes().all(|b| b.is_ascii_digit()) {
                return Err(DecimalError::InvalidFormat("invalid exponent"));
            }
            let exp_abs: i32 = exp_digits.parse().map_err(|_| DecimalError::Overflow)?;
            let exp_val = if exp_neg {
                exp_abs.checked_neg().ok_or(DecimalError::Overflow)?
            } else {
                exp_abs
            };
            (&rest[..pos], exp_val)
        } else {
            (rest, 0)
        };

        // Split optional decimal point.
        let (int_part, frac_part) = if let Some(dot) = mantissa_part.find('.') {
            let after = &mantissa_part[dot + 1..];
            if after.contains('.') {
                return Err(DecimalError::InvalidFormat("multiple decimal points"));
            }
            (&mantissa_part[..dot], after)
        } else {
            (mantissa_part, "")
        };

        if int_part.is_empty() && frac_part.is_empty() {
            return Err(DecimalError::InvalidFormat("missing digits"));
        }
        if !int_part.bytes().all(|b| b.is_ascii_digit())
            || !frac_part.bytes().all(|b| b.is_ascii_digit())
        {
            return Err(DecimalError::InvalidFormat(
                "non-digit character in decimal",
            ));
        }

        let frac_len = i32::try_from(frac_part.len()).map_err(|_| DecimalError::Overflow)?;
        let mut effective_scale = frac_len.checked_sub(exp).ok_or(DecimalError::Overflow)?;

        // Collect all significant digits (skip leading zeros).
        let mut digits = String::with_capacity(int_part.len() + frac_part.len());
        digits.push_str(int_part);
        digits.push_str(frac_part);
        let trimmed_leading = digits.trim_start_matches('0');

        if trimmed_leading.is_empty() {
            let scale = effective_scale.clamp(0, i32::from(MAX_SCALE)) as u8;
            return Ok(Self { mantissa: 0, scale });
        }

        // Trim trailing zeros only while effective_scale > MAX_SCALE so inputs like
        // "1.00" preserve their explicit scale while "100e-30" still fits in scale 28.
        let mut sig = trimmed_leading;
        while effective_scale > i32::from(MAX_SCALE) && sig.ends_with('0') {
            sig = &sig[..sig.len() - 1];
            effective_scale -= 1;
        }

        if effective_scale > i32::from(MAX_SCALE) {
            return Err(DecimalError::ScaleOutOfRange(
                effective_scale.min(i32::from(u8::MAX)) as u8,
            ));
        }

        let mut mag: u128 = sig.parse().map_err(|_| DecimalError::Overflow)?;

        if effective_scale < 0 {
            let shift = (-effective_scale) as u32;
            if shift > 38 {
                return Err(DecimalError::Overflow);
            }
            mag = mag
                .checked_mul(POW10_U128[shift as usize])
                .ok_or(DecimalError::Overflow)?;
            effective_scale = 0;
        }

        from_sign_mag_scale(negative, mag, effective_scale as u8)
    }
}
