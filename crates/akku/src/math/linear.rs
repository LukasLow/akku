//! Euclidean vectors (`Vec2`, `Vec3`, `Vec4`) and dense row-major matrices (`Mat`).

use std::error::Error;
use std::fmt;
use std::ops::{Add, AddAssign, Div, DivAssign, Mul, MulAssign, Neg, Sub, SubAssign};

/// Errors produced by matrix and linear-algebra operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinearError {
    /// Matrix or vector dimensions are incompatible for the requested operation.
    DimensionMismatch,
    /// A square matrix is singular (non-invertible) within numerical precision.
    SingularMatrix,
    /// A row or column index is out of bounds.
    IndexOutOfBounds,
}

impl fmt::Display for LinearError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DimensionMismatch => f.write_str("incompatible matrix or vector dimensions"),
            Self::SingularMatrix => f.write_str("matrix is singular"),
            Self::IndexOutOfBounds => f.write_str("matrix index out of bounds"),
        }
    }
}

impl Error for LinearError {}

// ---------------------------------------------------------------------------
// Vec2, Vec3, Vec4
// ---------------------------------------------------------------------------

/// A 2-dimensional Euclidean vector of `f64` components.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Vec2 {
    /// X coordinate.
    pub x: f64,
    /// Y coordinate.
    pub y: f64,
}

impl Vec2 {
    /// Creates a new `Vec2`.
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    /// Returns the zero vector `(0.0, 0.0)`.
    pub const fn zero() -> Self {
        Self { x: 0.0, y: 0.0 }
    }

    /// Computes the dot product `self · other`.
    pub fn dot(self, other: Self) -> f64 {
        self.x * other.x + self.y * other.y
    }

    /// Computes the squared Euclidean norm `|self|^2`.
    pub fn length_sq(self) -> f64 {
        self.dot(self)
    }

    /// Computes the Euclidean norm `|self|`.
    pub fn length(self) -> f64 {
        self.length_sq().sqrt()
    }

    /// Returns a unit-length vector in the same direction, or `None` if zero or non-finite.
    pub fn normalize(self) -> Option<Self> {
        let len = self.length();
        if len == 0.0 || !len.is_finite() {
            None
        } else {
            Some(self / len)
        }
    }

    /// Computes the Euclidean distance between `self` and `other`.
    pub fn distance(self, other: Self) -> f64 {
        (self - other).length()
    }

    /// Linearly interpolates between `self` (`t = 0`) and `other` (`t = 1`).
    pub fn lerp(self, other: Self, t: f64) -> Self {
        self + (other - self) * t
    }
}

impl Add for Vec2 {
    type Output = Self;
    fn add(self, rhs: Self) -> Self::Output {
        Self::new(self.x + rhs.x, self.y + rhs.y)
    }
}

impl AddAssign for Vec2 {
    fn add_assign(&mut self, rhs: Self) {
        self.x += rhs.x;
        self.y += rhs.y;
    }
}

impl Sub for Vec2 {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self::Output {
        Self::new(self.x - rhs.x, self.y - rhs.y)
    }
}

impl SubAssign for Vec2 {
    fn sub_assign(&mut self, rhs: Self) {
        self.x -= rhs.x;
        self.y -= rhs.y;
    }
}

impl Neg for Vec2 {
    type Output = Self;
    fn neg(self) -> Self::Output {
        Self::new(-self.x, -self.y)
    }
}

impl Mul<f64> for Vec2 {
    type Output = Self;
    fn mul(self, rhs: f64) -> Self::Output {
        Self::new(self.x * rhs, self.y * rhs)
    }
}

impl Mul<Vec2> for f64 {
    type Output = Vec2;
    fn mul(self, rhs: Vec2) -> Self::Output {
        Vec2::new(self * rhs.x, self * rhs.y)
    }
}

impl MulAssign<f64> for Vec2 {
    fn mul_assign(&mut self, rhs: f64) {
        self.x *= rhs;
        self.y *= rhs;
    }
}

impl Div<f64> for Vec2 {
    type Output = Self;
    fn div(self, rhs: f64) -> Self::Output {
        Self::new(self.x / rhs, self.y / rhs)
    }
}

impl DivAssign<f64> for Vec2 {
    fn div_assign(&mut self, rhs: f64) {
        self.x /= rhs;
        self.y /= rhs;
    }
}

impl fmt::Display for Vec2 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "({}, {})", self.x, self.y)
    }
}

/// A 3-dimensional Euclidean vector of `f64` components.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Vec3 {
    /// X coordinate.
    pub x: f64,
    /// Y coordinate.
    pub y: f64,
    /// Z coordinate.
    pub z: f64,
}

impl Vec3 {
    /// Creates a new `Vec3`.
    pub const fn new(x: f64, y: f64, z: f64) -> Self {
        Self { x, y, z }
    }

    /// Returns the zero vector `(0.0, 0.0, 0.0)`.
    pub const fn zero() -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        }
    }

    /// Computes the dot product `self · other`.
    pub fn dot(self, other: Self) -> f64 {
        self.x * other.x + self.y * other.y + self.z * other.z
    }

    /// Computes the 3D cross product `self × other`.
    pub fn cross(self, other: Self) -> Self {
        Self {
            x: self.y * other.z - self.z * other.y,
            y: self.z * other.x - self.x * other.z,
            z: self.x * other.y - self.y * other.x,
        }
    }

    /// Computes the squared Euclidean norm `|self|^2`.
    pub fn length_sq(self) -> f64 {
        self.dot(self)
    }

    /// Computes the Euclidean norm `|self|`.
    pub fn length(self) -> f64 {
        self.length_sq().sqrt()
    }

    /// Returns a unit-length vector in the same direction, or `None` if zero or non-finite.
    pub fn normalize(self) -> Option<Self> {
        let len = self.length();
        if len == 0.0 || !len.is_finite() {
            None
        } else {
            Some(self / len)
        }
    }

    /// Computes the Euclidean distance between `self` and `other`.
    pub fn distance(self, other: Self) -> f64 {
        (self - other).length()
    }

    /// Linearly interpolates between `self` (`t = 0`) and `other` (`t = 1`).
    pub fn lerp(self, other: Self, t: f64) -> Self {
        self + (other - self) * t
    }
}

impl Add for Vec3 {
    type Output = Self;
    fn add(self, rhs: Self) -> Self::Output {
        Self::new(self.x + rhs.x, self.y + rhs.y, self.z + rhs.z)
    }
}

impl AddAssign for Vec3 {
    fn add_assign(&mut self, rhs: Self) {
        self.x += rhs.x;
        self.y += rhs.y;
        self.z += rhs.z;
    }
}

impl Sub for Vec3 {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self::Output {
        Self::new(self.x - rhs.x, self.y - rhs.y, self.z - rhs.z)
    }
}

impl SubAssign for Vec3 {
    fn sub_assign(&mut self, rhs: Self) {
        self.x -= rhs.x;
        self.y -= rhs.y;
        self.z -= rhs.z;
    }
}

impl Neg for Vec3 {
    type Output = Self;
    fn neg(self) -> Self::Output {
        Self::new(-self.x, -self.y, -self.z)
    }
}

impl Mul<f64> for Vec3 {
    type Output = Self;
    fn mul(self, rhs: f64) -> Self::Output {
        Self::new(self.x * rhs, self.y * rhs, self.z * rhs)
    }
}

impl Mul<Vec3> for f64 {
    type Output = Vec3;
    fn mul(self, rhs: Vec3) -> Self::Output {
        Vec3::new(self * rhs.x, self * rhs.y, self * rhs.z)
    }
}

impl MulAssign<f64> for Vec3 {
    fn mul_assign(&mut self, rhs: f64) {
        self.x *= rhs;
        self.y *= rhs;
        self.z *= rhs;
    }
}

impl Div<f64> for Vec3 {
    type Output = Self;
    fn div(self, rhs: f64) -> Self::Output {
        Self::new(self.x / rhs, self.y / rhs, self.z / rhs)
    }
}

impl DivAssign<f64> for Vec3 {
    fn div_assign(&mut self, rhs: f64) {
        self.x /= rhs;
        self.y /= rhs;
        self.z /= rhs;
    }
}

impl fmt::Display for Vec3 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "({}, {}, {})", self.x, self.y, self.z)
    }
}

/// A 4-dimensional Euclidean vector of `f64` components.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Vec4 {
    /// X coordinate.
    pub x: f64,
    /// Y coordinate.
    pub y: f64,
    /// Z coordinate.
    pub z: f64,
    /// W coordinate.
    pub w: f64,
}

impl Vec4 {
    /// Creates a new `Vec4`.
    pub const fn new(x: f64, y: f64, z: f64, w: f64) -> Self {
        Self { x, y, z, w }
    }

    /// Returns the zero vector `(0.0, 0.0, 0.0, 0.0)`.
    pub const fn zero() -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            z: 0.0,
            w: 0.0,
        }
    }

    /// Computes the dot product `self · other`.
    pub fn dot(self, other: Self) -> f64 {
        self.x * other.x + self.y * other.y + self.z * other.z + self.w * other.w
    }

    /// Computes the squared Euclidean norm `|self|^2`.
    pub fn length_sq(self) -> f64 {
        self.dot(self)
    }

    /// Computes the Euclidean norm `|self|`.
    pub fn length(self) -> f64 {
        self.length_sq().sqrt()
    }

    /// Returns a unit-length vector in the same direction, or `None` if zero or non-finite.
    pub fn normalize(self) -> Option<Self> {
        let len = self.length();
        if len == 0.0 || !len.is_finite() {
            None
        } else {
            Some(self / len)
        }
    }

    /// Computes the Euclidean distance between `self` and `other`.
    pub fn distance(self, other: Self) -> f64 {
        (self - other).length()
    }

    /// Linearly interpolates between `self` (`t = 0`) and `other` (`t = 1`).
    pub fn lerp(self, other: Self, t: f64) -> Self {
        self + (other - self) * t
    }
}

impl Add for Vec4 {
    type Output = Self;
    fn add(self, rhs: Self) -> Self::Output {
        Self::new(
            self.x + rhs.x,
            self.y + rhs.y,
            self.z + rhs.z,
            self.w + rhs.w,
        )
    }
}

impl AddAssign for Vec4 {
    fn add_assign(&mut self, rhs: Self) {
        self.x += rhs.x;
        self.y += rhs.y;
        self.z += rhs.z;
        self.w += rhs.w;
    }
}

impl Sub for Vec4 {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self::Output {
        Self::new(
            self.x - rhs.x,
            self.y - rhs.y,
            self.z - rhs.z,
            self.w - rhs.w,
        )
    }
}

impl SubAssign for Vec4 {
    fn sub_assign(&mut self, rhs: Self) {
        self.x -= rhs.x;
        self.y -= rhs.y;
        self.z -= rhs.z;
        self.w -= rhs.w;
    }
}

impl Neg for Vec4 {
    type Output = Self;
    fn neg(self) -> Self::Output {
        Self::new(-self.x, -self.y, -self.z, -self.w)
    }
}

impl Mul<f64> for Vec4 {
    type Output = Self;
    fn mul(self, rhs: f64) -> Self::Output {
        Self::new(self.x * rhs, self.y * rhs, self.z * rhs, self.w * rhs)
    }
}

impl Mul<Vec4> for f64 {
    type Output = Vec4;
    fn mul(self, rhs: Vec4) -> Self::Output {
        Vec4::new(self * rhs.x, self * rhs.y, self * rhs.z, self * rhs.w)
    }
}

impl MulAssign<f64> for Vec4 {
    fn mul_assign(&mut self, rhs: f64) {
        self.x *= rhs;
        self.y *= rhs;
        self.z *= rhs;
        self.w *= rhs;
    }
}

impl Div<f64> for Vec4 {
    type Output = Self;
    fn div(self, rhs: f64) -> Self::Output {
        Self::new(self.x / rhs, self.y / rhs, self.z / rhs, self.w / rhs)
    }
}

impl DivAssign<f64> for Vec4 {
    fn div_assign(&mut self, rhs: f64) {
        self.x /= rhs;
        self.y /= rhs;
        self.z /= rhs;
        self.w /= rhs;
    }
}

impl fmt::Display for Vec4 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "({}, {}, {}, {})", self.x, self.y, self.z, self.w)
    }
}

// ---------------------------------------------------------------------------
// Dense row-major Mat
// ---------------------------------------------------------------------------

/// A dense row-major `rows × cols` matrix of `f64` values.
///
/// # Examples
///
/// ```
/// use akku::math::Mat;
///
/// let a = Mat::new(2, 2, vec![4.0, 7.0, 2.0, 6.0]).unwrap();
/// assert!((a.det().unwrap() - 10.0).abs() < 1e-12);
///
/// let inv = a.inverse().unwrap();
/// let prod = a.mul(&inv).unwrap();
/// assert!((prod.get(0, 0).unwrap() - 1.0).abs() < 1e-12);
/// assert!((prod.get(0, 1).unwrap()).abs() < 1e-12);
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct Mat {
    rows: usize,
    cols: usize,
    data: Vec<f64>,
}

struct LuFactor {
    n: usize,
    lu: Vec<f64>,
    piv: Vec<usize>,
    sign: f64,
}

impl Mat {
    /// Creates a dense row-major matrix of shape `rows × cols`.
    ///
    /// Returns [`LinearError::DimensionMismatch`] if `data.len() != rows * cols`.
    pub fn new(rows: usize, cols: usize, data: Vec<f64>) -> Result<Self, LinearError> {
        if rows.checked_mul(cols) != Some(data.len()) {
            return Err(LinearError::DimensionMismatch);
        }
        Ok(Self { rows, cols, data })
    }

    /// Creates a `rows × cols` matrix filled with `0.0`.
    pub fn zeros(rows: usize, cols: usize) -> Self {
        let len = rows.checked_mul(cols).expect("matrix size overflow");
        Self {
            rows,
            cols,
            data: vec![0.0; len],
        }
    }

    /// Creates an `n × n` identity matrix.
    pub fn identity(n: usize) -> Self {
        let mut m = Self::zeros(n, n);
        for i in 0..n {
            m.data[i * n + i] = 1.0;
        }
        m
    }

    /// Returns the number of rows.
    pub const fn rows(&self) -> usize {
        self.rows
    }

    /// Returns the number of columns.
    pub const fn cols(&self) -> usize {
        self.cols
    }

    /// Returns the underlying row-major slice.
    pub fn as_slice(&self) -> &[f64] {
        &self.data
    }

    /// Returns the element at `(row, col)`, or `None` if out of bounds.
    pub fn get(&self, row: usize, col: usize) -> Option<f64> {
        if row < self.rows && col < self.cols {
            Some(self.data[row * self.cols + col])
        } else {
            None
        }
    }

    /// Sets the element at `(row, col)`.
    ///
    /// Returns [`LinearError::IndexOutOfBounds`] if `row >= rows` or `col >= cols`.
    pub fn set(&mut self, row: usize, col: usize, value: f64) -> Result<(), LinearError> {
        if row >= self.rows || col >= self.cols {
            return Err(LinearError::IndexOutOfBounds);
        }
        self.data[row * self.cols + col] = value;
        Ok(())
    }

    /// Returns the transpose `self^T` (`cols × rows`).
    pub fn transpose(&self) -> Self {
        let mut out = vec![0.0; self.data.len()];
        for r in 0..self.rows {
            for c in 0..self.cols {
                out[c * self.rows + r] = self.data[r * self.cols + c];
            }
        }
        Self {
            rows: self.cols,
            cols: self.rows,
            data: out,
        }
    }

    /// Element-wise matrix addition `self + other`.
    pub fn add(&self, other: &Self) -> Result<Self, LinearError> {
        if self.rows != other.rows || self.cols != other.cols {
            return Err(LinearError::DimensionMismatch);
        }
        let data = self
            .data
            .iter()
            .zip(&other.data)
            .map(|(&a, &b)| a + b)
            .collect();
        Ok(Self {
            rows: self.rows,
            cols: self.cols,
            data,
        })
    }

    /// Element-wise matrix subtraction `self - other`.
    pub fn sub(&self, other: &Self) -> Result<Self, LinearError> {
        if self.rows != other.rows || self.cols != other.cols {
            return Err(LinearError::DimensionMismatch);
        }
        let data = self
            .data
            .iter()
            .zip(&other.data)
            .map(|(&a, &b)| a - b)
            .collect();
        Ok(Self {
            rows: self.rows,
            cols: self.cols,
            data,
        })
    }

    /// Matrix multiplication `self * other`.
    ///
    /// Returns [`LinearError::DimensionMismatch`] if `self.cols() != other.rows()`.
    pub fn mul(&self, other: &Self) -> Result<Self, LinearError> {
        if self.cols != other.rows {
            return Err(LinearError::DimensionMismatch);
        }
        let mut out = vec![0.0; self.rows * other.cols];
        for i in 0..self.rows {
            let a_row = &self.data[i * self.cols..(i + 1) * self.cols];
            let out_row = &mut out[i * other.cols..(i + 1) * other.cols];
            for (k, &aik) in a_row.iter().enumerate() {
                if aik == 0.0 {
                    continue;
                }
                let b_row = &other.data[k * other.cols..(k + 1) * other.cols];
                for j in 0..other.cols {
                    out_row[j] += aik * b_row[j];
                }
            }
        }
        Ok(Self {
            rows: self.rows,
            cols: other.cols,
            data: out,
        })
    }

    /// Matrix-vector multiplication `self * v`.
    ///
    /// Returns [`LinearError::DimensionMismatch`] if `v.len() != self.cols()`.
    pub fn mul_vec(&self, v: &[f64]) -> Result<Vec<f64>, LinearError> {
        if v.len() != self.cols {
            return Err(LinearError::DimensionMismatch);
        }
        let mut out = vec![0.0; self.rows];
        for (i, slot) in out.iter_mut().enumerate() {
            let row = &self.data[i * self.cols..(i + 1) * self.cols];
            *slot = row.iter().zip(v).map(|(&a, &x)| a * x).sum();
        }
        Ok(out)
    }

    /// LU factorization with partial pivoting.
    fn lu_decompose(&self) -> Result<LuFactor, LinearError> {
        if self.rows != self.cols {
            return Err(LinearError::DimensionMismatch);
        }
        let n = self.rows;
        let mut lu = self.data.clone();
        let mut piv: Vec<usize> = (0..n).collect();
        let mut sign = 1.0;

        let max_entry = lu
            .iter()
            .map(|x| x.abs())
            .fold(0.0_f64, |acc, v| if v > acc { v } else { acc });
        let tol = 1e-12 * max_entry;

        for k in 0..n {
            // Find pivot row with largest absolute entry in column k.
            let mut pivot_row = k;
            let mut pivot_abs = lu[k * n + k].abs();
            for r in (k + 1)..n {
                let val = lu[r * n + k].abs();
                if val > pivot_abs {
                    pivot_abs = val;
                    pivot_row = r;
                }
            }

            if pivot_abs <= tol || !pivot_abs.is_finite() {
                return Err(LinearError::SingularMatrix);
            }

            if pivot_row != k {
                for c in 0..n {
                    lu.swap(k * n + c, pivot_row * n + c);
                }
                piv.swap(k, pivot_row);
                sign = -sign;
            }

            let pivot = lu[k * n + k];
            for r in (k + 1)..n {
                let factor = lu[r * n + k] / pivot;
                lu[r * n + k] = factor;
                for c in (k + 1)..n {
                    lu[r * n + c] -= factor * lu[k * n + c];
                }
            }
        }

        Ok(LuFactor { n, lu, piv, sign })
    }

    /// Computes the determinant via LU decomposition with partial pivoting.
    ///
    /// Returns [`LinearError::DimensionMismatch`] for non-square matrices.
    /// Returns `Ok(0.0)` for singular square matrices.
    pub fn det(&self) -> Result<f64, LinearError> {
        if self.rows != self.cols {
            return Err(LinearError::DimensionMismatch);
        }
        if self.rows == 0 {
            return Ok(1.0);
        }
        match self.lu_decompose() {
            Ok(fac) => {
                let mut prod = fac.sign;
                for i in 0..fac.n {
                    prod *= fac.lu[i * fac.n + i];
                }
                Ok(prod)
            }
            Err(LinearError::SingularMatrix) => Ok(0.0),
            Err(e) => Err(e),
        }
    }

    /// Solves the linear system `A * x = b` via LU decomposition with partial pivoting.
    ///
    /// Returns [`LinearError::DimensionMismatch`] if `self` is not square or `b.len() != rows`,
    /// and [`LinearError::SingularMatrix`] if `self` is singular.
    pub fn solve(&self, b: &[f64]) -> Result<Vec<f64>, LinearError> {
        if self.rows != self.cols || b.len() != self.rows {
            return Err(LinearError::DimensionMismatch);
        }
        let fac = self.lu_decompose()?;
        Ok(fac.solve_rhs(b))
    }

    /// Computes the matrix inverse `A^-1` via LU decomposition with partial pivoting.
    ///
    /// Returns [`LinearError::DimensionMismatch`] if `self` is not square,
    /// and [`LinearError::SingularMatrix`] if `self` is singular.
    pub fn inverse(&self) -> Result<Self, LinearError> {
        let fac = self.lu_decompose()?;
        let n = fac.n;
        let mut inv_data = vec![0.0; n * n];
        let mut e = vec![0.0; n];

        for col in 0..n {
            if col > 0 {
                e[col - 1] = 0.0;
            }
            e[col] = 1.0;
            let x = fac.solve_rhs(&e);
            for row in 0..n {
                inv_data[row * n + col] = x[row];
            }
        }

        Ok(Self {
            rows: n,
            cols: n,
            data: inv_data,
        })
    }
}

impl LuFactor {
    fn solve_rhs(&self, b: &[f64]) -> Vec<f64> {
        let n = self.n;
        // Apply permutation and forward substitution for L y = P b.
        let mut x = vec![0.0; n];
        for i in 0..n {
            let row = &self.lu[i * n..(i + 1) * n];
            let mut sum = b[self.piv[i]];
            for (&a, &xj) in row[..i].iter().zip(&x[..i]) {
                sum -= a * xj;
            }
            x[i] = sum;
        }
        // Back substitution for U x = y.
        for i in (0..n).rev() {
            let row = &self.lu[i * n..(i + 1) * n];
            let mut sum = x[i];
            for (&a, &xj) in row[(i + 1)..n].iter().zip(&x[(i + 1)..n]) {
                sum -= a * xj;
            }
            x[i] = sum / row[i];
        }
        x
    }
}

impl Mul for &Mat {
    type Output = Mat;

    fn mul(self, rhs: Self) -> Self::Output {
        self.mul(rhs)
            .expect("matrix multiplication dimension mismatch")
    }
}

impl Mul for Mat {
    type Output = Self;

    fn mul(self, rhs: Self) -> Self::Output {
        Mul::mul(&self, &rhs)
    }
}
