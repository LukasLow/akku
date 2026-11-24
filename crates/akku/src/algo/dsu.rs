//! Disjoint-set union (union-find).
//!
//! [`UnionFind`] tracks a partition of `0..len` into disjoint sets with
//! path halving and union by size, giving effectively constant
//! (inverse-Ackermann) amortized time per operation. All operations are
//! iterative and bounds-checked.
//!
//! # Examples
//!
//! ```
//! use akku::algo::dsu::UnionFind;
//!
//! let mut sets = UnionFind::new(5);
//! assert!(sets.union(0, 1)?);
//! assert!(sets.union(3, 4)?);
//! assert!(!sets.union(1, 0)?); // already joined
//!
//! assert!(sets.connected(0, 1)?);
//! assert!(!sets.connected(1, 3)?);
//! assert_eq!(sets.set_count(), 3);
//! assert!(sets.find(9).is_err());
//! # Ok::<(), akku::algo::Error>(())
//! ```

use super::Error;

/// A disjoint-set forest over the elements `0..len`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UnionFind {
    parent: Vec<usize>,
    size: Vec<usize>,
    sets: usize,
}

impl UnionFind {
    /// Creates `len` singleton sets `{0}, {1}, …, {len - 1}`.
    pub fn new(len: usize) -> Self {
        Self {
            parent: (0..len).collect(),
            size: vec![1; len],
            sets: len,
        }
    }

    /// Returns the number of elements.
    pub fn len(&self) -> usize {
        self.parent.len()
    }

    /// Returns `true` if there are no elements.
    pub fn is_empty(&self) -> bool {
        self.parent.is_empty()
    }

    /// Returns the number of disjoint sets.
    pub fn set_count(&self) -> usize {
        self.sets
    }

    /// Adds a new singleton set and returns its element.
    pub fn push(&mut self) -> usize {
        let element = self.parent.len();
        self.parent.push(element);
        self.size.push(1);
        self.sets += 1;
        element
    }

    /// Returns the representative of `element`'s set, compressing the path.
    ///
    /// # Errors
    ///
    /// Returns [`Error::IndexOutOfBounds`] if `element >= len`.
    pub fn find(&mut self, element: usize) -> Result<usize, Error> {
        self.check(element)?;
        let mut node = element;
        loop {
            let parent = self.parent[node];
            if parent == node {
                return Ok(node);
            }
            // Path halving: point every other node at its grandparent.
            let grandparent = self.parent[parent];
            self.parent[node] = grandparent;
            node = grandparent;
        }
    }

    /// Returns the representative of `element`'s set without modifying the
    /// structure (useful behind a shared reference).
    ///
    /// # Errors
    ///
    /// Returns [`Error::IndexOutOfBounds`] if `element >= len`.
    pub fn find_root(&self, element: usize) -> Result<usize, Error> {
        self.check(element)?;
        let mut node = element;
        while self.parent[node] != node {
            node = self.parent[node];
        }
        Ok(node)
    }

    /// Merges the sets containing `a` and `b`.
    ///
    /// Returns `true` if two different sets were merged and `false` if `a`
    /// and `b` were already in the same set.
    ///
    /// # Errors
    ///
    /// Returns [`Error::IndexOutOfBounds`] if either element is out of range.
    pub fn union(&mut self, a: usize, b: usize) -> Result<bool, Error> {
        let (ra, rb) = (self.find(a)?, self.find(b)?);
        if ra == rb {
            return Ok(false);
        }
        let (big, small) = if self.size[ra] >= self.size[rb] {
            (ra, rb)
        } else {
            (rb, ra)
        };
        self.parent[small] = big;
        self.size[big] = self.size[big].saturating_add(self.size[small]);
        self.sets -= 1;
        Ok(true)
    }

    /// Returns `true` if `a` and `b` are in the same set.
    ///
    /// # Errors
    ///
    /// Returns [`Error::IndexOutOfBounds`] if either element is out of range.
    pub fn connected(&mut self, a: usize, b: usize) -> Result<bool, Error> {
        Ok(self.find(a)? == self.find(b)?)
    }

    /// Returns the number of elements in `element`'s set.
    ///
    /// # Errors
    ///
    /// Returns [`Error::IndexOutOfBounds`] if `element >= len`.
    pub fn set_size(&mut self, element: usize) -> Result<usize, Error> {
        let root = self.find(element)?;
        Ok(self.size[root])
    }

    fn check(&self, element: usize) -> Result<(), Error> {
        if element < self.parent.len() {
            Ok(())
        } else {
            Err(Error::IndexOutOfBounds {
                index: element,
                len: self.parent.len(),
            })
        }
    }
}
