//! Checked numeric abstraction shared by the graph and tree algorithms.

use std::cmp::Ordering;
use std::fmt;

/// A numeric value usable as a graph edge weight or a Fenwick-tree element.
///
/// Implemented for every primitive integer and for `f32`/`f64`. All
/// arithmetic is checked: integer operations report overflow instead of
/// wrapping, and floating-point operations report non-finite results (`NaN`,
/// `±∞`) instead of silently propagating them.
///
/// # Examples
///
/// ```
/// use akku::algo::Number;
///
/// assert_eq!(Number::checked_add(250u8, 5), Some(255));
/// assert_eq!(Number::checked_add(250u8, 6), None);
/// assert_eq!(Number::checked_add(f64::MAX, f64::MAX), None);
/// assert!(!f64::NAN.is_valid());
/// assert!((-3i32).is_negative());
/// ```
pub trait Number: Copy + PartialOrd + fmt::Debug {
    /// The additive identity (`0`).
    const ZERO: Self;

    /// Adds `rhs`, returning `None` on overflow or a non-finite result.
    fn checked_add(self, rhs: Self) -> Option<Self>;

    /// Subtracts `rhs`, returning `None` on overflow or a non-finite result.
    fn checked_sub(self, rhs: Self) -> Option<Self>;

    /// Returns `true` if the value is strictly less than zero.
    fn is_negative(self) -> bool;

    /// Returns `true` if the value is usable in arithmetic.
    ///
    /// Always `true` for integers; `false` for `NaN` and infinite floats.
    fn is_valid(self) -> bool;

    /// A total ordering (IEEE 754 `totalOrder` for floats).
    fn total_cmp(&self, other: &Self) -> Ordering;
}

macro_rules! impl_int {
    (@negative unsigned, $value:ident) => {{
        let _ = $value;
        false
    }};
    (@negative signed, $value:ident) => {
        $value < 0
    };
    ($kind:ident => $($t:ty),*) => {$(
        impl Number for $t {
            const ZERO: Self = 0;

            #[inline]
            fn checked_add(self, rhs: Self) -> Option<Self> {
                <$t>::checked_add(self, rhs)
            }

            #[inline]
            fn checked_sub(self, rhs: Self) -> Option<Self> {
                <$t>::checked_sub(self, rhs)
            }

            #[inline]
            fn is_negative(self) -> bool {
                let value = self;
                impl_int!(@negative $kind, value)
            }

            #[inline]
            fn is_valid(self) -> bool {
                true
            }

            #[inline]
            fn total_cmp(&self, other: &Self) -> Ordering {
                Ord::cmp(self, other)
            }
        }
    )*};
}

impl_int!(unsigned => u8, u16, u32, u64, u128, usize);
impl_int!(signed => i8, i16, i32, i64, i128, isize);

macro_rules! impl_float {
    ($($t:ty),*) => {$(
        impl Number for $t {
            const ZERO: Self = 0.0;

            #[inline]
            fn checked_add(self, rhs: Self) -> Option<Self> {
                let sum = self + rhs;
                sum.is_finite().then_some(sum)
            }

            #[inline]
            fn checked_sub(self, rhs: Self) -> Option<Self> {
                let diff = self - rhs;
                diff.is_finite().then_some(diff)
            }

            #[inline]
            fn is_negative(self) -> bool {
                self < 0.0
            }

            #[inline]
            fn is_valid(self) -> bool {
                self.is_finite()
            }

            #[inline]
            fn total_cmp(&self, other: &Self) -> Ordering {
                <$t>::total_cmp(self, other)
            }
        }
    )*};
}

impl_float!(f32, f64);
