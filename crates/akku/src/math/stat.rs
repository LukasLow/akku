//! Numerically stable descriptive statistics, order statistics, correlation,
//! and ordinary least-squares linear regression.

/// Result of an ordinary least-squares simple linear regression (`y = slope * x + intercept`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LinearRegression {
    /// Fitted slope (`β₁`).
    pub slope: f64,
    /// Fitted y-intercept (`β₀`).
    pub intercept: f64,
    /// Coefficient of determination (`R²`, in `0.0..=1.0`).
    pub r_squared: f64,
}

/// Computes the arithmetic mean via Welford's online recurrence.
///
/// Returns `None` if `data` is empty.
pub fn mean(data: &[f64]) -> Option<f64> {
    if data.is_empty() {
        return None;
    }
    let mut avg = 0.0;
    for (i, &x) in data.iter().enumerate() {
        avg += (x - avg) / ((i + 1) as f64);
    }
    Some(avg)
}

fn welford_mean_m2(data: &[f64]) -> Option<(f64, f64)> {
    if data.is_empty() {
        return None;
    }
    let mut avg = 0.0;
    let mut m2 = 0.0;
    for (i, &x) in data.iter().enumerate() {
        let n = (i + 1) as f64;
        let delta = x - avg;
        avg += delta / n;
        let delta2 = x - avg;
        m2 += delta * delta2;
    }
    Some((avg, m2))
}

/// Computes the population variance (`1 / n` denominator) using Welford's
/// numerically stable single-pass algorithm.
///
/// Returns `None` if `data` is empty.
///
/// # Examples
///
/// ```
/// use akku::math::stat::variance;
///
/// let shifted = [1e9 + 1.0, 1e9 + 2.0, 1e9 + 3.0];
/// assert!((variance(&shifted).unwrap() - (2.0 / 3.0)).abs() < 1e-12);
/// ```
pub fn variance(data: &[f64]) -> Option<f64> {
    let (_, m2) = welford_mean_m2(data)?;
    Some(m2 / (data.len() as f64))
}

/// Computes the Bessel-corrected sample variance (`1 / (n - 1)` denominator)
/// using Welford's algorithm.
///
/// Returns `None` if `data.len() < 2`.
pub fn sample_variance(data: &[f64]) -> Option<f64> {
    if data.len() < 2 {
        return None;
    }
    let (_, m2) = welford_mean_m2(data)?;
    Some(m2 / ((data.len() - 1) as f64))
}

/// Computes the population standard deviation (`sqrt(variance(data))`).
///
/// Returns `None` if `data` is empty.
pub fn std_dev(data: &[f64]) -> Option<f64> {
    variance(data).map(f64::sqrt)
}

/// Computes the Bessel-corrected sample standard deviation (`sqrt(sample_variance(data))`).
///
/// Returns `None` if `data.len() < 2`.
pub fn sample_std_dev(data: &[f64]) -> Option<f64> {
    sample_variance(data).map(f64::sqrt)
}

/// Returns the minimum value in `data`, or `None` if `data` is empty or contains `NaN`.
pub fn min(data: &[f64]) -> Option<f64> {
    if data.is_empty() {
        return None;
    }
    let mut best = data[0];
    if best.is_nan() {
        return None;
    }
    for &x in &data[1..] {
        if x.is_nan() {
            return None;
        }
        if x < best {
            best = x;
        }
    }
    Some(best)
}

/// Returns the maximum value in `data`, or `None` if `data` is empty or contains `NaN`.
pub fn max(data: &[f64]) -> Option<f64> {
    if data.is_empty() {
        return None;
    }
    let mut best = data[0];
    if best.is_nan() {
        return None;
    }
    for &x in &data[1..] {
        if x.is_nan() {
            return None;
        }
        if x > best {
            best = x;
        }
    }
    Some(best)
}

fn sorted_without_nan(data: &[f64]) -> Option<Vec<f64>> {
    if data.is_empty() || data.iter().any(|x| x.is_nan()) {
        return None;
    }
    let mut sorted = data.to_vec();
    sorted.sort_by(f64::total_cmp);
    Some(sorted)
}

/// Computes the median of `data`.
///
/// Returns `None` if `data` is empty or contains any `NaN`.
pub fn median(data: &[f64]) -> Option<f64> {
    let sorted = sorted_without_nan(data)?;
    let n = sorted.len();
    if n % 2 == 1 {
        Some(sorted[n / 2])
    } else {
        let a = sorted[n / 2 - 1];
        let b = sorted[n / 2];
        Some(a + (b - a) * 0.5)
    }
}

/// Computes the `p`-th percentile (`p` in `0.0..=100.0`) using R-7 linear interpolation
/// (matching NumPy's default and R `quantile(..., type = 7)`).
///
/// Returns `None` if `data` is empty, contains `NaN`, or `p` is outside `0.0..=100.0`.
pub fn percentile(data: &[f64], p: f64) -> Option<f64> {
    if !(0.0..=100.0).contains(&p) {
        return None;
    }
    let sorted = sorted_without_nan(data)?;
    let n = sorted.len();
    if n == 1 {
        return Some(sorted[0]);
    }
    let idx = (p / 100.0) * ((n - 1) as f64);
    let lo = idx.floor() as usize;
    let hi = idx.ceil() as usize;
    let frac = idx - (lo as f64);
    Some(sorted[lo] + frac * (sorted[hi] - sorted[lo]))
}

struct BivariateMoments {
    mean_x: f64,
    mean_y: f64,
    m2_x: f64,
    m2_y: f64,
    co_moment: f64,
}

fn bivariate_moments(x: &[f64], y: &[f64]) -> Option<BivariateMoments> {
    if x.len() != y.len() || x.len() < 2 {
        return None;
    }
    let mut mean_x = 0.0;
    let mut mean_y = 0.0;
    let mut m2_x = 0.0;
    let mut m2_y = 0.0;
    let mut co_moment = 0.0;

    for (i, (&xi, &yi)) in x.iter().zip(y.iter()).enumerate() {
        let n = (i + 1) as f64;
        let dx = xi - mean_x;
        let dy = yi - mean_y;
        mean_x += dx / n;
        mean_y += dy / n;
        let dx2 = xi - mean_x;
        let dy2 = yi - mean_y;
        m2_x += dx * dx2;
        m2_y += dy * dy2;
        co_moment += dx * dy2;
    }

    Some(BivariateMoments {
        mean_x,
        mean_y,
        m2_x,
        m2_y,
        co_moment,
    })
}

/// Computes the Bessel-corrected sample covariance (`1 / (n - 1)` denominator)
/// using Welford's online co-moment recurrence.
///
/// Returns `None` if `x.len() != y.len()` or `x.len() < 2`.
pub fn covariance(x: &[f64], y: &[f64]) -> Option<f64> {
    let m = bivariate_moments(x, y)?;
    Some(m.co_moment / ((x.len() - 1) as f64))
}

/// Computes the Pearson product-moment correlation coefficient `r ∈ [-1, 1]`.
///
/// Returns `None` if `x.len() != y.len()`, `x.len() < 2`, or either series has
/// zero variance.
pub fn pearson_corr(x: &[f64], y: &[f64]) -> Option<f64> {
    let m = bivariate_moments(x, y)?;
    if m.m2_x == 0.0 || m.m2_y == 0.0 {
        return None;
    }
    let r = m.co_moment / (m.m2_x * m.m2_y).sqrt();
    if r.is_finite() {
        Some(r.clamp(-1.0, 1.0))
    } else {
        Some(r)
    }
}

/// Fits an ordinary least-squares simple linear regression `y = slope * x + intercept`.
///
/// Returns `None` if `x.len() != y.len()`, `x.len() < 2`, or `x` has zero variance.
///
/// # Examples
///
/// ```
/// use akku::math::stat::linear_regression;
///
/// let x = [1.0, 2.0, 3.0, 4.0];
/// let y = [3.0, 5.0, 7.0, 9.0];
/// let fit = linear_regression(&x, &y).unwrap();
/// assert!((fit.slope - 2.0).abs() < 1e-12);
/// assert!((fit.intercept - 1.0).abs() < 1e-12);
/// assert!((fit.r_squared - 1.0).abs() < 1e-12);
/// ```
pub fn linear_regression(x: &[f64], y: &[f64]) -> Option<LinearRegression> {
    let m = bivariate_moments(x, y)?;
    if m.m2_x == 0.0 {
        return None;
    }
    let slope = m.co_moment / m.m2_x;
    let intercept = m.mean_y - slope * m.mean_x;
    let r_squared = if m.m2_y == 0.0 {
        1.0
    } else {
        let r2 = (m.co_moment * m.co_moment) / (m.m2_x * m.m2_y);
        if r2.is_finite() {
            r2.clamp(0.0, 1.0)
        } else {
            r2
        }
    };
    Some(LinearRegression {
        slope,
        intercept,
        r_squared,
    })
}
