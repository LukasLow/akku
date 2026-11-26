//! Conformance and cancellation-safety testing kit for [`Filesystem`] backends.
//!
//! Run [`check`] and [`check_cancellation`] against any [`Filesystem`]
//! implementation to verify compliance with the [`akku::vfs`](super) contracts
//! (rules `K01`–`K22` and `KC1`–`KC2`).
//!
//! # Preconditions
//!
//! - The tree is not modified while the kit is running.
//! - `caller` is allowed to read every visited node.
//! - `limits` specify the server-side bounds the backend must respect.

use std::fmt;

use super::fs::Filesystem;
use super::types::{Caller, Limits};

/// A single contract violation detected by [`check`] or [`check_cancellation`].
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    /// Stable rule identifier (e.g. `"K01"`, `"K17"`, `"KC2"`).
    pub rule: &'static str,
    /// Human-readable description of the violation.
    pub detail: String,
}

/// Summary report produced by [`check`] and [`check_cancellation`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    #[allow(dead_code)] // G3 stub; remove in G4
    violations: Vec<Violation>,
    #[allow(dead_code)] // G3 stub; remove in G4
    nodes: usize,
}

impl Report {
    /// Returns `true` if no violations were recorded.
    pub fn is_ok(&self) -> bool {
        todo!()
    }

    /// Returns the slice of recorded violations.
    pub fn violations(&self) -> &[Violation] {
        todo!()
    }

    /// Returns the number of nodes visited during the walk.
    pub fn nodes(&self) -> usize {
        todo!()
    }

    /// Panics with `"vfs::testing: <Display>"` if any violations were recorded.
    #[track_caller]
    pub fn assert_ok(&self) {
        todo!()
    }
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let _ = f;
        todo!()
    }
}

/// Walks the filesystem breadth-first from `fs.root()` (visiting at most
/// `max_nodes` nodes) and verifies every [`Filesystem`] contract (`K01`–`K22`).
///
/// The checker never panics on a misbehaving backend, bounds all loops, and
/// records unexpected errors as [`Violation`] entries in the returned [`Report`].
pub async fn check<F: Filesystem + ?Sized>(
    fs: &F,
    caller: &Caller,
    limits: &Limits,
    max_nodes: usize,
) -> Report {
    let _ = (fs, caller, limits, max_nodes);
    todo!()
}

/// Verifies cancellation safety (`KC1`–`KC2`) across every operation on up to
/// `max_nodes` visited nodes, then runs [`check`].
///
/// For each operation, drops the future unpolled and polls it once with
/// [`std::task::Waker::noop`] before dropping it, confirming that running the
/// operation to completion afterwards matches the baseline result.
pub async fn check_cancellation<F: Filesystem + ?Sized>(
    fs: &F,
    caller: &Caller,
    limits: &Limits,
    max_nodes: usize,
) -> Report {
    let _ = (fs, caller, limits, max_nodes);
    todo!()
}
