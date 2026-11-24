//! Zero-dependency O(1) LRU and TTL in-memory cache with eviction and stats.
//!
//! `akku::cache` provides an O(1) in-memory Least-Recently-Used (LRU) cache
//! backed by an index-based doubly-linked list inside a contiguous arena (`Vec`)
//! paired with a `HashMap` lookup table—achieving constant-time `get`, `peek`,
//! `insert`, `remove`, and LRU eviction with zero `unsafe` code.
//!
//! Both [`Cache`] and [`SyncCache`] are thread-safe (`Send + Sync`, `Clone`
//! shares the underlying state via `Arc`) and support:
//! - Entry count capacity limits (`capacity`) and optional byte/weight limits (`max_weight` + `weigher`).
//! - Default and per-entry Time-To-Live (TTL) expiration (lazy on lookup and active via [`Cache::purge_expired`]).
//! - Injectable [`Clock`] (such as [`MockClock`](crate::time::MockClock)) for deterministic, zero-sleep TTL tests.
//! - Eviction listeners (`on_evict`) invoked outside internal locks with [`EvictionReason`].
//! - Detailed hit, miss, eviction, expiration, and weight metrics via [`CacheStats`].
//!
//! # Examples
//!
//! ```
//! use std::time::Duration;
//! use akku::cache::{Cache, EvictionReason};
//! use akku::time::{Duration as AkkuDuration, MockClock, OffsetDateTime};
//!
//! let start = OffsetDateTime::from_unix_timestamp(1_700_000_000, 0).unwrap();
//! let clock = MockClock::new(start);
//!
//! let cache = Cache::builder(2)
//!     .clock(clock.clone())
//!     .default_ttl(Duration::from_secs(60))
//!     .build();
//!
//! cache.insert("a", 1);
//! cache.insert("b", 2);
//! assert_eq!(cache.get(&"a"), Some(1)); // promotes "a" to MRU
//!
//! cache.insert("c", 3); // evicts "b" (LRU)
//! assert_eq!(cache.get(&"b"), None);
//! assert_eq!(cache.get(&"a"), Some(1));
//!
//! clock.advance(AkkuDuration::from_secs(61));
//! assert_eq!(cache.get(&"a"), None); // expired
//! ```

use std::collections::HashMap;
use std::fmt;
use std::hash::Hash;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::time::{Clock, Duration as AkkuDuration, Instant, SystemClock};

const NONE: usize = usize::MAX;

/// Reason an entry was removed from a [`Cache`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EvictionReason {
    /// Evicted as the least-recently-used entry to satisfy entry capacity or weight limits.
    Capacity,
    /// Removed because its time-to-live (TTL) elapsed.
    Expired,
    /// Explicitly removed by the caller (`remove`, `clear`, or overwritten by `insert`).
    Removed,
}

/// Snapshot of cache performance and occupancy metrics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CacheStats {
    /// Total successful lookups (`get`, `peek`, and cache-hit paths of `get_or_insert_with`).
    pub hits: u64,
    /// Total lookups that found no live entry (including lookups that encountered an expired key).
    pub misses: u64,
    /// Total entries evicted due to capacity or weight limits ([`EvictionReason::Capacity`]).
    pub evictions: u64,
    /// Total entries removed because their TTL expired ([`EvictionReason::Expired`]).
    pub expirations: u64,
    /// Current number of entries resident in the cache.
    pub len: usize,
    /// Maximum number of entries the cache can hold before evicting.
    pub capacity: usize,
    /// Current total weight of all resident entries in the cache.
    pub weight: u64,
    /// Maximum total weight allowed if a weight limit is configured (`None` if unbounded by weight).
    pub max_weight: Option<u64>,
}

impl CacheStats {
    /// Computes the hit ratio in `0.0..=1.0` (`0.0` if no lookups have occurred).
    pub fn hit_rate(&self) -> f64 {
        let total = self.hits + self.misses;
        if total == 0 {
            0.0
        } else {
            (self.hits as f64) / (total as f64)
        }
    }
}

/// Type alias for the eviction listener callback.
pub type EvictHook<K, V> = Arc<dyn Fn(&K, V, EvictionReason) + Send + Sync + 'static>;

/// Type alias for the entry weight calculator callback.
pub type Weigher<K, V> = Arc<dyn Fn(&K, &V) -> u64 + Send + Sync + 'static>;

/// Builder for configuring and constructing a [`Cache`] or [`SyncCache`].
pub struct CacheBuilder<K, V> {
    capacity: usize,
    max_weight: Option<u64>,
    weigher: Option<Weigher<K, V>>,
    default_ttl: Option<AkkuDuration>,
    clock: Arc<dyn Clock>,
    on_evict: Option<EvictHook<K, V>>,
}

impl<K, V> fmt::Debug for CacheBuilder<K, V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CacheBuilder")
            .field("capacity", &self.capacity)
            .field("max_weight", &self.max_weight)
            .field("has_weigher", &self.weigher.is_some())
            .field("default_ttl", &self.default_ttl)
            .field("has_on_evict", &self.on_evict.is_some())
            .finish()
    }
}

impl<K, V> CacheBuilder<K, V>
where
    K: Eq + Hash + Clone,
    V: Clone,
{
    /// Creates a builder with the given maximum entry `capacity`.
    ///
    /// # Panics
    ///
    /// Panics if `capacity == 0`.
    pub fn new(capacity: usize) -> Self {
        assert!(capacity > 0, "Cache capacity must be greater than zero");
        Self {
            capacity,
            max_weight: None,
            weigher: None,
            default_ttl: None,
            clock: Arc::new(SystemClock::new()),
            on_evict: None,
        }
    }

    /// Configures an optional maximum total weight (e.g. byte budget) for the cache.
    ///
    /// When set without a custom [`CacheBuilder::weigher`], each entry has a default weight of `1`.
    pub fn max_weight(mut self, max_weight: u64) -> Self {
        assert!(max_weight > 0, "Cache max_weight must be greater than zero");
        self.max_weight = Some(max_weight);
        self
    }

    /// Configures a custom weight calculation function for entries.
    ///
    /// Combine with [`CacheBuilder::max_weight`] to enforce byte-size or custom cost budgets.
    pub fn weigher<F>(mut self, weigher: F) -> Self
    where
        F: Fn(&K, &V) -> u64 + Send + Sync + 'static,
    {
        self.weigher = Some(Arc::new(weigher));
        self
    }

    /// Sets a default time-to-live applied to all `insert` calls unless overridden by `insert_with_ttl`.
    pub fn default_ttl(mut self, ttl: Duration) -> Self {
        self.default_ttl = Some(std_to_akku_duration(ttl));
        self
    }

    /// Injects a custom [`Clock`] (defaults to [`SystemClock`]).
    pub fn clock<C: Clock + 'static>(mut self, clock: C) -> Self {
        self.clock = Arc::new(clock);
        self
    }

    /// Injects a shared [`Clock`] trait object.
    pub fn shared_clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = clock;
        self
    }

    /// Registers a callback invoked whenever an entry is evicted, expired, or removed.
    ///
    /// The callback is always invoked outside the cache's internal mutex to prevent deadlocks.
    pub fn on_evict<F>(mut self, hook: F) -> Self
    where
        F: Fn(&K, V, EvictionReason) + Send + Sync + 'static,
    {
        self.on_evict = Some(Arc::new(hook));
        self
    }

    /// Builds the [`Cache`].
    pub fn build(self) -> Cache<K, V> {
        Cache {
            shared: Arc::new(CacheShared {
                capacity: self.capacity,
                max_weight: self.max_weight,
                weigher: self.weigher,
                default_ttl: self.default_ttl,
                clock: self.clock,
                on_evict: self.on_evict,
                inner: Mutex::new(CacheInner::with_capacity(self.capacity)),
            }),
        }
    }

    /// Builds a [`SyncCache`] wrapper around the configured cache.
    pub fn build_sync(self) -> SyncCache<K, V> {
        SyncCache {
            cache: self.build(),
        }
    }
}

fn std_to_akku_duration(d: Duration) -> AkkuDuration {
    AkkuDuration::try_from(d).unwrap_or(AkkuDuration::MAX)
}

#[derive(Debug)]
struct Entry<K, V> {
    key: K,
    value: V,
    weight: u64,
    prev: usize,
    next: usize,
    expiry: Option<(Instant, AkkuDuration)>,
}

#[derive(Debug)]
enum Slot<K, V> {
    Occupied(Entry<K, V>),
    Free { next_free: usize },
}

struct CacheInner<K, V> {
    index: HashMap<K, usize>,
    entries: Vec<Slot<K, V>>,
    free_head: usize,
    head: usize,
    tail: usize,
    current_weight: u64,
    hits: u64,
    misses: u64,
    evictions: u64,
    expirations: u64,
}

impl<K, V> CacheInner<K, V>
where
    K: Eq + Hash + Clone,
{
    fn with_capacity(capacity: usize) -> Self {
        let prealloc = capacity.min(1024);
        Self {
            index: HashMap::with_capacity(prealloc),
            entries: Vec::with_capacity(prealloc),
            free_head: NONE,
            head: NONE,
            tail: NONE,
            current_weight: 0,
            hits: 0,
            misses: 0,
            evictions: 0,
            expirations: 0,
        }
    }

    fn is_expired_at(entry: &Entry<K, V>, now_mono: Instant) -> bool {
        if let Some((inserted_at, ttl)) = entry.expiry {
            if ttl <= AkkuDuration::ZERO {
                return true;
            }
            if let Some(elapsed) = now_mono.checked_duration_since(inserted_at) {
                return elapsed >= ttl;
            }
        }
        false
    }

    fn detach(&mut self, idx: usize) {
        let (prev, next) = match &self.entries[idx] {
            Slot::Occupied(e) => (e.prev, e.next),
            Slot::Free { .. } => unreachable!("cannot detach a free slot"),
        };

        if prev != NONE {
            if let Slot::Occupied(p) = &mut self.entries[prev] {
                p.next = next;
            }
        } else {
            self.head = next;
        }

        if next != NONE {
            if let Slot::Occupied(n) = &mut self.entries[next] {
                n.prev = prev;
            }
        } else {
            self.tail = prev;
        }

        if let Slot::Occupied(e) = &mut self.entries[idx] {
            e.prev = NONE;
            e.next = NONE;
        }
    }

    fn attach_front(&mut self, idx: usize) {
        let old_head = self.head;
        if let Slot::Occupied(e) = &mut self.entries[idx] {
            e.prev = NONE;
            e.next = old_head;
        }
        if old_head != NONE {
            if let Slot::Occupied(h) = &mut self.entries[old_head] {
                h.prev = idx;
            }
        } else {
            self.tail = idx;
        }
        self.head = idx;
    }

    fn move_to_front(&mut self, idx: usize) {
        if self.head == idx {
            return;
        }
        self.detach(idx);
        self.attach_front(idx);
    }

    fn allocate_slot(&mut self, entry: Entry<K, V>) -> usize {
        if self.free_head != NONE {
            let idx = self.free_head;
            match self.entries[idx] {
                Slot::Free { next_free } => {
                    self.free_head = next_free;
                }
                Slot::Occupied(_) => unreachable!("free_head pointed to occupied slot"),
            }
            self.entries[idx] = Slot::Occupied(entry);
            idx
        } else {
            let idx = self.entries.len();
            self.entries.push(Slot::Occupied(entry));
            idx
        }
    }

    fn remove_slot(&mut self, idx: usize) -> Entry<K, V> {
        self.detach(idx);
        let old = std::mem::replace(
            &mut self.entries[idx],
            Slot::Free {
                next_free: self.free_head,
            },
        );
        self.free_head = idx;
        match old {
            Slot::Occupied(entry) => {
                self.index.remove(&entry.key);
                self.current_weight = self.current_weight.saturating_sub(entry.weight);
                entry
            }
            Slot::Free { .. } => unreachable!("remove_slot called on free slot"),
        }
    }
}

struct CacheShared<K, V> {
    capacity: usize,
    max_weight: Option<u64>,
    weigher: Option<Weigher<K, V>>,
    default_ttl: Option<AkkuDuration>,
    clock: Arc<dyn Clock>,
    on_evict: Option<EvictHook<K, V>>,
    inner: Mutex<CacheInner<K, V>>,
}

impl<K, V> CacheShared<K, V> {
    fn lock(&self) -> std::sync::MutexGuard<'_, CacheInner<K, V>> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn compute_weight(&self, key: &K, value: &V) -> u64 {
        match &self.weigher {
            Some(w) => w(key, value),
            None => 1,
        }
    }

    fn fire_evictions(&self, evicted: Vec<(K, V, EvictionReason)>) {
        if let Some(hook) = &self.on_evict {
            for (k, v, reason) in evicted {
                hook(&k, v, reason);
            }
        }
    }
}

/// A thread-safe, `Send + Sync`, O(1) in-memory LRU cache with TTL, optional weight bounds,
/// eviction callbacks, and deterministic [`Clock`] support.
///
/// Cloning a `Cache` is O(1) (`Arc`-backed) and shares the same underlying state across threads.
pub struct Cache<K, V> {
    shared: Arc<CacheShared<K, V>>,
}

impl<K, V> Clone for Cache<K, V> {
    fn clone(&self) -> Self {
        Self {
            shared: Arc::clone(&self.shared),
        }
    }
}

impl<K: fmt::Debug, V: fmt::Debug> fmt::Debug for Cache<K, V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let guard = self.shared.lock();
        f.debug_struct("Cache")
            .field("len", &guard.index.len())
            .field("capacity", &self.shared.capacity)
            .field("weight", &guard.current_weight)
            .field("max_weight", &self.shared.max_weight)
            .field("hits", &guard.hits)
            .field("misses", &guard.misses)
            .field("evictions", &guard.evictions)
            .field("expirations", &guard.expirations)
            .finish()
    }
}

impl<K, V> Cache<K, V>
where
    K: Eq + Hash + Clone,
    V: Clone,
{
    /// Creates a new cache with `capacity` maximum entries and [`SystemClock`].
    ///
    /// # Panics
    ///
    /// Panics if `capacity == 0`.
    pub fn new(capacity: usize) -> Self {
        CacheBuilder::new(capacity).build()
    }

    /// Returns a [`CacheBuilder`] for configuring default TTL, weight limits, custom [`Clock`],
    /// or an `on_evict` listener.
    ///
    /// # Panics
    ///
    /// Panics if `capacity == 0`.
    pub fn builder(capacity: usize) -> CacheBuilder<K, V> {
        CacheBuilder::new(capacity)
    }

    /// Fetches a clone of the value for `key`, promoting it to most-recently-used (MRU).
    ///
    /// If the entry exists but has expired, it is eagerly removed (`expirations += 1`,
    /// `misses += 1`, `on_evict` invoked with [`EvictionReason::Expired`]) and `None` is returned.
    pub fn get(&self, key: &K) -> Option<V> {
        let now = self.shared.clock.monotonic();
        let mut evicted_expired = None;
        let result = {
            let mut guard = self.shared.lock();
            if let Some(&idx) = guard.index.get(key) {
                let expired = match &guard.entries[idx] {
                    Slot::Occupied(e) => CacheInner::is_expired_at(e, now),
                    Slot::Free { .. } => unreachable!(),
                };
                if expired {
                    let entry = guard.remove_slot(idx);
                    guard.expirations += 1;
                    guard.misses += 1;
                    evicted_expired = Some((entry.key, entry.value, EvictionReason::Expired));
                    None
                } else {
                    guard.move_to_front(idx);
                    guard.hits += 1;
                    match &guard.entries[idx] {
                        Slot::Occupied(e) => Some(e.value.clone()),
                        Slot::Free { .. } => unreachable!(),
                    }
                }
            } else {
                guard.misses += 1;
                None
            }
        };

        if let Some((k, v, reason)) = evicted_expired {
            if let Some(hook) = &self.shared.on_evict {
                hook(&k, v, reason);
            }
        }
        result
    }

    /// Fetches a clone of the value for `key` **without** updating its LRU position.
    ///
    /// If the entry has expired, it is removed (`expirations += 1`, `misses += 1`,
    /// `on_evict` invoked with [`EvictionReason::Expired`]) and `None` is returned.
    pub fn peek(&self, key: &K) -> Option<V> {
        let now = self.shared.clock.monotonic();
        let mut evicted_expired = None;
        let result = {
            let mut guard = self.shared.lock();
            if let Some(&idx) = guard.index.get(key) {
                let expired = match &guard.entries[idx] {
                    Slot::Occupied(e) => CacheInner::is_expired_at(e, now),
                    Slot::Free { .. } => unreachable!(),
                };
                if expired {
                    let entry = guard.remove_slot(idx);
                    guard.expirations += 1;
                    guard.misses += 1;
                    evicted_expired = Some((entry.key, entry.value, EvictionReason::Expired));
                    None
                } else {
                    guard.hits += 1;
                    match &guard.entries[idx] {
                        Slot::Occupied(e) => Some(e.value.clone()),
                        Slot::Free { .. } => unreachable!(),
                    }
                }
            } else {
                guard.misses += 1;
                None
            }
        };

        if let Some((k, v, reason)) = evicted_expired {
            if let Some(hook) = &self.shared.on_evict {
                hook(&k, v, reason);
            }
        }
        result
    }

    /// Returns `true` if `key` exists and has not expired.
    ///
    /// Does not modify LRU order or hit/miss statistics, though an expired entry encountered
    /// is lazily removed.
    pub fn contains_key(&self, key: &K) -> bool {
        let now = self.shared.clock.monotonic();
        let mut evicted_expired = None;
        let present = {
            let mut guard = self.shared.lock();
            if let Some(&idx) = guard.index.get(key) {
                let expired = match &guard.entries[idx] {
                    Slot::Occupied(e) => CacheInner::is_expired_at(e, now),
                    Slot::Free { .. } => unreachable!(),
                };
                if expired {
                    let entry = guard.remove_slot(idx);
                    guard.expirations += 1;
                    evicted_expired = Some((entry.key, entry.value, EvictionReason::Expired));
                    false
                } else {
                    true
                }
            } else {
                false
            }
        };

        if let Some((k, v, reason)) = evicted_expired {
            if let Some(hook) = &self.shared.on_evict {
                hook(&k, v, reason);
            }
        }
        present
    }

    /// Inserts `key` -> `value` using the cache's `default_ttl` (if configured), promoting it to MRU.
    ///
    /// Returns the previous non-expired value if `key` was already present.
    pub fn insert(&self, key: K, value: V) -> Option<V> {
        self.insert_internal(key, value, self.shared.default_ttl)
    }

    /// Inserts `key` -> `value` with an explicit `ttl`, overriding `default_ttl`.
    ///
    /// Returns the previous non-expired value if `key` was already present.
    pub fn insert_with_ttl(&self, key: K, value: V, ttl: Duration) -> Option<V> {
        self.insert_internal(key, value, Some(std_to_akku_duration(ttl)))
    }

    fn insert_internal(&self, key: K, value: V, ttl: Option<AkkuDuration>) -> Option<V> {
        let now = self.shared.clock.monotonic();
        let weight = self.shared.compute_weight(&key, &value);
        let expiry = ttl.map(|d| (now, d));
        let mut evicted = Vec::new();
        let mut previous_live = None;

        {
            let mut guard = self.shared.lock();

            // Check if key already exists.
            if let Some(&existing_idx) = guard.index.get(&key) {
                let old_entry = guard.remove_slot(existing_idx);
                if CacheInner::is_expired_at(&old_entry, now) {
                    guard.expirations += 1;
                    evicted.push((old_entry.key, old_entry.value, EvictionReason::Expired));
                } else {
                    previous_live = Some(old_entry.value.clone());
                    evicted.push((old_entry.key, old_entry.value, EvictionReason::Removed));
                }
            }

            // If a max_weight is configured and this single item exceeds max_weight on its own,
            // evict everything and immediately evict this item too.
            if let Some(max_w) = self.shared.max_weight {
                if weight > max_w {
                    guard.evictions += 1;
                    evicted.push((key, value, EvictionReason::Capacity));
                    drop(guard);
                    self.shared.fire_evictions(evicted);
                    return previous_live;
                }
            }

            // Evict from tail (LRU) while at capacity or exceeding max_weight.
            while !guard.index.is_empty()
                && (guard.index.len() >= self.shared.capacity
                    || self
                        .shared
                        .max_weight
                        .is_some_and(|max_w| guard.current_weight + weight > max_w))
            {
                let tail_idx = guard.tail;
                if tail_idx == NONE {
                    break;
                }
                let victim = guard.remove_slot(tail_idx);
                if CacheInner::is_expired_at(&victim, now) {
                    guard.expirations += 1;
                    evicted.push((victim.key, victim.value, EvictionReason::Expired));
                } else {
                    guard.evictions += 1;
                    evicted.push((victim.key, victim.value, EvictionReason::Capacity));
                }
            }

            // Insert the new entry at the head (MRU).
            let entry = Entry {
                key: key.clone(),
                value,
                weight,
                prev: NONE,
                next: NONE,
                expiry,
            };
            let idx = guard.allocate_slot(entry);
            guard.attach_front(idx);
            guard.index.insert(key, idx);
            guard.current_weight += weight;
        }

        self.shared.fire_evictions(evicted);
        previous_live
    }

    /// Returns the existing live value for `key` (promoting to MRU and incrementing `hits`),
    /// or computes `f()`, inserts it with `default_ttl`, increments `misses`, and returns it.
    pub fn get_or_insert_with<F>(&self, key: K, f: F) -> V
    where
        F: FnOnce() -> V,
    {
        match self.get_or_try_insert_with::<_, ()>(key, || Ok(f())) {
            Ok(v) => v,
            Err(()) => unreachable!(),
        }
    }

    /// Fallible counterpart to [`Cache::get_or_insert_with`].
    ///
    /// If `f()` returns `Err(e)`, nothing is inserted and `Err(e)` is returned.
    pub fn get_or_try_insert_with<F, E>(&self, key: K, f: F) -> Result<V, E>
    where
        F: FnOnce() -> Result<V, E>,
    {
        if let Some(val) = self.get(&key) {
            return Ok(val);
        }

        let computed = f()?;
        let now = self.shared.clock.monotonic();
        let weight = self.shared.compute_weight(&key, &computed);
        let expiry = self.shared.default_ttl.map(|d| (now, d));
        let mut evicted = Vec::new();

        let result_val = {
            let mut guard = self.shared.lock();

            // Re-check under lock in case another thread inserted concurrently.
            if let Some(&existing_idx) = guard.index.get(&key) {
                let expired = match &guard.entries[existing_idx] {
                    Slot::Occupied(e) => CacheInner::is_expired_at(e, now),
                    Slot::Free { .. } => unreachable!(),
                };
                if !expired {
                    guard.move_to_front(existing_idx);
                    match &guard.entries[existing_idx] {
                        Slot::Occupied(e) => return Ok(e.value.clone()),
                        Slot::Free { .. } => unreachable!(),
                    }
                } else {
                    let old_entry = guard.remove_slot(existing_idx);
                    guard.expirations += 1;
                    evicted.push((old_entry.key, old_entry.value, EvictionReason::Expired));
                }
            }

            if let Some(max_w) = self.shared.max_weight {
                if weight > max_w {
                    guard.evictions += 1;
                    evicted.push((key, computed.clone(), EvictionReason::Capacity));
                    drop(guard);
                    self.shared.fire_evictions(evicted);
                    return Ok(computed);
                }
            }

            while !guard.index.is_empty()
                && (guard.index.len() >= self.shared.capacity
                    || self
                        .shared
                        .max_weight
                        .is_some_and(|max_w| guard.current_weight + weight > max_w))
            {
                let tail_idx = guard.tail;
                if tail_idx == NONE {
                    break;
                }
                let victim = guard.remove_slot(tail_idx);
                if CacheInner::is_expired_at(&victim, now) {
                    guard.expirations += 1;
                    evicted.push((victim.key, victim.value, EvictionReason::Expired));
                } else {
                    guard.evictions += 1;
                    evicted.push((victim.key, victim.value, EvictionReason::Capacity));
                }
            }

            let entry = Entry {
                key: key.clone(),
                value: computed.clone(),
                weight,
                prev: NONE,
                next: NONE,
                expiry,
            };
            let idx = guard.allocate_slot(entry);
            guard.attach_front(idx);
            guard.index.insert(key, idx);
            guard.current_weight += weight;
            computed
        };

        self.shared.fire_evictions(evicted);
        Ok(result_val)
    }

    /// Explicitly removes `key` from the cache, returning the live value if present and not expired.
    ///
    /// Invokes `on_evict` with [`EvictionReason::Removed`] (or [`EvictionReason::Expired`] if it had already expired).
    pub fn remove(&self, key: &K) -> Option<V> {
        let now = self.shared.clock.monotonic();
        let mut evicted = None;
        let result = {
            let mut guard = self.shared.lock();
            if let Some(&idx) = guard.index.get(key) {
                let entry = guard.remove_slot(idx);
                if CacheInner::is_expired_at(&entry, now) {
                    guard.expirations += 1;
                    evicted = Some((entry.key, entry.value, EvictionReason::Expired));
                    None
                } else {
                    let ret = entry.value.clone();
                    evicted = Some((entry.key, entry.value, EvictionReason::Removed));
                    Some(ret)
                }
            } else {
                None
            }
        };

        if let Some((k, v, reason)) = evicted {
            if let Some(hook) = &self.shared.on_evict {
                hook(&k, v, reason);
            }
        }
        result
    }

    /// Actively scans all entries, removes every expired entry, increments `expirations`,
    /// invokes `on_evict` with [`EvictionReason::Expired`] for each, and returns the number of purged entries.
    pub fn purge_expired(&self) -> usize {
        let now = self.shared.clock.monotonic();
        let mut evicted = Vec::new();

        {
            let mut guard = self.shared.lock();
            let mut curr = guard.tail;
            while curr != NONE {
                let (prev, expired) = match &guard.entries[curr] {
                    Slot::Occupied(e) => (e.prev, CacheInner::is_expired_at(e, now)),
                    Slot::Free { .. } => unreachable!(),
                };
                if expired {
                    let entry = guard.remove_slot(curr);
                    guard.expirations += 1;
                    evicted.push((entry.key, entry.value, EvictionReason::Expired));
                }
                curr = prev;
            }
        }

        let count = evicted.len();
        self.shared.fire_evictions(evicted);
        count
    }

    /// Removes all entries from the cache, invoking `on_evict` with [`EvictionReason::Removed`]
    /// for each live entry (or [`EvictionReason::Expired`] for entries already expired).
    pub fn clear(&self) {
        let now = self.shared.clock.monotonic();
        let mut evicted = Vec::new();

        {
            let mut guard = self.shared.lock();
            let mut curr = guard.head;
            while curr != NONE {
                let next = match &guard.entries[curr] {
                    Slot::Occupied(e) => e.next,
                    Slot::Free { .. } => unreachable!(),
                };
                let entry = guard.remove_slot(curr);
                if CacheInner::is_expired_at(&entry, now) {
                    guard.expirations += 1;
                    evicted.push((entry.key, entry.value, EvictionReason::Expired));
                } else {
                    evicted.push((entry.key, entry.value, EvictionReason::Removed));
                }
                curr = next;
            }
            guard.entries.clear();
            guard.free_head = NONE;
            guard.head = NONE;
            guard.tail = NONE;
            guard.current_weight = 0;
        }

        self.shared.fire_evictions(evicted);
    }

    /// Returns the number of entries currently stored in the cache.
    pub fn len(&self) -> usize {
        self.shared.lock().index.len()
    }

    /// Returns `true` if the cache holds no entries.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Returns the configured maximum entry capacity.
    pub fn capacity(&self) -> usize {
        self.shared.capacity
    }

    /// Returns the current total weight of resident entries.
    pub fn weight(&self) -> u64 {
        self.shared.lock().current_weight
    }

    /// Returns the configured maximum weight limit, if any.
    pub fn max_weight(&self) -> Option<u64> {
        self.shared.max_weight
    }

    /// Returns a snapshot of [`CacheStats`].
    pub fn stats(&self) -> CacheStats {
        let guard = self.shared.lock();
        CacheStats {
            hits: guard.hits,
            misses: guard.misses,
            evictions: guard.evictions,
            expirations: guard.expirations,
            len: guard.index.len(),
            capacity: self.shared.capacity,
            weight: guard.current_weight,
            max_weight: self.shared.max_weight,
        }
    }
}

/// Explicit thread-safe shared cache handle wrapping [`Cache`].
///
/// Both [`Cache`] and `SyncCache` are `Send + Sync + Clone` and share their state across
/// threads via `Arc`. `SyncCache` is provided for callers that prefer an explicit concurrent
/// wrapper type name.
pub struct SyncCache<K, V> {
    cache: Cache<K, V>,
}

impl<K, V> Clone for SyncCache<K, V> {
    fn clone(&self) -> Self {
        Self {
            cache: self.cache.clone(),
        }
    }
}

impl<K: fmt::Debug, V: fmt::Debug> fmt::Debug for SyncCache<K, V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("SyncCache").field(&self.cache).finish()
    }
}

impl<K, V> From<Cache<K, V>> for SyncCache<K, V> {
    fn from(cache: Cache<K, V>) -> Self {
        Self { cache }
    }
}

impl<K, V> SyncCache<K, V>
where
    K: Eq + Hash + Clone,
    V: Clone,
{
    /// Creates a new `SyncCache` with `capacity` maximum entries.
    pub fn new(capacity: usize) -> Self {
        Self {
            cache: Cache::new(capacity),
        }
    }

    /// Returns a [`CacheBuilder`] for constructing a `SyncCache`.
    pub fn builder(capacity: usize) -> CacheBuilder<K, V> {
        Cache::builder(capacity)
    }

    /// Wraps an existing [`Cache`] in a `SyncCache`.
    pub fn from_cache(cache: Cache<K, V>) -> Self {
        Self { cache }
    }

    /// Returns a reference to the inner [`Cache`].
    pub fn as_cache(&self) -> &Cache<K, V> {
        &self.cache
    }

    /// Fetches a clone of the value for `key`, promoting it to MRU.
    pub fn get(&self, key: &K) -> Option<V> {
        self.cache.get(key)
    }

    /// Fetches a clone of the value for `key` without updating LRU order.
    pub fn peek(&self, key: &K) -> Option<V> {
        self.cache.peek(key)
    }

    /// Returns `true` if `key` is present and not expired.
    pub fn contains_key(&self, key: &K) -> bool {
        self.cache.contains_key(key)
    }

    /// Inserts `key` -> `value` using `default_ttl`.
    pub fn insert(&self, key: K, value: V) -> Option<V> {
        self.cache.insert(key, value)
    }

    /// Inserts `key` -> `value` with an explicit `ttl`.
    pub fn insert_with_ttl(&self, key: K, value: V, ttl: Duration) -> Option<V> {
        self.cache.insert_with_ttl(key, value, ttl)
    }

    /// Gets the live value for `key` or inserts the result of `f()`.
    pub fn get_or_insert_with<F>(&self, key: K, f: F) -> V
    where
        F: FnOnce() -> V,
    {
        self.cache.get_or_insert_with(key, f)
    }

    /// Fallibly gets the live value for `key` or inserts the `Ok` result of `f()`.
    pub fn get_or_try_insert_with<F, E>(&self, key: K, f: F) -> Result<V, E>
    where
        F: FnOnce() -> Result<V, E>,
    {
        self.cache.get_or_try_insert_with(key, f)
    }

    /// Removes `key` from the cache and returns its live value, if any.
    pub fn remove(&self, key: &K) -> Option<V> {
        self.cache.remove(key)
    }

    /// Actively purges all expired entries and returns the number removed.
    pub fn purge_expired(&self) -> usize {
        self.cache.purge_expired()
    }

    /// Removes all entries from the cache.
    pub fn clear(&self) {
        self.cache.clear()
    }

    /// Returns the number of resident entries.
    pub fn len(&self) -> usize {
        self.cache.len()
    }

    /// Returns `true` if the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.cache.is_empty()
    }

    /// Returns the maximum entry capacity.
    pub fn capacity(&self) -> usize {
        self.cache.capacity()
    }

    /// Returns the current total weight of resident entries.
    pub fn weight(&self) -> u64 {
        self.cache.weight()
    }

    /// Returns a snapshot of [`CacheStats`].
    pub fn stats(&self) -> CacheStats {
        self.cache.stats()
    }
}
