//! Thread-safe in-memory `BTreeMap`-backed key-value store with TTL and ordered prefix/range scans.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use crate::time::{Clock, ClockInstant, SignedDuration, SystemClock};

use super::{BatchOp, KvLimits, KvResult, KvStore};

#[derive(Debug, Clone)]
struct MemoryEntry {
    value: Vec<u8>,
    expiry: Option<(ClockInstant, SignedDuration)>,
}

impl MemoryEntry {
    fn is_expired(&self, now: ClockInstant) -> bool {
        if let Some((inserted_at, ttl)) = self.expiry {
            if ttl <= SignedDuration::ZERO {
                return true;
            }
            if let Some(elapsed) = now.checked_duration_since(inserted_at) {
                return elapsed >= ttl;
            }
        }
        false
    }
}

struct MemoryStoreInner {
    map: RwLock<BTreeMap<String, MemoryEntry>>,
    clock: Arc<dyn Clock>,
    limits: KvLimits,
}

impl MemoryStoreInner {
    fn read(&self) -> std::sync::RwLockReadGuard<'_, BTreeMap<String, MemoryEntry>> {
        self.map.read().unwrap_or_else(|e| e.into_inner())
    }

    fn write(&self) -> std::sync::RwLockWriteGuard<'_, BTreeMap<String, MemoryEntry>> {
        self.map.write().unwrap_or_else(|e| e.into_inner())
    }
}

/// A thread-safe, in-memory `BTreeMap`-backed key-value store implementing [`KvStore`].
///
/// Cloning a `MemoryStore` is O(1) (`Arc`-backed) and shares the same underlying dataset.
#[derive(Clone)]
pub struct MemoryStore {
    inner: Arc<MemoryStoreInner>,
}

impl fmt::Debug for MemoryStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MemoryStore")
            .field("len", &self.len())
            .field("limits", &self.inner.limits)
            .finish()
    }
}

impl Default for MemoryStore {
    fn default() -> Self {
        Self::new()
    }
}

impl MemoryStore {
    /// Creates a new empty `MemoryStore` using [`SystemClock`] and default [`KvLimits`].
    pub fn new() -> Self {
        Self::with_clock(SystemClock::new())
    }

    /// Creates a new empty `MemoryStore` with an injected [`Clock`] (e.g., [`MockClock`](crate::time::MockClock)).
    pub fn with_clock<C: Clock + 'static>(clock: C) -> Self {
        Self {
            inner: Arc::new(MemoryStoreInner {
                map: RwLock::new(BTreeMap::new()),
                clock: Arc::new(clock),
                limits: KvLimits::default(),
            }),
        }
    }

    /// Configures custom [`KvLimits`] (`max_key_size` and `max_value_size`) on this store.
    pub fn with_limits(self, limits: KvLimits) -> Self {
        let map = self.inner.read().clone();
        Self {
            inner: Arc::new(MemoryStoreInner {
                map: RwLock::new(map),
                clock: Arc::clone(&self.inner.clock),
                limits,
            }),
        }
    }

    /// Returns the configured [`KvLimits`].
    pub fn limits(&self) -> KvLimits {
        self.inner.limits
    }

    /// Removes all expired entries and returns the count of removed keys.
    pub fn purge_expired(&self) -> usize {
        let now = self.inner.clock.monotonic();
        let mut guard = self.inner.write();
        let before = guard.len();
        guard.retain(|_, entry| !entry.is_expired(now));
        before - guard.len()
    }

    /// Returns the number of live (non-expired) keys currently stored.
    pub fn len(&self) -> usize {
        let now = self.inner.clock.monotonic();
        let guard = self.inner.read();
        guard.values().filter(|e| !e.is_expired(now)).count()
    }

    /// Returns `true` if no live keys are present.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Removes all keys from the store.
    pub fn clear(&self) {
        self.inner.write().clear();
    }
}

fn std_to_akku_duration(d: Duration) -> SignedDuration {
    SignedDuration::try_from(d).unwrap_or(SignedDuration::MAX)
}

impl KvStore for MemoryStore {
    fn get(&self, key: &str) -> KvResult<Option<Vec<u8>>> {
        self.inner.limits.validate_key(key)?;
        let now = self.inner.clock.monotonic();
        {
            let guard = self.inner.read();
            match guard.get(key) {
                None => return Ok(None),
                Some(entry) if !entry.is_expired(now) => return Ok(Some(entry.value.clone())),
                Some(_) => {}
            }
        }
        // Lazily remove expired entry under write lock.
        let mut wguard = self.inner.write();
        if let Some(entry) = wguard.get(key) {
            if entry.is_expired(now) {
                wguard.remove(key);
            } else {
                return Ok(Some(entry.value.clone()));
            }
        }
        Ok(None)
    }

    fn set(&self, key: &str, value: &[u8]) -> KvResult<()> {
        self.inner.limits.validate_kv(key, value)?;
        let mut guard = self.inner.write();
        guard.insert(
            key.to_string(),
            MemoryEntry {
                value: value.to_vec(),
                expiry: None,
            },
        );
        Ok(())
    }

    fn set_ex(&self, key: &str, value: &[u8], ttl: Duration) -> KvResult<()> {
        self.inner.limits.validate_kv(key, value)?;
        let now = self.inner.clock.monotonic();
        let akku_ttl = std_to_akku_duration(ttl);
        let mut guard = self.inner.write();
        guard.insert(
            key.to_string(),
            MemoryEntry {
                value: value.to_vec(),
                expiry: Some((now, akku_ttl)),
            },
        );
        Ok(())
    }

    fn delete(&self, key: &str) -> KvResult<bool> {
        self.inner.limits.validate_key(key)?;
        let now = self.inner.clock.monotonic();
        let mut guard = self.inner.write();
        if let Some(old) = guard.remove(key) {
            Ok(!old.is_expired(now))
        } else {
            Ok(false)
        }
    }

    fn exists(&self, key: &str) -> KvResult<bool> {
        self.inner.limits.validate_key(key)?;
        let now = self.inner.clock.monotonic();
        let guard = self.inner.read();
        Ok(guard.get(key).is_some_and(|e| !e.is_expired(now)))
    }

    fn scan_prefix(&self, prefix: &str) -> KvResult<Vec<(String, Vec<u8>)>> {
        let now = self.inner.clock.monotonic();
        let guard = self.inner.read();
        let mut results = Vec::new();
        for (k, entry) in guard.range(prefix.to_string()..) {
            if !k.starts_with(prefix) {
                break;
            }
            if !entry.is_expired(now) {
                results.push((k.clone(), entry.value.clone()));
            }
        }
        Ok(results)
    }

    fn scan_range(&self, start: &str, end: &str) -> KvResult<Vec<(String, Vec<u8>)>> {
        if start >= end {
            return Ok(Vec::new());
        }
        let now = self.inner.clock.monotonic();
        let guard = self.inner.read();
        let mut results = Vec::new();
        for (k, entry) in guard.range(start.to_string()..end.to_string()) {
            if !entry.is_expired(now) {
                results.push((k.clone(), entry.value.clone()));
            }
        }
        Ok(results)
    }

    fn batch(&self, ops: &[BatchOp]) -> KvResult<()> {
        // Validate all operations before mutating so batch is all-or-nothing.
        for op in ops {
            match op {
                BatchOp::Set { key, value } | BatchOp::SetEx { key, value, .. } => {
                    self.inner.limits.validate_kv(key, value)?;
                }
                BatchOp::Delete { key } => {
                    self.inner.limits.validate_key(key)?;
                }
            }
        }

        let now = self.inner.clock.monotonic();
        let mut guard = self.inner.write();
        for op in ops {
            match op {
                BatchOp::Set { key, value } => {
                    guard.insert(
                        key.clone(),
                        MemoryEntry {
                            value: value.clone(),
                            expiry: None,
                        },
                    );
                }
                BatchOp::SetEx { key, value, ttl } => {
                    guard.insert(
                        key.clone(),
                        MemoryEntry {
                            value: value.clone(),
                            expiry: Some((now, std_to_akku_duration(*ttl))),
                        },
                    );
                }
                BatchOp::Delete { key } => {
                    guard.remove(key);
                }
            }
        }
        Ok(())
    }
}
