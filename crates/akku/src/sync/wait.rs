//! Safe O(1) slab-backed FIFO waiter queue shared across `akku::sync` primitives.

// G3 stub allows — remove in G4.
#![allow(dead_code, unused_variables)]

use std::task::Waker;

/// Lifecycle state of an occupied waiter slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Phase {
    Queued,
    Granted,
    Closed,
}

/// Handle stored inside a waiting future to reference its slot in [`WaitList`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WaitKey {
    index: usize,
    id: u64,
}

enum Slot<W> {
    Vacant {
        next_free: Option<usize>,
    },
    Occupied {
        id: u64,
        waker: Option<Waker>,
        prev: Option<usize>,
        next: Option<usize>,
        phase: Phase,
        payload: W,
    },
}

/// A safe, slab-backed FIFO intrusive-style waiter list.
pub(crate) struct WaitList<W> {
    slots: Vec<Slot<W>>,
    free: Option<usize>,
    head: Option<usize>,
    tail: Option<usize>,
    queued: usize,
    next_id: u64,
}

impl<W> WaitList<W> {
    pub(crate) const fn new() -> Self {
        Self {
            slots: Vec::new(),
            free: None,
            head: None,
            tail: None,
            queued: 0,
            next_id: 0,
        }
    }

    pub(crate) fn push_back(&mut self, waker: Option<Waker>, payload: W) -> WaitKey {
        todo!()
    }

    pub(crate) fn set_waker(&mut self, key: &WaitKey, waker: &Waker) -> Option<Waker> {
        todo!()
    }

    pub(crate) fn grant_front(&mut self) -> Option<Waker> {
        todo!()
    }

    pub(crate) fn remove(&mut self, key: &WaitKey) -> Option<(Phase, W)> {
        todo!()
    }

    pub(crate) fn drain(&mut self, phase: Phase) -> Vec<Waker> {
        todo!()
    }
}
