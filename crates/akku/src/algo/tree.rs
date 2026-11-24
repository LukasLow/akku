//! Range-query trees: Fenwick (binary indexed) trees and segment trees.
//!
//! - [`FenwickTree`]: `O(log n)` point updates and prefix/range sums over any
//!   [`Number`], with checked (overflow- and `NaN`-safe) arithmetic.
//! - [`SegmentTree`]: `O(log n)` point updates and range queries over any
//!   monoid (an associative operation with an identity), such as `min`,
//!   `max`, `gcd`, or (non-commutative) matrix/string concatenation.
//!
//! Both accept any [`RangeBounds<usize>`](std::ops::RangeBounds) (`a..b`,
//! `a..=b`, `..b`, `..`) and report invalid ranges as
//! [`Error::InvalidRange`] instead of panicking.
//!
//! # Examples
//!
//! ```
//! use akku::algo::tree::{FenwickTree, SegmentTree};
//!
//! let mut sums = FenwickTree::from_slice(&[5i64, 2, 7, 1])?;
//! sums.add(1, 10)?;
//! assert_eq!(sums.range_sum(1..3)?, 19);
//!
//! let mut mins = SegmentTree::from_slice(&[5, 2, 7, 1], i32::MAX, |a, b| *a.min(b));
//! assert_eq!(mins.query(0..3)?, 2);
//! mins.set(1, 9)?;
//! assert_eq!(mins.query(0..3)?, 5);
//! # Ok::<(), akku::algo::Error>(())
//! ```

use std::ops::RangeBounds;

use super::{resolve_range, Error, Number};

/// A Fenwick tree (binary indexed tree) of prefix sums.
///
/// Every update and query is checked: overflowing integer sums and
/// non-finite float sums return [`Error::Overflow`] and leave the tree
/// unchanged. For floats, repeated updates accumulate the usual rounding
/// error.
#[derive(Debug, Clone, PartialEq)]
pub struct FenwickTree<T> {
    /// 1-based Fenwick layout stored 0-based: node `k` lives at `tree[k - 1]`.
    tree: Vec<T>,
}

impl<T: Number> FenwickTree<T> {
    /// Creates a tree of `len` zeros.
    pub fn new(len: usize) -> Self {
        Self {
            tree: vec![T::ZERO; len],
        }
    }

    /// Builds a tree from `values` in `O(n)`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidValue`] for `NaN`/infinite values and
    /// [`Error::Overflow`] if a partial sum overflows.
    pub fn from_slice(values: &[T]) -> Result<Self, Error> {
        if values.iter().any(|v| !v.is_valid()) {
            return Err(Error::InvalidValue);
        }
        let mut tree = values.to_vec();
        let n = tree.len();
        for k in 1..=n {
            let parent = k + lowbit(k);
            if parent <= n {
                tree[parent - 1] = tree[parent - 1]
                    .checked_add(tree[k - 1])
                    .ok_or(Error::Overflow)?;
            }
        }
        Ok(Self { tree })
    }

    /// Returns the number of elements.
    pub fn len(&self) -> usize {
        self.tree.len()
    }

    /// Returns `true` if the tree has no elements.
    pub fn is_empty(&self) -> bool {
        self.tree.is_empty()
    }

    /// Adds `delta` to element `index`.
    ///
    /// # Errors
    ///
    /// - [`Error::IndexOutOfBounds`] if `index >= len`.
    /// - [`Error::InvalidValue`] if `delta` is `NaN` or infinite.
    /// - [`Error::Overflow`] if any affected sum overflows; the tree is left
    ///   unchanged.
    pub fn add(&mut self, index: usize, delta: T) -> Result<(), Error> {
        self.check(index)?;
        if !delta.is_valid() {
            return Err(Error::InvalidValue);
        }
        // Pass 1 verifies every affected node so a failure changes nothing.
        let mut k = index + 1;
        while k <= self.tree.len() {
            self.tree[k - 1].checked_add(delta).ok_or(Error::Overflow)?;
            k += lowbit(k);
        }
        let mut k = index + 1;
        while k <= self.tree.len() {
            if let Some(sum) = self.tree[k - 1].checked_add(delta) {
                self.tree[k - 1] = sum;
            }
            k += lowbit(k);
        }
        Ok(())
    }

    /// Sets element `index` to `value`.
    ///
    /// # Errors
    ///
    /// Same as [`add`](Self::add).
    pub fn set(&mut self, index: usize, value: T) -> Result<(), Error> {
        if !value.is_valid() {
            return Err(Error::InvalidValue);
        }
        let current = self.get(index)?;
        self.add(index, value.checked_sub(current).ok_or(Error::Overflow)?)
    }

    /// Returns element `index`.
    ///
    /// # Errors
    ///
    /// [`Error::IndexOutOfBounds`] if `index >= len`, or
    /// [`Error::Overflow`] if an intermediate sum overflows.
    pub fn get(&self, index: usize) -> Result<T, Error> {
        self.check(index)?;
        self.range_sum(index..=index)
    }

    /// Returns the sum of the first `end` elements (`0..end`).
    ///
    /// # Errors
    ///
    /// [`Error::InvalidRange`] if `end > len`, or [`Error::Overflow`].
    pub fn prefix_sum(&self, end: usize) -> Result<T, Error> {
        if end > self.tree.len() {
            return Err(Error::InvalidRange {
                start: 0,
                end,
                len: self.tree.len(),
            });
        }
        let mut sum = T::ZERO;
        let mut k = end;
        while k > 0 {
            sum = sum.checked_add(self.tree[k - 1]).ok_or(Error::Overflow)?;
            k &= k - 1;
        }
        Ok(sum)
    }

    /// Returns the sum of the elements in `range`.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidRange`] for reversed or out-of-bounds ranges, or
    /// [`Error::Overflow`].
    pub fn range_sum<R: RangeBounds<usize>>(&self, range: R) -> Result<T, Error> {
        let (start, end) = resolve_range(&range, self.tree.len())?;
        let total = self.prefix_sum(end)?;
        let before = self.prefix_sum(start)?;
        total.checked_sub(before).ok_or(Error::Overflow)
    }

    fn check(&self, index: usize) -> Result<(), Error> {
        if index < self.tree.len() {
            Ok(())
        } else {
            Err(Error::IndexOutOfBounds {
                index,
                len: self.tree.len(),
            })
        }
    }
}

/// Lowest set bit of `k` (`k > 0`).
#[inline]
fn lowbit(k: usize) -> usize {
    k & k.wrapping_neg()
}

/// An iterative, bottom-up segment tree over a monoid.
///
/// `op` must be associative and `identity` must satisfy
/// `op(identity, x) == x == op(x, identity)`; commutativity is **not**
/// required — queries combine elements strictly left to right.
///
/// # Examples
///
/// ```
/// use akku::algo::tree::SegmentTree;
///
/// // Non-commutative monoid: string concatenation.
/// let words = ["a", "b", "c", "d"].map(String::from);
/// let mut tree = SegmentTree::from_slice(&words, String::new(), |a, b| format!("{a}{b}"));
/// assert_eq!(tree.query(1..=3)?, "bcd");
/// tree.set(2, "X".to_string())?;
/// assert_eq!(tree.total(), "abXd");
/// assert!(tree.query(3..9).is_err());
/// # Ok::<(), akku::algo::Error>(())
/// ```
#[derive(Debug, Clone)]
pub struct SegmentTree<T, F> {
    len: usize,
    /// Number of leaves (a power of two `>= len`).
    leaves: usize,
    /// Heap layout: root at 1, children of `i` at `2i` and `2i + 1`.
    tree: Vec<T>,
    identity: T,
    op: F,
}

impl<T, F> SegmentTree<T, F>
where
    T: Clone,
    F: Fn(&T, &T) -> T,
{
    /// Creates a tree of `len` copies of `identity`.
    pub fn new(len: usize, identity: T, op: F) -> Self {
        Self::build(len, std::iter::empty(), identity, op)
    }

    /// Builds a tree from `values` in `O(n)`.
    pub fn from_slice(values: &[T], identity: T, op: F) -> Self {
        Self::build(values.len(), values.iter().cloned(), identity, op)
    }

    fn build(len: usize, values: impl Iterator<Item = T>, identity: T, op: F) -> Self {
        let leaves = len
            .max(1)
            .checked_next_power_of_two()
            .unwrap_or(1 << (usize::BITS - 1));
        let mut tree = vec![identity.clone(); leaves.saturating_mul(2)];
        for (slot, value) in tree.iter_mut().skip(leaves).zip(values) {
            *slot = value;
        }
        for i in (1..leaves).rev() {
            tree[i] = op(&tree[2 * i], &tree[2 * i + 1]);
        }
        Self {
            len,
            leaves,
            tree,
            identity,
            op,
        }
    }

    /// Returns the number of elements.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Returns `true` if the tree has no elements.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Returns the monoid identity.
    pub fn identity(&self) -> &T {
        &self.identity
    }

    /// Returns element `index`, or `None` if out of bounds.
    pub fn get(&self, index: usize) -> Option<&T> {
        if index < self.len {
            self.tree.get(self.leaves + index)
        } else {
            None
        }
    }

    /// Replaces element `index` with `value` in `O(log n)`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::IndexOutOfBounds`] if `index >= len`.
    pub fn set(&mut self, index: usize, value: T) -> Result<(), Error> {
        if index >= self.len {
            return Err(Error::IndexOutOfBounds {
                index,
                len: self.len,
            });
        }
        let mut i = self.leaves + index;
        self.tree[i] = value;
        while i > 1 {
            i /= 2;
            self.tree[i] = (self.op)(&self.tree[2 * i], &self.tree[2 * i + 1]);
        }
        Ok(())
    }

    /// Combines the elements in `range` left to right in `O(log n)`.
    ///
    /// An empty range yields the identity.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidRange`] for reversed or out-of-bounds ranges.
    pub fn query<R: RangeBounds<usize>>(&self, range: R) -> Result<T, Error> {
        let (start, end) = resolve_range(&range, self.len)?;
        let mut left = self.identity.clone();
        let mut right = self.identity.clone();
        let (mut l, mut r) = (start + self.leaves, end + self.leaves);
        while l < r {
            if l & 1 == 1 {
                left = (self.op)(&left, &self.tree[l]);
                l += 1;
            }
            if r & 1 == 1 {
                r -= 1;
                right = (self.op)(&self.tree[r], &right);
            }
            l /= 2;
            r /= 2;
        }
        Ok((self.op)(&left, &right))
    }

    /// Combines all elements (the root) in `O(1)`.
    pub fn total(&self) -> T {
        self.tree
            .get(1)
            .cloned()
            .unwrap_or_else(|| self.identity.clone())
    }
}
