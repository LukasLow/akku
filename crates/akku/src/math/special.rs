//! Special mathematical functions and exact combinatorics.
//!
//! Includes the Gamma, Beta, error (`erf` / `erfc`), Bessel (`J0` / `J1`),
//! exact factorial, and overflow-safe binomial coefficient functions.

use std::f64::consts::{FRAC_PI_4, PI};

const LANCZOS_G: f64 = 7.0;
const LANCZOS_COEFFS: [f64; 9] = [
    0.999_999_999_999_809_9,
    676.520_368_121_885_1,
    -1_259.139_216_722_402_8,
    771.323_428_777_653_1,
    -176.615_029_162_140_6,
    12.507_343_278_686_905,
    -0.138_571_095_265_720_12,
    9.984_369_578_019_572e-6,
    1.505_632_735_149_311_6e-7,
];

const SQRT_2PI: f64 = 2.506_628_274_631_000_5;
const LN_SQRT_2PI: f64 = 0.918_938_533_204_672_7;

/// Computes the Gamma function `Γ(x)` using the Lanczos approximation (`g = 7`, `n = 9`)
/// and Euler's reflection formula for `x < 0.5`.
///
/// Returns `f64::NAN` at non-positive integer poles (`0, -1, -2, ...`) and for `NaN`
/// or `-∞`. Returns `+∞` for `+∞` or overflow (`x > 171.624`).
///
/// # Examples
///
/// ```
/// use akku::math::special::gamma;
///
/// assert!((gamma(0.5) - std::f64::consts::PI.sqrt()).abs() < 1e-14);
/// assert!((gamma(5.0) - 24.0).abs() < 1e-12);
/// ```
pub fn gamma(x: f64) -> f64 {
    if x.is_nan() || x == f64::NEG_INFINITY {
        return f64::NAN;
    }
    if x == f64::INFINITY {
        return f64::INFINITY;
    }
    if x <= 0.0 && x.fract() == 0.0 {
        return f64::NAN;
    }
    if x < 0.5 {
        return PI / ((PI * x).sin() * gamma(1.0 - x));
    }
    if x > 140.0 {
        return ln_gamma(x).exp();
    }

    let z = x - 1.0;
    let mut sum = LANCZOS_COEFFS[0];
    for (i, &c) in LANCZOS_COEFFS.iter().enumerate().skip(1) {
        sum += c / (z + i as f64);
    }
    let t = z + LANCZOS_G + 0.5;
    SQRT_2PI * t.powf(z + 0.5) * (-t).exp() * sum
}

/// Computes the natural logarithm of the absolute value of the Gamma function,
/// `ln(|Γ(x)|)`.
///
/// Returns `+∞` at non-positive integer poles (`0, -1, -2, ...`) and for `±∞`.
/// Returns `f64::NAN` for `NaN`.
pub fn ln_gamma(x: f64) -> f64 {
    if x.is_nan() {
        return f64::NAN;
    }
    if x.is_infinite() {
        return f64::INFINITY;
    }
    if x <= 0.0 && x.fract() == 0.0 {
        return f64::INFINITY;
    }
    if x < 0.5 {
        return PI.ln() - (PI * x).sin().abs().ln() - ln_gamma(1.0 - x);
    }

    let z = x - 1.0;
    let mut sum = LANCZOS_COEFFS[0];
    for (i, &c) in LANCZOS_COEFFS.iter().enumerate().skip(1) {
        sum += c / (z + i as f64);
    }
    let t = z + LANCZOS_G + 0.5;
    LN_SQRT_2PI + (z + 0.5) * t.ln() - t + sum.ln()
}

/// Computes the Euler Beta function `B(a, b) = Γ(a) Γ(b) / Γ(a + b)`.
///
/// Uses [`ln_gamma`] for positive arguments to prevent intermediate overflow.
pub fn beta(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        return f64::NAN;
    }
    if a > 0.0 && b > 0.0 {
        (ln_gamma(a) + ln_gamma(b) - ln_gamma(a + b)).exp()
    } else {
        gamma(a) * gamma(b) / gamma(a + b)
    }
}

// ---------------------------------------------------------------------------
// Cody's rational Chebyshev approximations for erf and erfc (~1e-15 accuracy)
// W. J. Cody, "Rational Chebyshev Approximations for the Error Function",
// Mathematics of Computation, Vol. 23, No. 107 (1969), pp. 631-637.
// ---------------------------------------------------------------------------

const ERF_A: [f64; 5] = [
    3.161_123_743_870_565_6,
    1.138_641_541_510_501_6e2,
    3.774_852_376_853_02e2,
    3.209_377_589_138_469_5e3,
    1.857_777_061_846_031_5e-1,
];

const ERF_B: [f64; 4] = [
    2.360_129_095_234_412e1,
    2.440_246_379_344_441_7e2,
    1.282_616_526_077_372_3e3,
    2.844_236_833_439_171e3,
];

const ERFC_C: [f64; 9] = [
    5.641_884_969_886_701e-1,
    8.883_149_794_388_376,
    6.611_919_063_714_163e1,
    2.986_351_381_974_001e2,
    8.819_522_212_417_69e2,
    1.712_047_612_634_070_6e3,
    2.051_078_377_826_071_5e3,
    1.230_339_354_797_997_2e3,
    2.153_115_354_744_038_5e-8,
];

const ERFC_D: [f64; 8] = [
    1.574_492_611_070_983_5e1,
    1.176_939_508_913_125e2,
    5.371_811_018_620_099e2,
    1.621_389_574_566_690_2e3,
    3.290_799_235_733_459_6e3,
    4.362_619_090_143_247e3,
    3.439_367_674_143_721_6e3,
    1.230_339_354_803_749_4e3,
];

const ERFC_P: [f64; 6] = [
    3.053_266_349_612_323_4e-1,
    3.603_448_999_498_044_4e-1,
    1.257_817_261_112_292_5e-1,
    1.608_378_514_874_228e-2,
    6.587_491_615_298_378e-4,
    1.631_538_713_730_209_8e-2,
];

const ERFC_Q: [f64; 5] = [
    2.568_520_192_289_822,
    1.872_952_849_923_460_5,
    5.279_051_029_514_284e-1,
    6.051_834_131_244_132e-2,
    2.335_204_976_268_692e-3,
];

const INV_SQRT_PI: f64 = 0.564_189_583_547_756_3;

fn cody_erf_core(x: f64, compute_erfc: bool) -> f64 {
    if x.is_nan() {
        return f64::NAN;
    }
    let y = x.abs();

    if y <= 0.46875 {
        let ysq = if y < 1.11e-16 { 0.0 } else { y * y };
        let mut xnum = ERF_A[4] * ysq;
        let mut xden = ysq;
        for i in 0..3 {
            xnum = (xnum + ERF_A[i]) * ysq;
            xden = (xden + ERF_B[i]) * ysq;
        }
        let erf_val = x * (xnum + ERF_A[3]) / (xden + ERF_B[3]);
        if compute_erfc {
            1.0 - erf_val
        } else {
            erf_val
        }
    } else if y <= 4.0 {
        let mut xnum = ERFC_C[8] * y;
        let mut xden = y;
        for i in 0..7 {
            xnum = (xnum + ERFC_C[i]) * y;
            xden = (xden + ERFC_D[i]) * y;
        }
        let mut res = (xnum + ERFC_C[7]) / (xden + ERFC_D[7]);
        let ysq = (y * 16.0).trunc() / 16.0;
        let del = (y - ysq) * (y + ysq);
        res *= (-ysq * ysq).exp() * (-del).exp();

        if compute_erfc {
            if x < 0.0 {
                2.0 - res
            } else {
                res
            }
        } else {
            let erf_y = (0.5 - res) + 0.5;
            if x < 0.0 {
                -erf_y
            } else {
                erf_y
            }
        }
    } else {
        let res = if y >= 27.3 {
            0.0
        } else {
            let ysq = 1.0 / (y * y);
            let mut xnum = ERFC_P[5] * ysq;
            let mut xden = ysq;
            for i in 0..4 {
                xnum = (xnum + ERFC_P[i]) * ysq;
                xden = (xden + ERFC_Q[i]) * ysq;
            }
            let r = ysq * (xnum + ERFC_P[4]) / (xden + ERFC_Q[4]);
            let r = (INV_SQRT_PI - r) / y;
            let y_hi = (y * 16.0).trunc() / 16.0;
            let del = (y - y_hi) * (y + y_hi);
            (-y_hi * y_hi).exp() * (-del).exp() * r
        };

        if compute_erfc {
            if x < 0.0 {
                2.0 - res
            } else {
                res
            }
        } else {
            let erf_y = (0.5 - res) + 0.5;
            if x < 0.0 {
                -erf_y
            } else {
                erf_y
            }
        }
    }
}

/// Computes the Gauss error function `erf(x) = (2 / √π) ∫₀ˣ e^(-t²) dt`.
///
/// Uses Cody's rational Chebyshev approximation (~1e-15 relative accuracy).
///
/// # Examples
///
/// ```
/// use akku::math::special::erf;
///
/// assert_eq!(erf(0.0), 0.0);
/// assert!((erf(1.0) - 0.8427007929497149).abs() < 1e-14);
/// ```
pub fn erf(x: f64) -> f64 {
    cody_erf_core(x, false)
}

/// Computes the complementary error function `erfc(x) = 1 - erf(x)`.
///
/// Uses Cody's rational Chebyshev approximation directly for `|x| > 0.46875`,
/// avoiding catastrophic cancellation in the tail (for example, `erfc(5.0)`).
///
/// # Examples
///
/// ```
/// use akku::math::special::erfc;
///
/// let val = erfc(5.0);
/// assert!((val - 1.537459794428035e-12).abs() < 1e-24);
/// ```
pub fn erfc(x: f64) -> f64 {
    cody_erf_core(x, true)
}

// ---------------------------------------------------------------------------
// Bessel functions J0 and J1 (power series + Hankel asymptotic expansion)
// ---------------------------------------------------------------------------

fn bessel_hankel(x: f64, order: u32) -> f64 {
    let mu = 4.0 * f64::from(order * order);
    let mut p = 1.0;
    let mut q = 0.0;

    let mut term = 1.0;
    let mut prev_abs = f64::INFINITY;

    for m in 1..=64 {
        let mf = f64::from(m);
        let odd = 2.0 * mf - 1.0;
        let next = term * (mu - odd * odd) / (mf * 8.0 * x);
        let next_abs = next.abs();
        if next_abs > prev_abs {
            break;
        }
        term = next;
        prev_abs = next_abs;

        if m % 2 == 0 {
            let k = m / 2;
            if k % 2 == 1 {
                p -= term;
            } else {
                p += term;
            }
        } else {
            let k = (m - 1) / 2;
            if k % 2 == 1 {
                q -= term;
            } else {
                q += term;
            }
        }

        if next_abs < 1e-16 {
            break;
        }
    }

    let phase = x - (2.0 * f64::from(order) + 1.0) * FRAC_PI_4;
    (2.0 / (PI * x)).sqrt() * (p * phase.cos() - q * phase.sin())
}

/// Computes the Bessel function of the first kind of order 0, `J₀(x)`.
///
/// Uses the Frobenius power series for `|x| <= 14.0` and the Hankel asymptotic
/// expansion for `|x| > 14.0`.
///
/// # Examples
///
/// ```
/// use akku::math::special::bessel_j0;
///
/// assert_eq!(bessel_j0(0.0), 1.0);
/// assert!(bessel_j0(2.404825557695773).abs() < 1e-12);
/// ```
pub fn bessel_j0(x: f64) -> f64 {
    if x.is_nan() {
        return f64::NAN;
    }
    if x.is_infinite() {
        return 0.0;
    }
    let ax = x.abs();
    if ax == 0.0 {
        return 1.0;
    }
    if ax <= 14.0 {
        let neg_quarter_x2 = -0.25 * ax * ax;
        let mut sum = 1.0;
        let mut term = 1.0;
        for k in 1..=100 {
            let kf = f64::from(k);
            term *= neg_quarter_x2 / (kf * kf);
            sum += term;
            if term.abs() <= sum.abs() * 1e-16 {
                break;
            }
        }
        sum
    } else {
        bessel_hankel(ax, 0)
    }
}

/// Computes the Bessel function of the first kind of order 1, `J₁(x)`.
///
/// Uses the Frobenius power series for `|x| <= 14.0` and the Hankel asymptotic
/// expansion for `|x| > 14.0`.
///
/// # Examples
///
/// ```
/// use akku::math::special::bessel_j1;
///
/// assert_eq!(bessel_j1(0.0), 0.0);
/// assert!(bessel_j1(3.8317059702075125).abs() < 1e-12);
/// ```
pub fn bessel_j1(x: f64) -> f64 {
    if x.is_nan() {
        return f64::NAN;
    }
    if x.is_infinite() {
        return 0.0;
    }
    if x == 0.0 {
        return 0.0;
    }
    let sign = x.signum();
    let ax = x.abs();
    let mag = if ax <= 14.0 {
        let neg_quarter_x2 = -0.25 * ax * ax;
        let mut term = 0.5 * ax;
        let mut sum = term;
        for k in 1..=100 {
            let kf = f64::from(k);
            term *= neg_quarter_x2 / (kf * (kf + 1.0));
            sum += term;
            if term.abs() <= sum.abs() * 1e-16 {
                break;
            }
        }
        sum
    } else {
        bessel_hankel(ax, 1)
    };
    sign * mag
}

// ---------------------------------------------------------------------------
// Exact combinatorics
// ---------------------------------------------------------------------------

/// Computes the exact factorial `n!` as `u128`, returning `None` if `n > 34`.
///
/// # Examples
///
/// ```
/// use akku::math::special::factorial;
///
/// assert_eq!(factorial(0), Some(1));
/// assert_eq!(factorial(20), Some(2_432_902_008_176_640_000));
/// assert_eq!(factorial(35), None);
/// ```
pub const fn factorial(n: u32) -> Option<u128> {
    let mut acc: u128 = 1;
    let mut i: u32 = 2;
    while i <= n {
        match acc.checked_mul(i as u128) {
            Some(v) => acc = v,
            None => return None,
        }
        i += 1;
    }
    Some(acc)
}

const fn gcd_u128(mut a: u128, mut b: u128) -> u128 {
    while b != 0 {
        let t = a % b;
        a = b;
        b = t;
    }
    a
}

/// Computes the exact binomial coefficient `C(n, k)` as `u128`, reducing factors
/// by their greatest common divisor prior to multiplication to avoid premature
/// intermediate overflow.
///
/// Returns `Some(0)` if `k > n`, and `None` if the final result overflows `u128`.
///
/// # Examples
///
/// ```
/// use akku::math::special::binomial;
///
/// assert_eq!(binomial(60, 30), Some(118_264_581_564_861_424));
/// assert_eq!(binomial(5, 7), Some(0));
/// ```
pub const fn binomial(n: u64, k: u64) -> Option<u128> {
    if k > n {
        return Some(0);
    }
    let k = if k <= n - k { k } else { n - k };
    if k == 0 {
        return Some(1);
    }

    let mut acc: u128 = 1;
    let mut i: u64 = 1;
    while i <= k {
        let mut num = (n - k + i) as u128;
        let mut den = i as u128;

        let g1 = gcd_u128(num, den);
        num /= g1;
        den /= g1;

        // Because acc * num / den is guaranteed integral and gcd(num, den) == 1,
        // den always divides acc evenly.
        acc /= den;
        match acc.checked_mul(num) {
            Some(v) => acc = v,
            None => return None,
        }
        i += 1;
    }
    Some(acc)
}
