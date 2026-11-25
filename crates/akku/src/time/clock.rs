//! Injectable wall and monotonic clock abstractions for production and testing.

use std::sync::{Arc, Mutex};

use super::datetime::OffsetDateTime;
use super::duration::SignedDuration;
use super::TimeError;

/// An opaque monotonic timestamp produced by an injectable [`Clock`].
///
/// Unlike [`std::time::Instant`] (re-exported at [`crate::time::Instant`]), a
/// `ClockInstant` can originate from either [`SystemClock`] or a deterministic
/// [`MockClock`]. Instants obtained from the same clock source can be compared
/// and subtracted via [`ClockInstant::duration_since`] or measured against a
/// clock via [`ClockInstant::elapsed_from`].
///
/// System-backed values convert freely from [`std::time::Instant`] via [`From`]
/// and fallibly back via [`TryFrom`] (returning [`TimeError::MockInstant`] for
/// mock readings).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ClockInstant {
    inner: ClockInstantInner,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum ClockInstantInner {
    System(std::time::Instant),
    Mock(i128),
}

impl ClockInstant {
    pub(crate) const fn from_system(instant: std::time::Instant) -> Self {
        Self {
            inner: ClockInstantInner::System(instant),
        }
    }

    pub(crate) const fn from_mock_nanos(nanos: i128) -> Self {
        Self {
            inner: ClockInstantInner::Mock(nanos),
        }
    }

    /// Returns the signed [`SignedDuration`] elapsed from `earlier` to `self`.
    ///
    /// # Panics
    ///
    /// Panics if `self` and `earlier` originate from incompatible clock sources
    /// (for example, one from [`SystemClock`] and one from [`MockClock`]), or
    /// if the difference overflows [`SignedDuration`].
    pub fn duration_since(&self, earlier: Self) -> SignedDuration {
        self.checked_duration_since(earlier)
            .expect("cannot compute duration between incompatible or overflowing instants")
    }

    /// Fallibly returns the signed [`SignedDuration`] elapsed from `earlier` to `self`.
    ///
    /// Returns `None` if `self` and `earlier` originate from different clock
    /// kinds or if the resulting duration overflows.
    pub fn checked_duration_since(&self, earlier: Self) -> Option<SignedDuration> {
        match (self.inner, earlier.inner) {
            (ClockInstantInner::System(a), ClockInstantInner::System(b)) => {
                if a >= b {
                    SignedDuration::try_from(a.duration_since(b)).ok()
                } else {
                    SignedDuration::try_from(b.duration_since(a))
                        .ok()
                        .and_then(SignedDuration::checked_neg)
                }
            }
            (ClockInstantInner::Mock(a), ClockInstantInner::Mock(b)) => {
                a.checked_sub(b).and_then(SignedDuration::from_total_nanos)
            }
            _ => None,
        }
    }

    /// Returns the [`SignedDuration`] elapsed between `self` and the current
    /// monotonic reading of `clock`.
    ///
    /// # Examples
    ///
    /// ```
    /// use akku::time::{Clock, MockClock, OffsetDateTime, SignedDuration};
    ///
    /// let start = OffsetDateTime::from_unix_timestamp(1_700_000_000, 0).unwrap();
    /// let clock = MockClock::new(start);
    /// let t0 = clock.monotonic();
    /// clock.advance(SignedDuration::from_millis(250));
    /// assert_eq!(t0.elapsed_from(&clock), SignedDuration::from_millis(250));
    /// ```
    pub fn elapsed_from<C: Clock + ?Sized>(&self, clock: &C) -> SignedDuration {
        clock.monotonic().duration_since(*self)
    }
}

impl From<std::time::Instant> for ClockInstant {
    fn from(instant: std::time::Instant) -> Self {
        Self::from_system(instant)
    }
}

impl TryFrom<ClockInstant> for std::time::Instant {
    type Error = TimeError;

    fn try_from(value: ClockInstant) -> Result<Self, Self::Error> {
        match value.inner {
            ClockInstantInner::System(instant) => Ok(instant),
            ClockInstantInner::Mock(_) => Err(TimeError::MockInstant),
        }
    }
}

/// A source of current wall-clock and monotonic time.
///
/// Depend on `Clock` in application code so production uses [`SystemClock`]
/// while tests inject [`MockClock`] for deterministic time control.
pub trait Clock: Send + Sync {
    /// Returns the current wall-clock timestamp.
    fn now(&self) -> OffsetDateTime;

    /// Returns a monotonic [`ClockInstant`] suitable for measuring elapsed durations.
    fn monotonic(&self) -> ClockInstant;
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

    fn monotonic(&self) -> ClockInstant {
        ClockInstant::from_system(std::time::Instant::now())
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
/// use akku::time::{Clock, MockClock, OffsetDateTime, SignedDuration};
///
/// let start: OffsetDateTime = "2024-01-01T00:00:00Z".parse().unwrap();
/// let clock = MockClock::new(start);
/// clock.advance(SignedDuration::from_secs(60));
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
    pub fn advance(&self, delta: SignedDuration) {
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

    fn monotonic(&self) -> ClockInstant {
        let guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
        ClockInstant::from_mock_nanos(guard.monotonic_nanos)
    }
}
