use std::collections::HashSet;
use std::f64::consts::PI;
use std::hash::{DefaultHasher, Hash, Hasher};

use akku::math::special::{
    bessel_j0, bessel_j1, beta, binomial, erf, erfc, factorial, gamma, ln_gamma,
};
use akku::math::stat::{
    covariance, linear_regression, max, mean, median, min, pearson_corr, percentile,
    sample_std_dev, sample_variance, std_dev, variance,
};
use akku::math::{Decimal, DecimalError, LinearError, Mat, RoundingMode, Vec2, Vec3, Vec4};

fn approx_eq(a: f64, b: f64, eps: f64) {
    assert!(
        (a - b).abs() <= eps,
        "expected {a} ≈ {b} (diff = {}, eps = {eps})",
        (a - b).abs()
    );
}

#[test]
fn decimal_exact_arithmetic_equality_and_hash() {
    let a: Decimal = "0.1".parse().unwrap();
    let b: Decimal = "0.2".parse().unwrap();
    let c: Decimal = "0.3".parse().unwrap();
    assert_eq!(a + b, c);
    assert_eq!((a + b).to_string(), "0.3");

    // Scale-normalized Eq, Ord, and Hash.
    let one_0: Decimal = "1.0".parse().unwrap();
    let one_00: Decimal = "1.00".parse().unwrap();
    let one: Decimal = Decimal::ONE;
    assert_eq!(one_0, one_00);
    assert_eq!(one_0, one);
    assert_eq!(one_00.normalize().scale(), 0);

    let hash_of = |d: Decimal| {
        let mut h = DefaultHasher::new();
        d.hash(&mut h);
        h.finish()
    };
    assert_eq!(hash_of(one_0), hash_of(one_00));
    assert_eq!(hash_of(one_0), hash_of(one));

    let mut set = HashSet::new();
    set.insert(one_0);
    set.insert(one_00);
    set.insert(one);
    assert_eq!(set.len(), 1);

    // Zero normalization across signs & scales.
    let z1: Decimal = "0.00".parse().unwrap();
    let z2: Decimal = "-0.0000".parse().unwrap();
    assert_eq!(z1, z2);
    assert_eq!(hash_of(z1), hash_of(z2));

    // Ordering across different scales.
    assert!("1.2".parse::<Decimal>().unwrap() < "1.25".parse::<Decimal>().unwrap());
    assert!("-1.25".parse::<Decimal>().unwrap() < "-1.2".parse::<Decimal>().unwrap());

    // Remainder, negation, abs, to_f64, try_from_f64, from_i64/from_u64.
    assert_eq!(
        "5.5"
            .parse::<Decimal>()
            .unwrap()
            .checked_rem(Decimal::from_i64(2)),
        Ok("1.5".parse().unwrap())
    );
    assert_eq!(
        "-5.5"
            .parse::<Decimal>()
            .unwrap()
            .checked_rem(Decimal::from_u64(2)),
        Ok("-1.5".parse().unwrap())
    );
    assert_eq!(
        (-"3.25".parse::<Decimal>().unwrap()).abs(),
        "3.25".parse().unwrap()
    );
    assert_eq!(
        Decimal::try_from_f64(0.125).unwrap(),
        "0.125".parse().unwrap()
    );
    approx_eq(
        "123.456".parse::<Decimal>().unwrap().to_f64(),
        123.456,
        1e-12,
    );
    assert_eq!(
        Decimal::try_from_f64(f64::NAN),
        Err(DecimalError::NotFinite)
    );
    assert_eq!(
        Decimal::try_from_f64(f64::INFINITY),
        Err(DecimalError::NotFinite)
    );
}

#[test]
fn decimal_all_six_rounding_modes_positive_and_negative_halfway() {
    let cases = [
        // (mode, 2.5, 3.5, -2.5, -3.5)
        (RoundingMode::HalfEven, "2", "4", "-2", "-4"),
        (RoundingMode::HalfUp, "3", "4", "-3", "-4"),
        (RoundingMode::HalfDown, "2", "3", "-2", "-3"),
        (RoundingMode::Floor, "2", "3", "-3", "-4"),
        (RoundingMode::Ceil, "3", "4", "-2", "-3"),
        (RoundingMode::Trunc, "2", "3", "-2", "-3"),
    ];

    let p25: Decimal = "2.5".parse().unwrap();
    let p35: Decimal = "3.5".parse().unwrap();
    let n25: Decimal = "-2.5".parse().unwrap();
    let n35: Decimal = "-3.5".parse().unwrap();

    for (mode, e_p25, e_p35, e_n25, e_n35) in cases {
        assert_eq!(
            p25.round_dp(0, mode),
            e_p25.parse().unwrap(),
            "{mode:?} 2.5"
        );
        assert_eq!(
            p35.round_dp(0, mode),
            e_p35.parse().unwrap(),
            "{mode:?} 3.5"
        );
        assert_eq!(
            n25.round_dp(0, mode),
            e_n25.parse().unwrap(),
            "{mode:?} -2.5"
        );
        assert_eq!(
            n35.round_dp(0, mode),
            e_n35.parse().unwrap(),
            "{mode:?} -3.5"
        );
    }

    // Non-halfway rounding checks for Floor / Ceil / Trunc.
    let p21: Decimal = "2.1".parse().unwrap();
    let n21: Decimal = "-2.1".parse().unwrap();
    assert_eq!(p21.round_dp(0, RoundingMode::Ceil), Decimal::from_i64(3));
    assert_eq!(p21.round_dp(0, RoundingMode::Floor), Decimal::from_i64(2));
    assert_eq!(n21.round_dp(0, RoundingMode::Ceil), Decimal::from_i64(-2));
    assert_eq!(n21.round_dp(0, RoundingMode::Floor), Decimal::from_i64(-3));
    assert_eq!(n21.round_dp(0, RoundingMode::Trunc), Decimal::from_i64(-2));
}

#[test]
fn decimal_u256_intermediates_division_by_zero_and_scientific_parsing() {
    // Large U256 intermediate multiplication that would overflow i128 without U256:
    // (10^20 * 10^-20) * (10^20 * 10^-20) = 1.000... (product of mantissas is 10^40 > i128::MAX).
    let a = Decimal::from_i128_with_scale(100_000_000_000_000_000_000, 20).unwrap(); // 1.0
    let b = Decimal::from_i128_with_scale(300_000_000_000_000_000_000, 20).unwrap(); // 3.0
    assert_eq!(a.checked_mul(b).unwrap(), Decimal::from_i64(3));

    // Large U256 intermediate division: 10^25 / 10^-10 = 10^35 (fits in i128!).
    let big: Decimal = "1e25".parse().unwrap();
    let tiny: Decimal = "1e-10".parse().unwrap();
    assert_eq!(big.checked_div(tiny).unwrap(), "1e35".parse().unwrap());

    // Exact repeating decimal precision (1 / 3 to 28 decimal places).
    let third = Decimal::ONE.checked_div(Decimal::from_i64(3)).unwrap();
    assert_eq!(third.to_string(), "0.3333333333333333333333333333");

    // Division and remainder by zero -> Err(DivisionByZero).
    assert_eq!(
        Decimal::TEN.checked_div(Decimal::ZERO),
        Err(DecimalError::DivisionByZero)
    );
    assert_eq!(
        Decimal::TEN.checked_rem(Decimal::ZERO),
        Err(DecimalError::DivisionByZero)
    );

    // Scientific notation and invalid string rejection.
    assert_eq!("1e-5".parse::<Decimal>().unwrap().to_string(), "0.00001");
    assert_eq!(
        "+2.5E+3".parse::<Decimal>().unwrap(),
        Decimal::from_i64(2500)
    );
    assert_eq!(
        "-3.75e-2".parse::<Decimal>().unwrap().to_string(),
        "-0.0375"
    );
    assert_eq!(
        "100e-30".parse::<Decimal>().unwrap(),
        "1e-28".parse().unwrap()
    );

    for bad in ["", "NaN", "inf", "-inf", "1.2.3", "1e", "abc", "12e-29"] {
        assert!(
            bad.parse::<Decimal>().is_err(),
            "expected error for {bad:?}"
        );
    }
    assert_eq!(
        Decimal::from_i128_with_scale(1, 29),
        Err(DecimalError::ScaleOutOfRange(29))
    );
}

#[test]
fn linear_vectors_and_cross_orthogonality() {
    let v2a = Vec2::new(3.0, 4.0);
    assert_eq!(v2a.length(), 5.0);
    assert_eq!(Vec2::zero().normalize(), None);
    let n2 = v2a.normalize().unwrap();
    approx_eq(n2.length(), 1.0, 1e-15);
    assert_eq!(v2a.lerp(Vec2::new(7.0, 8.0), 0.5), Vec2::new(5.0, 6.0));

    let a = Vec3::new(1.5, -2.0, 3.25);
    let b = Vec3::new(-4.0, 0.5, 2.0);
    let c = a.cross(b);
    // Cross product is orthogonal to both inputs.
    approx_eq(c.dot(a), 0.0, 1e-12);
    approx_eq(c.dot(b), 0.0, 1e-12);
    // Anti-commutativity.
    assert_eq!(b.cross(a), -c);

    let v4 = Vec4::new(1.0, 2.0, 3.0, 4.0);
    approx_eq(v4.distance(Vec4::zero()), 30.0_f64.sqrt(), 1e-15);
    approx_eq((v4 * 2.0).dot(Vec4::new(1.0, 0.0, 0.0, 1.0)), 10.0, 1e-15);
}

#[test]
fn linear_matrix_3x3_and_4x4_det_solve_inverse_and_errors() {
    // 3x3 invertible matrix
    let a3 = Mat::new(3, 3, vec![2.0, -1.0, 0.0, -1.0, 2.0, -1.0, 0.0, -1.0, 2.0]).unwrap();
    approx_eq(a3.det().unwrap(), 4.0, 1e-12);

    let b3 = [1.0, 0.0, 1.0];
    let x3 = a3.solve(&b3).unwrap();
    let ax3 = a3.mul_vec(&x3).unwrap();
    for i in 0..3 {
        approx_eq(ax3[i], b3[i], 1e-12);
    }

    let inv3 = a3.inverse().unwrap();
    let id3 = a3.mul(&inv3).unwrap();
    for r in 0..3 {
        for c in 0..3 {
            let expected = if r == c { 1.0 } else { 0.0 };
            approx_eq(id3.get(r, c).unwrap(), expected, 1e-12);
        }
    }

    // 4x4 matrix requiring partial pivoting (zero on leading diagonal!)
    let a4 = Mat::new(
        4,
        4,
        vec![
            0.0, 2.0, 1.0, -1.0, 3.0, -1.0, 2.0, 4.0, 1.0, 1.0, -3.0, 2.0, 2.0, 0.0, 1.0, -2.0,
        ],
    )
    .unwrap();
    let det4 = a4.det().unwrap();
    assert!(det4.abs() > 1.0);

    let b4 = [5.0, 11.0, -2.0, 1.0];
    let x4 = a4.solve(&b4).unwrap();
    let ax4 = a4.mul_vec(&x4).unwrap();
    for i in 0..4 {
        approx_eq(ax4[i], b4[i], 1e-12);
    }

    let inv4 = a4.inverse().unwrap();
    let id4 = (&a4) * (&inv4);
    for r in 0..4 {
        for c in 0..4 {
            let expected = if r == c { 1.0 } else { 0.0 };
            approx_eq(id4.get(r, c).unwrap(), expected, 1e-12);
        }
    }

    // Transpose & add/sub.
    let rect = Mat::new(2, 3, vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0]).unwrap();
    let rect_t = rect.transpose();
    assert_eq!((rect_t.rows(), rect_t.cols()), (3, 2));
    assert_eq!(rect_t.get(2, 1), Some(6.0));
    assert_eq!(rect.sub(&rect).unwrap(), Mat::zeros(2, 3));

    // Singular matrix -> LinearError::SingularMatrix.
    let singular = Mat::new(3, 3, vec![1.0, 2.0, 3.0, 2.0, 4.0, 6.0, 7.0, 8.0, 9.0]).unwrap();
    approx_eq(singular.det().unwrap(), 0.0, 1e-12);
    assert_eq!(singular.inverse(), Err(LinearError::SingularMatrix));
    assert_eq!(
        singular.solve(&[1.0, 2.0, 3.0]),
        Err(LinearError::SingularMatrix)
    );

    // Dimension mismatches -> LinearError::DimensionMismatch.
    assert_eq!(
        Mat::new(2, 2, vec![1.0, 2.0]),
        Err(LinearError::DimensionMismatch)
    );
    assert_eq!(rect.det(), Err(LinearError::DimensionMismatch));
    assert_eq!(rect.inverse(), Err(LinearError::DimensionMismatch));
    assert_eq!(rect.mul(&rect), Err(LinearError::DimensionMismatch));
    assert_eq!(rect.add(&rect_t), Err(LinearError::DimensionMismatch));
    assert_eq!(a3.solve(&[1.0, 2.0]), Err(LinearError::DimensionMismatch));
    assert_eq!(a3.mul_vec(&[1.0, 2.0]), Err(LinearError::DimensionMismatch));
}

#[test]
fn special_functions_and_exact_combinatorics() {
    // Gamma, ln_gamma, beta.
    approx_eq(gamma(0.5), PI.sqrt(), 1e-14);
    approx_eq(gamma(5.0), 24.0, 1e-12);
    approx_eq(gamma(-0.5), -2.0 * PI.sqrt(), 1e-13);
    assert!(gamma(0.0).is_nan());
    assert!(gamma(-3.0).is_nan());
    approx_eq(ln_gamma(5.0), 24.0_f64.ln(), 1e-14);
    assert!(ln_gamma(0.0).is_infinite());
    // B(2, 3) = 1! * 2! / 4! = 2 / 24 = 1/12
    approx_eq(beta(2.0, 3.0), 1.0 / 12.0, 1e-14);

    // erf & erfc (Cody approximation, no tail cancellation!).
    assert_eq!(erf(0.0), 0.0);
    assert_eq!(erfc(0.0), 1.0);
    approx_eq(erf(1.0), 0.842_700_792_949_714_9, 1e-14);
    approx_eq(erf(-1.0), -0.842_700_792_949_714_9, 1e-14);
    approx_eq(erf(0.25), 0.276_326_390_168_236_9, 1e-14);
    let erfc5 = erfc(5.0);
    let expected_erfc5 = 1.537_459_794_428_035e-12;
    assert!(
        ((erfc5 - expected_erfc5) / expected_erfc5).abs() < 1e-13,
        "erfc(5.0) = {erfc5:e}, expected {expected_erfc5:e}"
    );
    approx_eq(erfc(-5.0), 2.0 - expected_erfc5, 1e-14);

    // Bessel J0 & J1 (series + asymptotic regimes).
    assert_eq!(bessel_j0(0.0), 1.0);
    approx_eq(bessel_j0(1.0), 0.765_197_686_557_966_6, 1e-14);
    approx_eq(bessel_j0(2.404_825_557_7), 0.0, 1e-10);
    approx_eq(bessel_j0(-2.404_825_557_7), 0.0, 1e-10);
    // Asymptotic regime (x > 14): known J0(20.0) = 0.1670246643405832
    approx_eq(bessel_j0(20.0), 0.167_024_664_340_583_2, 1e-12);

    assert_eq!(bessel_j1(0.0), 0.0);
    approx_eq(bessel_j1(1.0), 0.440_050_585_744_933_55, 1e-14);
    approx_eq(bessel_j1(3.831_705_970_2), 0.0, 1e-10);
    approx_eq(bessel_j1(-1.0), -bessel_j1(1.0), 1e-15);
    // Asymptotic regime (x > 14): known J1(20.0) = 0.06683312417585004
    approx_eq(bessel_j1(20.0), 0.066_833_124_175_850_04, 1e-12);

    // Exact factorial & binomial.
    assert_eq!(factorial(0), Some(1));
    assert_eq!(factorial(20), Some(2_432_902_008_176_640_000));
    assert_eq!(
        factorial(34),
        Some(295_232_799_039_604_140_847_618_609_643_520_000_000)
    );
    assert_eq!(factorial(35), None);

    assert_eq!(binomial(60, 30), Some(118_264_581_564_861_424));
    assert_eq!(binomial(100, 0), Some(1));
    assert_eq!(binomial(100, 100), Some(1));
    assert_eq!(binomial(5, 6), Some(0));
}

#[test]
fn stat_welford_stability_order_stats_and_regression() {
    // Welford numerical stability on huge shifted data (naive E[x^2] - E[x]^2 catastrophically fails!).
    let shifted = [1e9 + 1.0, 1e9 + 2.0, 1e9 + 3.0];
    approx_eq(mean(&shifted).unwrap(), 1e9 + 2.0, 1e-9);
    approx_eq(variance(&shifted).unwrap(), 2.0 / 3.0, 1e-12);
    approx_eq(sample_variance(&shifted).unwrap(), 1.0, 1e-12);
    approx_eq(std_dev(&shifted).unwrap(), (2.0_f64 / 3.0).sqrt(), 1e-12);
    approx_eq(sample_std_dev(&shifted).unwrap(), 1.0, 1e-12);

    assert_eq!(mean(&[]), None);
    assert_eq!(variance(&[]), None);
    assert_eq!(sample_variance(&[42.0]), None);

    // Median & min/max: odd, even, empty, NaN-safe.
    assert_eq!(median(&[5.0, 1.0, 3.0]), Some(3.0));
    assert_eq!(median(&[4.0, 1.0, 3.0, 2.0]), Some(2.5));
    assert_eq!(median(&[]), None);
    assert_eq!(median(&[1.0, f64::NAN, 3.0]), None);
    assert_eq!(min(&[3.0, -2.0, 5.0]), Some(-2.0));
    assert_eq!(max(&[3.0, -2.0, 5.0]), Some(5.0));
    assert_eq!(min(&[1.0, f64::NAN]), None);
    assert_eq!(max(&[1.0, f64::NAN]), None);

    // R-7 linear interpolation percentiles.
    let xs = [10.0, 20.0, 30.0, 40.0];
    assert_eq!(percentile(&xs, 0.0), Some(10.0));
    assert_eq!(percentile(&xs, 25.0), Some(17.5));
    assert_eq!(percentile(&xs, 50.0), Some(25.0));
    assert_eq!(percentile(&xs, 75.0), Some(32.5));
    assert_eq!(percentile(&xs, 100.0), Some(40.0));
    assert_eq!(percentile(&xs, -1.0), None);
    assert_eq!(percentile(&xs, 101.0), None);
    assert_eq!(percentile(&[1.0, f64::NAN], 50.0), None);

    // Covariance, Pearson correlation, and linear regression on exact line y = 2x + 3.
    let x = [1.0, 2.0, 3.0, 4.0, 5.0];
    let y = [5.0, 7.0, 9.0, 11.0, 13.0];
    approx_eq(covariance(&x, &y).unwrap(), 5.0, 1e-12);
    approx_eq(pearson_corr(&x, &y).unwrap(), 1.0, 1e-12);
    let exact = linear_regression(&x, &y).unwrap();
    approx_eq(exact.slope, 2.0, 1e-12);
    approx_eq(exact.intercept, 3.0, 1e-12);
    approx_eq(exact.r_squared, 1.0, 1e-12);

    // Noisy data: verify exact OLS closed-form solution.
    // x = [1, 2, 3, 4], y = [2, 3, 5, 4]
    // mean_x = 2.5, mean_y = 3.5, Sxx = 5.0, Syy = 5.0, Sxy = 4.0
    // slope = 0.8, intercept = 1.5, r = 0.8, r^2 = 0.64
    let nx = [1.0, 2.0, 3.0, 4.0];
    let ny = [2.0, 3.0, 5.0, 4.0];
    approx_eq(pearson_corr(&nx, &ny).unwrap(), 0.8, 1e-12);
    let noisy = linear_regression(&nx, &ny).unwrap();
    approx_eq(noisy.slope, 0.8, 1e-12);
    approx_eq(noisy.intercept, 1.5, 1e-12);
    approx_eq(noisy.r_squared, 0.64, 1e-12);

    // Degenerate / mismatched series.
    assert_eq!(pearson_corr(&[1.0, 1.0], &[2.0, 3.0]), None);
    assert_eq!(linear_regression(&[1.0, 1.0], &[2.0, 3.0]), None);
    assert_eq!(covariance(&[1.0, 2.0], &[1.0]), None);
}
