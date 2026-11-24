//! Pure-Rust mathematics: fixed-point decimals, linear algebra, special functions,
//! and numerically stable descriptive statistics.
//!
//! `akku::math` is organized into four focused submodules with core types
//! re-exported at the domain root:
//!
//! - [`decimal`]: exact fixed-point [`Decimal`] arithmetic with 256-bit intermediate
//!   products/quotients and six [`RoundingMode`] strategies.
//! - [`linear`]: [`Vec2`], [`Vec3`], [`Vec4`], and dense [`Mat`] operations
//!   (determinant, linear system solver, matrix inverse).
//! - [`special`]: [`special::gamma`], [`special::ln_gamma`], [`special::beta`],
//!   [`special::erf`], [`special::erfc`], [`special::bessel_j0`],
//!   [`special::bessel_j1`], [`special::factorial`], and [`special::binomial`].
//! - [`stat`]: Welford-stable variance/covariance, NaN-safe order statistics,
//!   R-7 percentiles, Pearson correlation, and ordinary least-squares regression.
//!
//! # Examples
//!
//! ```
//! use akku::math::{Decimal, Vec3};
//!
//! let a: Decimal = "0.1".parse().unwrap();
//! let b: Decimal = "0.2".parse().unwrap();
//! assert_eq!(a + b, "0.3".parse().unwrap());
//!
//! let x = Vec3::new(1.0, 0.0, 0.0);
//! let y = Vec3::new(0.0, 1.0, 0.0);
//! assert_eq!(x.cross(y), Vec3::new(0.0, 0.0, 1.0));
//! ```

pub mod decimal;
pub mod linear;
pub mod special;
pub mod stat;

pub use decimal::{Decimal, DecimalError, RoundingMode};
pub use linear::{LinearError, Mat, Vec2, Vec3, Vec4};
