//! Injectable wall and monotonic clock abstractions for production and testing.

use std::sync::{Arc, Mutex};

use super::datetime::OffsetDateTime;
use super::duration::Duration;

/// An opaque monotonic timestamp produced by a [`Clock`].
///
/// Instants obtained from the same clock source (for example, [`SystemClock`]
/// or a [`MockClock`]) can be compared and subtracted via [`Instant::duration_since`]
/// or measured against a clock via [`Instant::elapsed_from`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Instant {
    inner: InstantInner,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum InstantInner {
    System(std::time::Instant),
    Mock(i128),
}

impl Instant {
    pub(crate) const fn from_system(instant: std::time::Instant) -> Self {
        Self {
            inner: InstantInner::System(instant),
        }
    }

    pub(crate) const fn from_mock_nanos(nanos: i128) -> Self {
        Self {
            inner: InstantInner::Mock(nanos),
        }
    }

    /// Returns the signed [`Duration`] elapsed from `earlier` to `self`.
    ///
    /// # Panics
    ///
    /// Panics if `self` and `earlier` originate from incompatible clock sources
    /// (for example, one from [`SystemClock`] and one from [`MockClock`]), or
    /// if the difference overflows [`Duration`].
    pub fn duration_since(&self, earlier: Self) -> Duration {
        self.checked_duration_since(earlier)
            .expect("cannot compute duration between incompatible or overflowing instants")
    }

    /// Fallibly returns the signed [`Duration`] elapsed from `earlier` to `self`.
    ///
    /// Returns `None` if `self` and `earlier` originate from different clock
    /// kinds or if the resulting duration overflows.
    pub fn checked_duration_since(&self, earlier: Self) -> Option<Duration> {
        match (self.inner, earlier.inner) {
            (InstantInner::System(a), InstantInner::System(b)) => {
                if a >= b {
                    Duration::try_from(a.duration_since(b)).ok()
                } else {
                    Duration::try_from(b.duration_since(a))
                        .ok()
                        .and_then(Duration::checked_neg)
                }
            }
            (InstantInner::Mock(a), InstantInner::Mock(b)) => {
                a.checked_sub(b).and_then(Duration::from_total_nanos)
            }
            _ => None,
        }
    }

    /// Returns the [`Duration`] elapsed between `self` and the current
    /// monotonic reading of `clock`.
    ///
    /// # Examples
    ///
    /// ```
    /// use akku::time::{Clock, Duration, MockClock, OffsetDateTime};
    ///
    /// let start = OffsetDateTime::from_unix_timestamp(1_700_000_000, 0).unwrap();
    /// let clock = MockClock::new(start);
    /// let t0 = clock.monotonic();
    /// clock.advance(Duration::from_millis(250));
    /// assert_eq!(t0.elapsed_from(&clock), Duration::from_millis(250));
    /// ```
    pub fn elapsed_from<C: Clock + ?Sized>(&self, clock: &C) -> Duration {
        clock.monotonic().duration_since(*self)
    }
}

/// A source of current wall-clock and monotonic time.
///
/// Depend on `Clock` in application code so production uses [`SystemClock`]
/// while tests inject [`MockClock`] for deterministic time control.
pub trait Clock: Send + Sync {
    /// Returns the current wall-clock timestamp.
    fn now(&self) -> OffsetDateTime;

    /// Returns a monotonic [`Instant`] suitable for measuring elapsed durations.
    fn monotonic(&self) -> Instant;
}

/// The real operating-system clock backed by [`std::time::SystemTime`] and
/// [`std::time::Instant`].
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl SystemClock {
    /// Creates a new `SystemClock`.
    pub const fn new() -> Self {
        Self
    }
}

impl Clock for SystemClock {
    fn now(&self) -> OffsetDateTime {
        OffsetDateTime::now_utc()
    }

    fn monotonic(&self) -> Instant {
        Instant::from_system(std::time::Instant::now())
    }
}

#[derive(Debug)]
struct MockClockState {
    wall: OffsetDateTime,
    monotonic_nanos: i128,
}

/// A deterministic, thread-safe mock clock for unit and integration tests.
///
/// Cloning a `MockClock` shares the same underlying time state across threads.
///
/// # Examples
///
/// ```
/// use akku::time::{Clock, Duration, MockClock, OffsetDateTime};
///
/// let start: OffsetDateTime = "2024-01-01T00:00:00Z".parse().unwrap();
/// let clock = MockClock::new(start);
/// clock.advance(Duration::from_secs(60));
/// assert_eq!(clock.now().format_rfc3339(), "2024-01-01T00:01:00Z");
/// ```
#[derive(Debug, Clone)]
pub struct MockClock {
    state: Arc<Mutex<MockClockState>>,
}

impl MockClock {
    /// Creates a new `MockClock` initialized at wall-clock time `start`.
    pub fn new(start: OffsetDateTime) -> Self {
        Self {
            state: Arc::new(Mutex::new(MockClockState {
                wall: start,
                monotonic_nanos: 0,
            })),
        }
    }

    /// Advances the mock clock by `delta`.
    ///
    /// Both the wall clock and monotonic clock advance for positive durations.
    /// If `delta` is negative, only the wall clock moves backward so monotonic
    /// timestamps remain non-decreasing.
    ///
    /// # Panics
    ///
    /// Panics if advancing the wall clock or monotonic counter overflows.
    pub fn advance(&self, delta: Duration) {
        let mut guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
        guard.wall = guard
            .wall
            .checked_add(delta)
            .expect("mock clock wall time overflow");
        if delta.is_positive() {
            guard.monotonic_nanos = guard
                .monotonic_nanos
                .checked_add(delta.as_nanos())
                .expect("mock clock monotonic overflow");
        }
    }

    /// Sets the mock wall-clock time to `time` without altering the monotonic counter.
    pub fn set(&self, time: OffsetDateTime) {
        let mut guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
        guard.wall = time;
    }
}

impl Clock for MockClock {
    fn now(&self) -> OffsetDateTime {
        let guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
        guard.wall
    }

    fn monotonic(&self) -> Instant {
        let guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
        Instant::from_mock_nanos(guard.monotonic_nanos)
    }
}
